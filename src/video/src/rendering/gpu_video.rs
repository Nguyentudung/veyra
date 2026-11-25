//! Direct YUV texture handoff (Mode C).
//!
//! The decoder worker uploads decoded luma/chroma into leased textures on
//! Rune's queue. NV12 stays two-plane; YUV420P uploads its Cb and Cr planes
//! directly, so it does not need a CPU-side chroma interleave. Rune's video
//! compositor performs YUV conversion, scaling and presentation in its normal
//! render pass. There is intentionally no Veyra command encoder, GPU render
//! pass or queue submit.

use super::{super::ffi, frame::VideoFrame};
use rune::{
    native_surface::{
        ColorInfo, Format, GpuContext, GpuPlanarVideoFrame, GpuVideoFrame, Matrix, Transfer,
    },
    wgpu,
};
use std::{
    any::Any,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Instant,
};

// Three slots cover the usual handoff. Rune may temporarily retain an older
// frame in its prepared draw and completion callback while a newer one waits
// in the mailbox. Reserve a fourth only when all three are leased, so high-rate
// streams can keep advancing without paying for that texture on every video.
const VIDEO_SLOT_COUNT: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlaneLayout {
    Nv12,
    Yuv420p,
}

impl PlaneLayout {
    fn from_pixel_format(format: i32, planar: bool) -> Option<Self> {
        match format {
            ffi::AVPixelFormat_AV_PIX_FMT_NV12 => Some(Self::Nv12),
            ffi::AVPixelFormat_AV_PIX_FMT_YUV420P if planar => Some(Self::Yuv420p),
            ffi::AVPixelFormat_AV_PIX_FMT_YUV420P => Some(Self::Nv12),
            _ => None,
        }
    }
}

struct PlaneSlot {
    _y: wgpu::Texture,
    chroma: ChromaPlanes,
}

enum ChromaPlanes {
    Nv12 {
        _uv: wgpu::Texture,
        views: [wgpu::TextureView; 2],
    },
    Yuv420p {
        _u: wgpu::Texture,
        _v: wgpu::Texture,
        views: [wgpu::TextureView; 3],
    },
}

impl PlaneSlot {
    fn frame(
        &self,
        size_px: [u32; 2],
        format: Format,
        color: ColorInfo,
        generation: u64,
        pool_id: u64,
        slot: u32,
        lease: Arc<dyn Any + Send + Sync>,
        timestamp: f64,
    ) -> VideoFrame {
        match &self.chroma {
            ChromaPlanes::Nv12 { views, .. } => VideoFrame::GpuVideo {
                frame: GpuVideoFrame {
                    planes: views.clone(),
                    size_px,
                    format,
                    color,
                    generation,
                    pool_id,
                    slot,
                    lease,
                },
                timestamp,
            },
            ChromaPlanes::Yuv420p { views, .. } => VideoFrame::GpuPlanarVideo {
                frame: GpuPlanarVideoFrame {
                    planes: views.clone(),
                    size_px,
                    format,
                    color,
                    generation,
                    pool_id,
                    slot,
                    lease,
                },
                timestamp,
            },
        }
    }
}

struct PlanePool {
    id: u64,
    size: [u32; 2],
    layout: PlaneLayout,
    slots: Vec<PlaneSlot>,
    extra_slot: OnceLock<PlaneSlot>,
    free: Mutex<Vec<usize>>,
}

impl PlanePool {
    fn take_slot(&self) -> Option<usize> {
        self.free
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop()
    }

    fn take_slot_or_expand(&self, device: &wgpu::Device) -> Option<usize> {
        if let Some(slot) = self.take_slot() {
            return Some(slot);
        }
        // NativeVideoRenderer has a single producer thread; completion
        // callbacks only return indices to `free`. Therefore this one-shot
        // expansion cannot hand the extra slot to two uploaders.
        if self.extra_slot.get().is_none() {
            self.extra_slot
                .get_or_init(|| plane_slot(device, self.size[0], self.size[1], self.layout));
            eprintln!("[video] gpu-video: expanded output-plane pool to 4 slots");
            Some(VIDEO_SLOT_COUNT)
        } else {
            self.take_slot()
        }
    }

    fn slot(&self, index: usize) -> &PlaneSlot {
        if index == VIDEO_SLOT_COUNT {
            self.extra_slot.get().expect("expanded plane slot exists")
        } else {
            &self.slots[index]
        }
    }

