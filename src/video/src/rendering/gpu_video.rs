//! Direct NV12 texture handoff (Mode C).
//!
//! The decoder worker uploads luma and interleaved chroma into a leased pair
//! of textures on Rune's queue. Rune's existing NV12 compositor then performs
//! YUV conversion, scaling and presentation in its normal render pass. There
//! is intentionally no Veyra command encoder, GPU render pass or queue submit.

use super::{super::ffi, frame::VideoFrame};
use rune::{
    native_surface::{ColorInfo, Format, GpuContext, GpuVideoFrame, Matrix, Transfer},
    wgpu,
};
use std::{
    any::Any,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

// The latest-wins handoff needs one texture being uploaded, one in the
// mailbox, and one still sampled by Rune. A fourth slot only masks a stalled
// compositor while retaining another full NV12 picture for the whole session.
// Keep the steady-state pool at three so the normal GPU path does not reserve
// an extra 1.5 bytes/pixel of video memory.
const VIDEO_SLOT_COUNT: usize = 3;

struct PlaneSlot {
    _y: wgpu::Texture,
    _uv: wgpu::Texture,
    views: [wgpu::TextureView; 2],
}

struct PlanePool {
    id: u64,
    size: [u32; 2],
    slots: Vec<PlaneSlot>,
    free: Mutex<Vec<usize>>,
}

impl PlanePool {
    fn take_slot(&self) -> Option<usize> {
        self.free
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop()
    }
}

/// Ownership only: this may be dropped by Rune's completion callback, so it
/// must never invoke wgpu from `Drop`.
struct SlotLease {
    pool: Weak<PlanePool>,
    slot: usize,
}

impl Drop for SlotLease {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.upgrade() {
            pool.free
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(self.slot);
        }
    }
}

pub(crate) struct NativeVideoRenderer {
    context: GpuContext,
    pool: Option<Arc<PlanePool>>,
    next_pool_id: u64,
    // This is the only per-frame CPU conversion for decoded YUV420P. It keeps
    // capacity across frames and is not involved when FFmpeg supplies NV12.
    uv_interleaved: Vec<u8>,
    y_fallback: Vec<u8>,
    uv_fallback: Vec<u8>,
    uploaded_frames: u64,
    starved_frames: u64,
    last_interleave_ms: f64,
    last_upload_ms: f64,
}

impl NativeVideoRenderer {
    pub(crate) fn new(context: GpuContext) -> Result<Self, String> {
        context.compositor_diagnostics.set_renderer("gpu-video");
        Ok(Self {
            context,
            pool: None,
            next_pool_id: 0,
            uv_interleaved: Vec::new(),
            y_fallback: Vec::new(),
            uv_fallback: Vec::new(),
            uploaded_frames: 0,
            starved_frames: 0,
            last_interleave_ms: 0.0,
            last_upload_ms: 0.0,
        })
    }

    pub(crate) fn generation(&self) -> u64 {
        self.context.generation
    }

    pub(crate) unsafe fn upload(
        &mut self,
        frame: *mut ffi::AVFrame,
        timestamp: f64,
    ) -> Result<Option<VideoFrame>, String> {
        let width = (*frame).width as u32;
        let height = (*frame).height as u32;
        if width == 0 || height == 0 {
            return Err("invalid video dimensions".into());
        }
        if width > self.context.device.limits().max_texture_dimension_2d
            || height > self.context.device.limits().max_texture_dimension_2d
        {
            return Err("video dimensions exceed Rune GPU texture limit".into());
        }
        let (format, color) = color_info(frame, width, height)?;
        let pool = self.ensure_pool(width, height);
        let Some(slot) = pool.take_slot() else {
            self.starved_frames = self.starved_frames.saturating_add(1);
            if self.starved_frames % 60 == 1 {
                eprintln!(
                    "[video] gpu-video: output-plane pool starved; dropped={} free_slots=0/{}",
                    self.starved_frames, VIDEO_SLOT_COUNT
                );
            }
            return Ok(None);
        };
        // Create the lease before any fallible pointer validation/copy. An
        // error then returns the slot rather than slowly exhausting the pool.
        let lease: Arc<dyn Any + Send + Sync> = Arc::new(SlotLease {
            pool: Arc::downgrade(&pool),
            slot,
        });
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let slot_textures = &pool.slots[slot];
        let upload_started = Instant::now();
        upload_r8_plane(
            &self.context.queue,
            &self.context.compositor_diagnostics,
            &slot_textures._y,
            (*frame).data[0],
            (*frame).linesize[0],
            width,
            height,
            &mut self.y_fallback,
        )?;

        let interleave_started = Instant::now();
        match (*frame).format {
            ffi::AVPixelFormat_AV_PIX_FMT_NV12 => upload_rg8_plane(
                &self.context.queue,
                &self.context.compositor_diagnostics,
                &slot_textures._uv,
                (*frame).data[1],
                (*frame).linesize[1],
                chroma_width,
                chroma_height,
                &mut self.uv_fallback,
            )?,
            ffi::AVPixelFormat_AV_PIX_FMT_YUV420P => {
                interleave_yuv420p(
                    (*frame).data[1],
                    (*frame).linesize[1],
                    (*frame).data[2],
                    (*frame).linesize[2],
                    chroma_width,
                    chroma_height,
                    &mut self.uv_interleaved,
                )?;
                write_texture(
                    &self.context.queue,
                    &self.context.compositor_diagnostics,
                    &slot_textures._uv,
                    &self.uv_interleaved,
                    chroma_width * 2,
                    chroma_width,
                    chroma_height,
                );
            }
            _ => return Err("gpu-video supports only YUV420P and NV12".into()),
        }
        self.last_interleave_ms = interleave_started.elapsed().as_secs_f64() * 1000.0;
        self.last_upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;

        // `write_texture` work is submitted by Rune's next compositor submit
        // on this same queue. No producer-side encoder or submission exists.
        self.uploaded_frames = self.uploaded_frames.saturating_add(1);
        self.log_diagnostics();
        Ok(Some(VideoFrame::GpuVideo {
            frame: GpuVideoFrame {
                planes: slot_textures.views.clone(),
                size_px: pool.size,
                format,
                color,
                generation: self.context.generation,
                pool_id: pool.id,
                slot: slot as u32,
                lease,
            },
            timestamp,
        }))
    }

