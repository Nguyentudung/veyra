use super::{
    ColorInfo, Format, GpuContext, GpuPlanarVideoFrame, Matrix, NativeSurface, SurfaceRenderer,
    Transfer, nv12_reference,
};
use crate::{
    render::{Gpu, OFFSCREEN_FORMAT},
    wgpu,
};
use std::sync::Arc;

fn srgb(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn r8_texture(
    device: &wgpu::Device,
    label: &'static str,
    width: u32,
    height: u32,
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
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn upload_r8(queue: &wgpu::Queue, texture: &wgpu::Texture, bytes: &[u8], width: u32, height: u32) {
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

#[test]
#[ignore = "requires a functioning headless GPU driver"]
fn external_gpu_yuv420p_uses_the_video_compositor() {
    let gpu = Gpu::headless_wgpu();
    let (width, height) = (8, 8);
    let y = r8_texture(&gpu.device, "external yuv420p y", width, height);
    let u = r8_texture(&gpu.device, "external yuv420p u", width / 2, height / 2);
    let v = r8_texture(&gpu.device, "external yuv420p v", width / 2, height / 2);
    let codes = [81, 90, 240];
    upload_r8(
        &gpu.queue,
        &y,
        &vec![codes[0]; (width * height) as usize],
        width,
        height,
    );
    upload_r8(
        &gpu.queue,
        &u,
        &vec![codes[1]; (width * height / 4) as usize],
        width / 2,
        height / 2,
    );
    upload_r8(
        &gpu.queue,
        &v,
        &vec![codes[2]; (width * height / 4) as usize],
        width / 2,
        height / 2,
    );

    let surface = NativeSurface::new();
    let context = GpuContext::new(&gpu.device, &gpu.queue, 1);
    surface.publish_gpu_context(&context);
    let color = ColorInfo {
        matrix: Matrix::Bt709,
        transfer: Transfer::Srgb,
    };
    surface
        .push_gpu_planar_video(
            GpuPlanarVideoFrame {
                planes: [
                    y.create_view(&Default::default()),
                    u.create_view(&Default::default()),
                    v.create_view(&Default::default()),
                ],
                size_px: [width, height],
                format: Format::Nv12Video,
                color,
                generation: 1,
                pool_id: 1,
                slot: 0,
                lease: Arc::new(()),
            },
            1,
        )
        .unwrap();
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let pixels = super::gpu_tests::render(
        &gpu,
        &mut renderer,
        &[super::gpu_tests::draw(
            &surface,
            [0.0, 0.0, width as f32, height as f32],
        )],
        (width, height),
    );
    let expected = nv12_reference(codes[0], codes[1], codes[2], Format::Nv12Video, color).map(srgb);
    for pixel in pixels.chunks_exact(4) {
        for channel in 0..3 {
            assert!(
                pixel[channel].abs_diff(expected[channel]) <= 1,
                "{pixel:?} != {expected:?}"
            );
        }
        assert_eq!(pixel[3], 255);
    }
}
