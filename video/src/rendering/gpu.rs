//! Off-screen Vulkan renderer with bounded asynchronous readback.
//!
//! Rune's public NativeSurface API requires CPU BGRA bytes. Three readback
//! slots allow CPU submission to overlap completion of older GPU copies.

use super::{super::ffi, frame::VideoFrame};
use std::{sync::mpsc, time::Instant};

const READBACK_SLOT_COUNT: usize = 3;

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

struct ReadbackSlot {
    buffer: Option<wgpu::Buffer>,
    state: SlotState,
}

enum SlotState {
    Idle,
    Mapping {
        width: u32,
        height: u32,
        stride: u32,
        timestamp: f64,
        generation: u64,
        submitted_at: Instant,
    },
}

struct MapResult {
    slot: usize,
    result: Result<(), wgpu::BufferAsyncError>,
}

pub(crate) struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    plane_textures: Option<(wgpu::Texture, wgpu::Texture, wgpu::Texture)>,
    plane_views: Option<(wgpu::TextureView, wgpu::TextureView, wgpu::TextureView)>,
    bind_group: Option<wgpu::BindGroup>,
    plane_dimensions: (u32, u32),
    plane_buffers: [Vec<u8>; 3],
    output: Option<(wgpu::Texture, u32, u32)>,
    readbacks: Vec<ReadbackSlot>,
    map_tx: mpsc::Sender<MapResult>,
    map_rx: mpsc::Receiver<MapResult>,
    resource_creations: u64,
    submitted_frames: u64,
    skipped_readbacks: u64,
    completed_frames: u64,
    last_upload_ms: f64,
    last_readback_latency_ms: f64,
    generation: u64,
}

impl GpuRenderer {
    pub(crate) fn new() -> Result<Self, String> {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(instance_desc);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("Vulkan adapter unavailable: {e}"))?;
        let info = adapter.get_info();
        eprintln!(
            "[video] Vulkan GPU renderer: {} ({:?})",
            info.name, info.backend
        );
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("veyra-video-vulkan"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("Vulkan device unavailable: {e}"))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("veyra-yuv420-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("veyra-yuv-bindings"),
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
            label: Some("veyra-yuv-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("veyra-yuv-pipeline"),
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
            label: Some("veyra-video-linear-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (map_tx, map_rx) = mpsc::channel();
        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            sampler,
            plane_textures: None,
            plane_views: None,
            bind_group: None,
            plane_dimensions: (0, 0),
            plane_buffers: [Vec::new(), Vec::new(), Vec::new()],
            output: None,
            readbacks: (0..READBACK_SLOT_COUNT)
                .map(|_| ReadbackSlot {
                    buffer: None,
                    state: SlotState::Idle,
                })
                .collect(),
            map_tx,
            map_rx,
            resource_creations: 0,
            submitted_frames: 0,
            skipped_readbacks: 0,
            completed_frames: 0,
            last_upload_ms: 0.0,
            last_readback_latency_ms: 0.0,
            generation: 0,
        })
    }