    fn ensure_pool(&mut self, width: u32, height: u32) -> Arc<PlanePool> {
        if let Some(pool) = self
            .pool
            .as_ref()
            .filter(|pool| pool.size == [width, height])
        {
            return Arc::clone(pool);
        }
        self.next_pool_id = self.next_pool_id.wrapping_add(1);
        let device = &self.context.device;
        let slots = (0..VIDEO_SLOT_COUNT)
            .map(|_| {
                let y = plane_texture(
                    device,
                    "veyra-video-y",
                    width,
                    height,
                    wgpu::TextureFormat::R8Unorm,
                );
                let uv = plane_texture(
                    device,
                    "veyra-video-uv",
                    width.div_ceil(2),
                    height.div_ceil(2),
                    wgpu::TextureFormat::Rg8Unorm,
                );
                let views = [
                    y.create_view(&Default::default()),
                    uv.create_view(&Default::default()),
                ];
                PlaneSlot {
                    _y: y,
                    _uv: uv,
                    views,
                }
            })
            .collect();
        let pool = Arc::new(PlanePool {
            id: self.next_pool_id,
            size: [width, height],
            slots,
            free: Mutex::new((0..VIDEO_SLOT_COUNT).collect()),
        });
        self.pool = Some(Arc::clone(&pool));
        pool
    }

    fn log_diagnostics(&self) {
        if self.uploaded_frames > 0 && self.uploaded_frames % 300 == 0 {
            let free = self.pool.as_ref().map_or(0, |pool| {
                pool.free
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .len()
            });
            eprintln!(
                "[video] gpu-video: uploaded={} starved={} free_slots={}/{} upload_ms={:.2} interleave_ms={:.2} pool_id={}",
                self.uploaded_frames,
                self.starved_frames,
                free,
                VIDEO_SLOT_COUNT,
                self.last_upload_ms,
                self.last_interleave_ms,
                self.pool.as_ref().map_or(0, |pool| pool.id),
            );
        }
    }
}

fn plane_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

unsafe fn upload_r8_plane(
    queue: &wgpu::Queue,
    diagnostics: &rune::native_surface::CompositorDiagnostics,
    texture: &wgpu::Texture,
    data: *const u8,
    stride: i32,
    width: u32,
    height: u32,
    fallback: &mut Vec<u8>,
) -> Result<(), String> {
    upload_plane(
        queue,
        diagnostics,
        texture,
        data,
        stride,
        width,
        height,
        1,
        fallback,
    )
}

unsafe fn upload_rg8_plane(
    queue: &wgpu::Queue,
    diagnostics: &rune::native_surface::CompositorDiagnostics,
    texture: &wgpu::Texture,
    data: *const u8,
    stride: i32,
    width: u32,
    height: u32,
    fallback: &mut Vec<u8>,
) -> Result<(), String> {
    upload_plane(
        queue,
        diagnostics,
        texture,
        data,
        stride,
        width,
        height,
        2,
        fallback,
    )
}

