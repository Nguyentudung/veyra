//! GPU-native Mode B renderer.
//!
//! This renderer never maps a GPU buffer or builds a CPU BGRA `Vec`.  It uses
//! Rune's device and queue, renders YUV420P into one leased BGRA output slot,
//! then hands that texture to `NativeSurface::push_gpu` through `VideoFrame`.
//! The lease is released only after Rune's sampling submission completes.

use super::{super::ffi, frame::VideoFrame};
use rune::{
    native_surface::{GpuContext, GpuFrame},
    wgpu,
};
use std::{
    any::Any,
    sync::{Arc, Mutex, Weak},
};

/// Four covers the producer, latest-wins mailbox, Rune's prepared frame, and
/// the GPU submission that is still sampling a previous texture.
pub(crate) const OUTPUT_SLOT_COUNT: usize = 4;

const SHADER: &str = r#"
@group(0) @binding(0) var y_tex: texture_2d<f32>;
@group(0) @binding(1) var u_tex: texture_2d<f32>;
@group(0) @binding(2) var v_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
  var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
  var o: Out; o.pos = vec4(p[i], 0.0, 1.0);
  o.uv = vec2((o.pos.x + 1.0) * 0.5, 1.0 - (o.pos.y + 1.0) * 0.5); return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> {
  let y = textureSample(y_tex, samp, i.uv).r;
  let u = textureSample(u_tex, samp, i.uv).r - 0.5;
  let v = textureSample(v_tex, samp, i.uv).r - 0.5;
  let l = (y - 0.0625) * 1.16438356;
  return vec4(clamp(vec3(l + 1.596027 * v, l - 0.391762 * u - 0.812968 * v,
                         l + 2.017232 * u), vec3(0.0), vec3(1.0)), 1.0);
}
"#;

struct OutputSlot {
    // The view in `GpuFrame` keeps the individual resource alive, while the
    // pool keeps all slots alive until their last lease has been dropped.
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct OutputPool {
    id: u64,
    size: [u32; 2],
    slots: Vec<OutputSlot>,
    free: Mutex<Vec<usize>>,
}

impl OutputPool {
    fn take_slot(&self) -> Option<usize> {
        self.free
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop()
    }
}

/// Pure CPU-side ownership bookkeeping. `Drop` is allowed from Rune's wgpu
/// completion callback, so it must never issue a wgpu command.
struct SlotLease {
    pool: Weak<OutputPool>,
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

pub(crate) struct NativeGpuRenderer {
    context: GpuContext,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    plane_textures: Option<(wgpu::Texture, wgpu::Texture, wgpu::Texture)>,
    plane_views: Option<(wgpu::TextureView, wgpu::TextureView, wgpu::TextureView)>,
    bind_group: Option<wgpu::BindGroup>,
    plane_dimensions: (u32, u32),
    // Negative FFmpeg strides are uncommon. Only they need a packed CPU
    // fallback; normal positive-stride planes go straight to write_texture.
    plane_buffers: [Vec<u8>; 3],
    output_pool: Option<Arc<OutputPool>>,
    submitted_frames: u64,
    starved_frames: u64,
}

impl NativeGpuRenderer {
    pub(crate) fn new(context: GpuContext) -> Result<Self, String> {
        let device = &context.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("veyra-native-yuv420-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("veyra-native-yuv-bindings"),
            entries: &[
                texture_binding(0),
                texture_binding(1),
                texture_binding(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("veyra-native-yuv-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("veyra-native-yuv-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    // This must match Rune's existing ByteFrame BGRA path so
                    // the compositor applies exactly the same gamma handling.
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("veyra-native-video-linear-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Ok(Self {
            context,
            pipeline,
            bind_group_layout,
            sampler,
            plane_textures: None,
            plane_views: None,
            bind_group: None,
            plane_dimensions: (0, 0),
            plane_buffers: [Vec::new(), Vec::new(), Vec::new()],
            output_pool: None,
            submitted_frames: 0,
            starved_frames: 0,
        })
    }

    pub(crate) fn generation(&self) -> u64 {
        self.context.generation
    }

    pub(crate) unsafe fn render_yuv420p(
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

        self.ensure_plane_textures(width, height);
        let pool = self.ensure_output_pool(width, height);
        let Some(slot) = pool.take_slot() else {
            self.starved_frames = self.starved_frames.saturating_add(1);
            self.log_diagnostics();
            return Ok(None);
        };
        // Create this before any fallible upload. If FFmpeg hands us an invalid
        // plane, unwinding the local Arc returns the slot instead of silently
        // exhausting the pool on later frames.
        let lease: Arc<dyn Any + Send + Sync> = Arc::new(SlotLease {
            pool: Arc::downgrade(&pool),
            slot,
        });

        let (y, u, v) = self.plane_textures.as_ref().expect("plane textures");
        upload_frame_plane(
            &self.context.queue,
            y,
            (*frame).data[0],
            (*frame).linesize[0],
            width,
            height,
            &mut self.plane_buffers[0],
        )?;
        upload_frame_plane(
            &self.context.queue,
            u,
            (*frame).data[1],
            (*frame).linesize[1],
            width.div_ceil(2),
            height.div_ceil(2),
            &mut self.plane_buffers[1],
        )?;
        upload_frame_plane(
            &self.context.queue,
            v,
            (*frame).data[2],
            (*frame).linesize[2],
            width.div_ceil(2),
            height.div_ceil(2),
            &mut self.plane_buffers[2],
        )?;

        let output = &pool.slots[slot];
        let mut encoder =
            self.context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("veyra-native-video-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("veyra-native-yuv-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, self.bind_group.as_ref().expect("YUV bind group"), &[]);
            pass.draw(0..3, 0..1);
        }
        // The queue is Rune's queue. Its submission ordering therefore makes
        // these plane uploads and this output render visible before Rune later
        // samples the published output texture.
        self.context.queue.submit(Some(encoder.finish()));

        self.submitted_frames = self.submitted_frames.saturating_add(1);
        self.log_diagnostics();
        Ok(Some(VideoFrame::Gpu {
            frame: GpuFrame {
                view: output.view.clone(),
                size_px: pool.size,
                generation: self.context.generation,
                pool_id: pool.id,
                slot: slot as u32,
                lease,
            },
            timestamp,
        }))
    }

    fn ensure_output_pool(&mut self, width: u32, height: u32) -> Arc<OutputPool> {
        if let Some(pool) = self
            .output_pool
            .as_ref()
            .filter(|pool| pool.size == [width, height])
        {
            return Arc::clone(pool);
        }
        let slots = (0..OUTPUT_SLOT_COUNT)
            .map(|slot| {
                let texture = self
                    .context
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("veyra-native-video-output"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Bgra8Unorm,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    });
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("veyra-native-video-output-view"),
                    ..Default::default()
                });
                let _ = slot;
                OutputSlot {
                    _texture: texture,
                    view,
                }
            })
            .collect();
        let pool = Arc::new(OutputPool {
            id: super::next_pool_id(),
            size: [width, height],
            slots,
            free: Mutex::new((0..OUTPUT_SLOT_COUNT).collect()),
        });
        self.output_pool = Some(Arc::clone(&pool));
        pool
    }