    /// Submits without waiting; returns only a readback that was already ready.
    pub(crate) unsafe fn render_yuv420p(
        &mut self,
        frame: *mut ffi::AVFrame,
        timestamp: f64,
    ) -> Result<Option<VideoFrame>, String> {
        let ready = self.poll_completed()?;
        let width = (*frame).width as u32;
        let height = (*frame).height as u32;
        if width == 0 || height == 0 {
            return Err("invalid video dimensions".into());
        }
        let Some(slot_index) = self.idle_slot() else {
            self.skipped_readbacks = self.skipped_readbacks.saturating_add(1);
            self.log_diagnostics();
            return Ok(ready);
        };
        let upload_started = Instant::now();
        pack_plane_into(
            (*frame).data[0],
            (*frame).linesize[0],
            width,
            height,
            &mut self.plane_buffers[0],
        )?;
        pack_plane_into(
            (*frame).data[1],
            (*frame).linesize[1],
            width.div_ceil(2),
            height.div_ceil(2),
            &mut self.plane_buffers[1],
        )?;
        pack_plane_into(
            (*frame).data[2],
            (*frame).linesize[2],
            width.div_ceil(2),
            height.div_ceil(2),
            &mut self.plane_buffers[2],
        )?;
        self.ensure_plane_textures(width, height);
        self.ensure_output_texture(width, height);
        let stride = aligned_stride(width);
        self.ensure_readback_buffer(slot_index, stride, height);
        let (y_tex, u_tex, v_tex) = self.plane_textures.as_ref().unwrap();
        self.upload_plane(y_tex, &self.plane_buffers[0], width, height);
        self.upload_plane(
            u_tex,
            &self.plane_buffers[1],
            width.div_ceil(2),
            height.div_ceil(2),
        );
        self.upload_plane(
            v_tex,
            &self.plane_buffers[2],
            width.div_ceil(2),
            height.div_ceil(2),
        );
        self.last_upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;

        {
            let output = &self.output.as_ref().unwrap().0;
            let readback = self.readbacks[slot_index].buffer.as_ref().unwrap();
            let output_view = output.create_view(&Default::default());
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("veyra-video-encoder"),
                });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("veyra-video-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
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
            pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: output,
                    mip_level: 0,
                    origin: Default::default(),
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit(Some(encoder.finish()));
        }
        self.readbacks[slot_index].state = SlotState::Mapping {
            width,
            height,
            stride,
            timestamp,
            generation: self.generation,
            submitted_at: Instant::now(),
        };
        let tx = self.map_tx.clone();
        self.readbacks[slot_index]
            .buffer
            .as_ref()
            .unwrap()
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(MapResult {
                    slot: slot_index,
                    result,
                });
            });
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        self.log_diagnostics();
        Ok(ready)
    }

    pub(crate) fn poll_completed(&mut self) -> Result<Option<VideoFrame>, String> {
        self.device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| e.to_string())?;
        let mut newest = None;
        while let Ok(event) = self.map_rx.try_recv() {
            let slot = &mut self.readbacks[event.slot];
            let SlotState::Mapping {
                width,
                height,
                stride,
                timestamp,
                generation,
                submitted_at,
            } = std::mem::replace(&mut slot.state, SlotState::Idle)
            else {
                continue;
            };
            if event.result.is_err() {
                eprintln!(
                    "[video] Vulkan readback mapping failed for slot {}",
                    event.slot
                );
                continue;
            }
            let buffer = slot.buffer.as_ref().expect("mapped slot owns a buffer");
            let mapped = buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("GPU mapped range failed: {e:?}"))?;
            let mut rgba = vec![0; (width * height * 4) as usize];
            for row in 0..height as usize {
                rgba[row * width as usize * 4..(row + 1) * width as usize * 4].copy_from_slice(
                    &mapped[row * stride as usize..row * stride as usize + width as usize * 4],
                );
            }
            drop(mapped);
            buffer.unmap();
            if generation != self.generation {
                continue;
            }
            self.last_readback_latency_ms = submitted_at.elapsed().as_secs_f64() * 1000.0;
            self.completed_frames = self.completed_frames.saturating_add(1);
            newest = Some(VideoFrame {
                rgba,
                width,
                height,
                timestamp,
            });
        }
        Ok(newest)
    }

    /// End-of-stream only; normal playback never waits for GPU completion.
    pub(crate) fn finish_pending(&mut self) -> Result<Option<VideoFrame>, String> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|e| e.to_string())?;
        self.poll_completed()
    }

    pub(crate) fn reset_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn idle_slot(&self) -> Option<usize> {
        self.readbacks
            .iter()
            .position(|slot| matches!(slot.state, SlotState::Idle))
    }

    fn ensure_readback_buffer(&mut self, slot_index: usize, stride: u32, height: u32) {
        let required = (stride * height) as u64;
        let slot = &mut self.readbacks[slot_index];
        if slot
            .buffer
            .as_ref()
            .is_some_and(|buffer| buffer.size() >= required)
        {
            return;
        }
        self.resource_creations = self.resource_creations.saturating_add(1);
        slot.buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("veyra-video-readback"),
            size: required,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
    }

    fn ensure_output_texture(&mut self, width: u32, height: u32) {
        if self
            .output
            .as_ref()
            .is_some_and(|(_, w, h)| *w == width && *h == height)
        {
            return;
        }
        self.resource_creations = self.resource_creations.saturating_add(1);
        self.output = Some((
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("veyra-video-output"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }),
            width,
            height,
        ));
    }

    fn ensure_plane_textures(&mut self, width: u32, height: u32) {
        if self.plane_textures.is_some() && self.plane_dimensions == (width, height) {
            return;
        }
        self.plane_textures = Some((
            self.create_plane_texture("y", width, height),
            self.create_plane_texture("u", width.div_ceil(2), height.div_ceil(2)),
            self.create_plane_texture("v", width.div_ceil(2), height.div_ceil(2)),
        ));
        self.plane_dimensions = (width, height);
        let (y, u, v) = self.plane_textures.as_ref().unwrap();
        self.plane_views = Some((
            y.create_view(&Default::default()),
            u.create_view(&Default::default()),
            v.create_view(&Default::default()),
        ));
        let (y, u, v) = self.plane_views.as_ref().unwrap();
        self.bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("veyra-yuv-bind-group"),
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
        }));
    }

    fn create_plane_texture(&mut self, label: &str, width: u32, height: u32) -> wgpu::Texture {
        self.resource_creations = self.resource_creations.saturating_add(1);
        self.device.create_texture(&wgpu::TextureDescriptor {
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

    fn upload_plane(&self, texture: &wgpu::Texture, bytes: &[u8], width: u32, height: u32) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: Default::default(),
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    fn log_diagnostics(&self) {
        if self.submitted_frames > 0 && self.submitted_frames % 300 == 0 {
            let occupied = self.readbacks.len()
                - self
                    .readbacks
                    .iter()
                    .filter(|slot| matches!(slot.state, SlotState::Idle))
                    .count();
            eprintln!(
                "[video] Vulkan async: submitted={} completed={} skipped={} occupied_slots={}/{} upload_ms={:.2} readback_latency_ms={:.2} resource_creations={}",
                self.submitted_frames,
                self.completed_frames,
                self.skipped_readbacks,
                occupied,
                READBACK_SLOT_COUNT,
                self.last_upload_ms,
                self.last_readback_latency_ms,
                self.resource_creations,
            );
        }
    }
}

fn aligned_stride(width: u32) -> u32 {
    (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
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
unsafe fn pack_plane_into(
    data: *const u8,
    stride: i32,
    width: u32,
    height: u32,
    output: &mut Vec<u8>,
) -> Result<(), String> {
    if data.is_null() || stride < width as i32 {
        return Err("invalid FFmpeg plane stride".into());
    }
    output.resize((width * height) as usize, 0);
    for row in 0..height as usize {
        std::ptr::copy_nonoverlapping(
            data.add(row * stride as usize),
            output.as_mut_ptr().add(row * width as usize),
            width as usize,
        );
    }
    Ok(())
}