unsafe fn upload_plane(
    queue: &wgpu::Queue,
    diagnostics: &rune::native_surface::CompositorDiagnostics,
    texture: &wgpu::Texture,
    data: *const u8,
    stride: i32,
    width: u32,
    height: u32,
    bytes_per_texel: u32,
    fallback: &mut Vec<u8>,
) -> Result<(), String> {
    let packed_row = width
        .checked_mul(bytes_per_texel)
        .ok_or("video plane row size overflow")?;
    if data.is_null() || stride.unsigned_abs() < packed_row {
        return Err("invalid FFmpeg plane stride".into());
    }
    if stride > 0 {
        let pitch = stride as usize;
        let len = pitch
            .checked_mul(height.saturating_sub(1) as usize)
            .and_then(|n| n.checked_add(packed_row as usize))
            .ok_or("FFmpeg plane size overflow")?;
        write_texture(
            queue,
            diagnostics,
            texture,
            std::slice::from_raw_parts(data, len),
            stride as u32,
            width,
            height,
        );
        return Ok(());
    }
    fallback.resize((packed_row * height) as usize, 0);
    for row in 0..height as usize {
        std::ptr::copy_nonoverlapping(
            data.offset(row as isize * stride as isize),
            fallback.as_mut_ptr().add(row * packed_row as usize),
            packed_row as usize,
        );
    }
    write_texture(
        queue,
        diagnostics,
        texture,
        fallback,
        packed_row,
        width,
        height,
    );
    Ok(())
}

unsafe fn interleave_yuv420p(
    u: *const u8,
    u_stride: i32,
    v: *const u8,
    v_stride: i32,
    width: u32,
    height: u32,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    if u.is_null()
        || v.is_null()
        || u_stride.unsigned_abs() < width
        || v_stride.unsigned_abs() < width
    {
        return Err("invalid FFmpeg chroma plane stride".into());
    }
    out.resize((width * height * 2) as usize, 0);
    for row in 0..height as usize {
        let u_row = u.offset(row as isize * u_stride as isize);
        let v_row = v.offset(row as isize * v_stride as isize);
        let dst = out.as_mut_ptr().add(row * width as usize * 2);
        for column in 0..width as usize {
            *dst.add(column * 2) = *u_row.add(column);
            *dst.add(column * 2 + 1) = *v_row.add(column);
        }
    }
    Ok(())
}

fn write_texture(
    queue: &wgpu::Queue,
    diagnostics: &rune::native_surface::CompositorDiagnostics,
    texture: &wgpu::Texture,
    bytes: &[u8],
    bytes_per_row: u32,
    width: u32,
    height: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Default::default(),
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    diagnostics.record_write_texture();
}

unsafe fn color_info(
    frame: *const ffi::AVFrame,
    width: u32,
    height: u32,
) -> Result<(Format, ColorInfo), String> {
    let format = match (*frame).color_range {
        ffi::AVColorRange_AVCOL_RANGE_JPEG => Format::Nv12Full,
        ffi::AVColorRange_AVCOL_RANGE_MPEG | ffi::AVColorRange_AVCOL_RANGE_UNSPECIFIED => {
            Format::Nv12Video
        }
        _ => return Err("unsupported FFmpeg color range for gpu-video".into()),
    };
    let matrix = match (*frame).colorspace {
        ffi::AVColorSpace_AVCOL_SPC_BT709 => Matrix::Bt709,
        ffi::AVColorSpace_AVCOL_SPC_BT470BG | ffi::AVColorSpace_AVCOL_SPC_SMPTE170M => {
            Matrix::Bt601
        }
        // FFmpeg leaves matrix unspecified for some old content. This is the
        // conventional SD/HD default, documented rather than silently treating
        // all content as BT.601.
        ffi::AVColorSpace_AVCOL_SPC_UNSPECIFIED => {
            if width >= 1280 || height > 576 {
                Matrix::Bt709
            } else {
                Matrix::Bt601
            }
        }
        _ => return Err("unsupported FFmpeg color matrix for gpu-video".into()),
    };
    let transfer = match (*frame).color_trc {
        ffi::AVColorTransferCharacteristic_AVCOL_TRC_IEC61966_2_1 => Transfer::Srgb,
        ffi::AVColorTransferCharacteristic_AVCOL_TRC_BT709
        | ffi::AVColorTransferCharacteristic_AVCOL_TRC_SMPTE170M
        | ffi::AVColorTransferCharacteristic_AVCOL_TRC_SMPTE240M
        | ffi::AVColorTransferCharacteristic_AVCOL_TRC_BT1361_ECG
        | ffi::AVColorTransferCharacteristic_AVCOL_TRC_UNSPECIFIED => Transfer::Bt709,
        _ => return Err("unsupported FFmpeg transfer function for gpu-video".into()),
    };
    Ok((format, ColorInfo { matrix, transfer }))
}