    fn capacity(&self) -> usize {
        VIDEO_SLOT_COUNT + usize::from(self.extra_slot.get().is_some())
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
    planar_yuv420p: bool,
    uv_interleaved: Vec<u8>,
    y_fallback: Vec<u8>,
    chroma_fallback: Vec<u8>,
    uploaded_frames: u64,
    starved_frames: u64,
    last_chroma_upload_ms: f64,
    last_upload_ms: f64,
}

impl NativeVideoRenderer {
    pub(crate) fn new(context: GpuContext) -> Result<Self, String> {
        context.compositor_diagnostics.set_renderer("gpu-video");
        let planar_yuv420p = std::env::var("VEYRA_YUV420P_UPLOAD").as_deref() != Ok("interleaved");
        Ok(Self {
            context,
            pool: None,
            planar_yuv420p,
            uv_interleaved: Vec::new(),
            y_fallback: Vec::new(),
            chroma_fallback: Vec::new(),
            uploaded_frames: 0,
            starved_frames: 0,
            last_chroma_upload_ms: 0.0,
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
        let layout = PlaneLayout::from_pixel_format((*frame).format, self.planar_yuv420p)
            .ok_or("gpu-video supports only YUV420P and NV12")?;
        let (format, color) = color_info(frame, width, height)?;
        let pool = self.ensure_pool(width, height, layout);
        let Some(slot) = pool.take_slot_or_expand(&self.context.device) else {
            self.starved_frames = self.starved_frames.saturating_add(1);
            if self.starved_frames % 60 == 1 {
                eprintln!(
                    "[video] gpu-video: output-plane pool starved; dropped={} free_slots=0/{}",
                    self.starved_frames,
                    pool.capacity()
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
        let slot_textures = pool.slot(slot);
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

        let chroma_started = Instant::now();
        match (&slot_textures.chroma, (*frame).format) {
            (ChromaPlanes::Nv12 { _uv, .. }, ffi::AVPixelFormat_AV_PIX_FMT_NV12) => {
                upload_rg8_plane(
                    &self.context.queue,
                    &self.context.compositor_diagnostics,
                    _uv,
                    (*frame).data[1],
                    (*frame).linesize[1],
                    chroma_width,
                    chroma_height,
                    &mut self.chroma_fallback,
                )?
            }
            (ChromaPlanes::Nv12 { _uv, .. }, ffi::AVPixelFormat_AV_PIX_FMT_YUV420P) => {
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
                    _uv,
                    &self.uv_interleaved,
                    chroma_width * 2,
                    chroma_width,
                    chroma_height,
                );
            }
            (ChromaPlanes::Yuv420p { _u, _v, .. }, ffi::AVPixelFormat_AV_PIX_FMT_YUV420P) => {
                upload_r8_plane(
                    &self.context.queue,
                    &self.context.compositor_diagnostics,
                    _u,
                    (*frame).data[1],
                    (*frame).linesize[1],
                    chroma_width,
                    chroma_height,
                    &mut self.chroma_fallback,
                )?;
                upload_r8_plane(
                    &self.context.queue,
                    &self.context.compositor_diagnostics,
                    _v,
                    (*frame).data[2],
                    (*frame).linesize[2],
                    chroma_width,
                    chroma_height,
                    &mut self.chroma_fallback,
                )?;
            }
            _ => return Err("gpu-video plane layout changed while uploading".into()),
        }
        self.last_chroma_upload_ms = chroma_started.elapsed().as_secs_f64() * 1000.0;
        self.last_upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;

        // `write_texture` work is submitted by Rune's next compositor submit
        // on this same queue. No producer-side encoder or submission exists.
        self.uploaded_frames = self.uploaded_frames.saturating_add(1);
        self.log_diagnostics();
        Ok(Some(slot_textures.frame(
            pool.size,
            format,
            color,
            self.context.generation,
            pool.id,
            slot as u32,
            lease,
            timestamp,
        )))
    }

    fn ensure_pool(&mut self, width: u32, height: u32, layout: PlaneLayout) -> Arc<PlanePool> {
        if let Some(pool) = self
            .pool
            .as_ref()
            .filter(|pool| pool.size == [width, height] && pool.layout == layout)
        {
            return Arc::clone(pool);
        }
        let slots = (0..VIDEO_SLOT_COUNT)
            .map(|_| plane_slot(&self.context.device, width, height, layout))
            .collect();
        let pool = Arc::new(PlanePool {
            id: super::next_pool_id(),
            size: [width, height],
            layout,
            slots,
            extra_slot: OnceLock::new(),
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
                "[video] gpu-video: uploaded={} starved={} free_slots={}/{} upload_ms={:.2} chroma_upload_ms={:.2} pool_id={}",
                self.uploaded_frames,
                self.starved_frames,
                free,
                self.pool
                    .as_ref()
                    .map_or(VIDEO_SLOT_COUNT, |pool| pool.capacity()),
                self.last_upload_ms,
                self.last_chroma_upload_ms,
                self.pool.as_ref().map_or(0, |pool| pool.id),
            );
        }
    }
}

fn plane_slot(device: &wgpu::Device, width: u32, height: u32, layout: PlaneLayout) -> PlaneSlot {
    let y = plane_texture(
        device,
        "veyra-video-y",
        width,
        height,
        wgpu::TextureFormat::R8Unorm,
    );
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let chroma = match layout {
        PlaneLayout::Nv12 => {
            let uv = plane_texture(
                device,
                "veyra-video-uv",
                chroma_width,
                chroma_height,
                wgpu::TextureFormat::Rg8Unorm,
            );
            ChromaPlanes::Nv12 {
                views: [
                    y.create_view(&Default::default()),
                    uv.create_view(&Default::default()),
                ],
                _uv: uv,
            }
        }
        PlaneLayout::Yuv420p => {
            let u = plane_texture(
                device,
                "veyra-video-u",
                chroma_width,
                chroma_height,
                wgpu::TextureFormat::R8Unorm,
            );
            let v = plane_texture(
                device,
                "veyra-video-v",
                chroma_width,
                chroma_height,
                wgpu::TextureFormat::R8Unorm,
            );
            ChromaPlanes::Yuv420p {
                views: [
                    y.create_view(&Default::default()),
                    u.create_view(&Default::default()),
                    v.create_view(&Default::default()),
                ],
                _u: u,
                _v: v,
            }
        }
    };
    PlaneSlot { _y: y, chroma }
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
        interleave_chroma_row(u_row, v_row, dst, width as usize);
    }
    Ok(())
}

unsafe fn interleave_chroma_row(u: *const u8, v: *const u8, dst: *mut u8, width: usize) {
    let mut column = 0;
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::{
            __m128i, _mm_loadu_si128, _mm_storeu_si128, _mm_unpackhi_epi8, _mm_unpacklo_epi8,
        };
        while column + 16 <= width {
            let u_bytes = _mm_loadu_si128(u.add(column).cast::<__m128i>());
            let v_bytes = _mm_loadu_si128(v.add(column).cast::<__m128i>());
            _mm_storeu_si128(
                dst.add(column * 2).cast::<__m128i>(),
                _mm_unpacklo_epi8(u_bytes, v_bytes),
            );
            _mm_storeu_si128(
                dst.add(column * 2 + 16).cast::<__m128i>(),
                _mm_unpackhi_epi8(u_bytes, v_bytes),
            );
            column += 16;
        }
    }
    while column < width {
        *dst.add(column * 2) = *u.add(column);
        *dst.add(column * 2 + 1) = *v.add(column);
        column += 1;
    }
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

#[cfg(test)]
mod layout_tests {
    use super::{PlaneLayout, ffi, interleave_yuv420p};

    #[test]
    fn direct_upload_selects_the_matching_plane_layout() {
        assert_eq!(
            PlaneLayout::from_pixel_format(ffi::AVPixelFormat_AV_PIX_FMT_NV12, true),
            Some(PlaneLayout::Nv12)
        );
        assert_eq!(
            PlaneLayout::from_pixel_format(ffi::AVPixelFormat_AV_PIX_FMT_YUV420P, true),
            Some(PlaneLayout::Yuv420p)
        );
        assert_eq!(
            PlaneLayout::from_pixel_format(ffi::AVPixelFormat_AV_PIX_FMT_YUV420P, false),
            Some(PlaneLayout::Nv12)
        );
        assert_eq!(PlaneLayout::from_pixel_format(-1, true), None);
    }

    #[test]
    fn interleave_handles_padding_negative_strides_and_simd_tails() {
        for width in [1_usize, 15, 16, 17, 31, 32, 33] {
            let pitch = width + 3;
            let mut u = vec![0_u8; pitch * 2];
            let mut v = vec![0_u8; pitch * 2];
            for row in 0..2 {
                for col in 0..width {
                    u[row * pitch + col] = (row * 31 + col) as u8;
                    v[row * pitch + col] = (row * 47 + col * 2) as u8;
                }
            }
            for negative in [false, true] {
                let start = if negative { pitch } else { 0 };
                let stride = if negative {
                    -(pitch as i32)
                } else {
                    pitch as i32
                };
                let mut actual = Vec::new();
                unsafe {
                    interleave_yuv420p(
                        u.as_ptr().add(start),
                        stride,
                        v.as_ptr().add(start),
                        stride,
                        width as u32,
                        2,
                        &mut actual,
                    )
                    .unwrap();
                }
                let mut expected = Vec::new();
                for row in 0..2 {
                    let src = if negative { 1 - row } else { row };
                    for col in 0..width {
                        expected.extend_from_slice(&[u[src * pitch + col], v[src * pitch + col]]);
                    }
                }
                assert_eq!(actual, expected, "width={width}, negative={negative}");
            }
        }
    }
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