    fn ensure_plane_textures(&mut self, width: u32, height: u32) {
        if self.plane_textures.is_some() && self.plane_dimensions == (width, height) {
            return;
        }
        self.plane_textures = Some((
            self.create_plane_texture("veyra-native-y", width, height),
            self.create_plane_texture("veyra-native-u", width.div_ceil(2), height.div_ceil(2)),
            self.create_plane_texture("veyra-native-v", width.div_ceil(2), height.div_ceil(2)),
        ));
        self.plane_dimensions = (width, height);
        let (y, u, v) = self
            .plane_textures
            .as_ref()
            .expect("created plane textures");
        self.plane_views = Some((
            y.create_view(&Default::default()),
            u.create_view(&Default::default()),
            v.create_view(&Default::default()),
        ));
        let (y, u, v) = self.plane_views.as_ref().expect("created plane views");
        self.bind_group = Some(
            self.context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("veyra-native-yuv-bind-group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        binding_view(0, y),
                        binding_view(1, u),
                        binding_view(2, v),
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                }),
        );
    }

    fn create_plane_texture(&self, label: &str, width: u32, height: u32) -> wgpu::Texture {
        self.context
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
    }

    fn log_diagnostics(&self) {
        if self.submitted_frames > 0 && self.submitted_frames % 300 == 0 {
            let free = self.output_pool.as_ref().map_or(0, |pool| {
                pool.free
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .len()
            });
            eprintln!(
                "[video] gpu-native: submitted={} starved={} free_slots={}/{} pool_id={}",
                self.submitted_frames,
                self.starved_frames,
                free,
                OUTPUT_SLOT_COUNT,
                self.output_pool.as_ref().map_or(0, |pool| pool.id),
            );
        }
    }
}

fn texture_binding(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn binding_view<'a>(binding: u32, view: &'a wgpu::TextureView) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

unsafe fn upload_frame_plane(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: *const u8,
    stride: i32,
    width: u32,
    height: u32,
    fallback: &mut Vec<u8>,
) -> Result<(), String> {
    if data.is_null() || stride.unsigned_abs() < width {
        return Err("invalid FFmpeg plane stride".into());
    }
    if stride > 0 {
        let pitch = stride as usize;
        let len = pitch
            .checked_mul(height.saturating_sub(1) as usize)
            .and_then(|n| n.checked_add(width as usize))
            .ok_or("FFmpeg plane size overflow")?;
        write_plane(
            queue,
            texture,
            std::slice::from_raw_parts(data, len),
            stride as u32,
            width,
            height,
        );
        return Ok(());
    }

    fallback.resize((width * height) as usize, 0);
    for row in 0..height as usize {
        std::ptr::copy_nonoverlapping(
            data.offset(row as isize * stride as isize),
            fallback.as_mut_ptr().add(row * width as usize),
            width as usize,
        );
    }
    write_plane(queue, texture, fallback, width, width, height);
    Ok(())
}

fn write_plane(
    queue: &wgpu::Queue,
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
}
