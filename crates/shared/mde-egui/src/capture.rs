//! Headless offscreen **PNG screenshot rasterizer** for egui surfaces (§4 polish
//! tool). Completes the capability that [`crate::style::Style::CAPTURE_CLEAR`]
//! already anticipates: render an arbitrary egui UI to an actual PNG with no
//! on-screen window, so `/polish` (and CI, and live-verify) can capture a surface
//! and inspect the pixels.
//!
//! The render path is **offscreen wgpu**: an adapter (the farm build VMs ship the
//! `lavapipe` software Vulkan ICD, so this works headlessly), an offscreen
//! `Rgba8UnormSrgb` texture, egui tessellation painted through
//! [`egui_wgpu::Renderer`], a texture→buffer copy, a mapped readback, and a PNG
//! encode via the in-tree `image` crate. `eframe` re-exports both `wgpu` and
//! `egui_wgpu` (the `wgpu` feature is on), so this adds no new graphics deps.
//!
//! The frame is cleared to [`Style::CAPTURE_CLEAR`](crate::style::Style::CAPTURE_CLEAR)
//! — a near-black held strictly darker than every real surface tone — so a
//! genuinely blank capture is obvious in the PNG itself, not only via a pixel scan.

use eframe::{egui_wgpu, wgpu};

use crate::style::Style;

/// Why a headless capture could not be produced.
#[derive(Debug)]
pub enum CaptureError {
    /// No usable wgpu adapter (not even a software one). On a headless host this
    /// means no Vulkan/GL ICD is installed (e.g. no `mesa-vulkan-drivers`/lavapipe).
    NoAdapter,
    /// The device could not be created from the adapter.
    Device(wgpu::RequestDeviceError),
    /// Reading the rendered pixels back from the GPU buffer failed.
    Readback,
    /// Encoding the RGBA pixels to PNG failed.
    Encode(image::ImageError),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdapter => write!(f, "no wgpu adapter (no software render ICD available)"),
            Self::Device(e) => write!(f, "wgpu device: {e}"),
            Self::Readback => write!(f, "buffer readback failed"),
            Self::Encode(e) => write!(f, "png encode: {e}"),
        }
    }
}
impl std::error::Error for CaptureError {}

/// wgpu requires buffer copy rows to be a multiple of this many bytes.
const ROW_ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

fn align_up(value: u32, align: u32) -> u32 {
    value.div_ceil(align) * align
}

/// Render `run_ui` to a PNG of `size` logical points at `pixels_per_point`.
///
/// Two egui frames are run: the first lets layout settle (sizes that depend on
/// prior-frame geometry), the second is captured — mirroring the settle pattern
/// the shell's shape-capture harness uses.
pub fn capture_ui_png(
    size: egui::Vec2,
    pixels_per_point: f32,
    mut run_ui: impl FnMut(&egui::Context),
) -> Result<Vec<u8>, CaptureError> {
    let width = (size.x * pixels_per_point).round().max(1.0) as u32;
    let height = (size.y * pixels_per_point).round().max(1.0) as u32;

    // ---- wgpu: instance -> adapter (accept a software adapter) -> device ----
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .ok_or(CaptureError::NoAdapter)?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("mde-egui-capture-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .map_err(CaptureError::Device)?;

    // ---- offscreen render target + readback buffer (256-aligned rows) ----
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mde-egui-capture-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let padded_bytes_per_row = align_up(width * 4, ROW_ALIGN);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mde-egui-capture-readback"),
        size: (padded_bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // ---- egui: settle frame, then the captured frame ----
    let ctx = egui::Context::default();
    let raw_input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    ctx.set_pixels_per_point(pixels_per_point);
    // The font/texture atlas is generated as a `textures_delta` on the FIRST frame
    // it is needed — so both frames' deltas must be applied, not just the last's
    // (egui paints even solid fills through the atlas's white texel).
    let settle = ctx.run(raw_input(), &mut run_ui);
    let full_output = ctx.run(raw_input(), &mut run_ui);
    let paint_jobs = ctx.tessellate(full_output.shapes, pixels_per_point);

    // ---- egui_wgpu renderer: upload textures + buffers, paint ----
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    let screen_descriptor = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point,
    };
    for (id, delta) in settle
        .textures_delta
        .set
        .iter()
        .chain(&full_output.textures_delta.set)
    {
        renderer.update_texture(&device, &queue, *id, delta);
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mde-egui-capture"),
    });
    let cmd_bufs = renderer.update_buffers(
        &device,
        &queue,
        &mut encoder,
        &paint_jobs,
        &screen_descriptor,
    );

    let clear = Style::CAPTURE_CLEAR;
    let srgb_to_linear = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mde-egui-capture-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear(clear.r()),
                            g: srgb_to_linear(clear.g()),
                            b: srgb_to_linear(clear.b()),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            })
            .forget_lifetime();
        renderer.render(&mut pass, &paint_jobs, &screen_descriptor);
    }
    for id in &full_output.textures_delta.free {
        renderer.free_texture(id);
    }

    // ---- copy target -> readback buffer ----
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(
        cmd_bufs
            .into_iter()
            .chain(std::iter::once(encoder.finish())),
    );

    // ---- map + read back, un-padding each row ----
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(
        wgpu::MapMode::Read,
        move |r: Result<(), wgpu::BufferAsyncError>| {
            let _ = tx.send(r.is_ok());
        },
    );
    let _ = device.poll(wgpu::Maintain::Wait);
    if !rx.recv().unwrap_or(false) {
        return Err(CaptureError::Readback);
    }
    let data = slice.get_mapped_range();
    let unpadded_bytes_per_row = (width * 4) as usize;
    let mut rgba = Vec::with_capacity(unpadded_bytes_per_row * height as usize);
    for row in 0..height as usize {
        let start = row * padded_bytes_per_row as usize;
        rgba.extend_from_slice(&data[start..start + unpadded_bytes_per_row]);
    }
    drop(data);
    readback.unmap();

    // ---- encode PNG ----
    let img: image::RgbaImage =
        image::ImageBuffer::from_raw(width, height, rgba).ok_or(CaptureError::Readback)?;
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(CaptureError::Encode)?;
    Ok(png.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rasterizer produces a real PNG of the right size whose content is not
    /// the blank `CAPTURE_CLEAR` sentinel — i.e. the themed panel actually drew.
    #[test]
    fn capture_renders_a_themed_panel_to_a_non_blank_png() {
        let size = egui::vec2(200.0, 120.0);
        let ppp = 1.0;
        let png = match capture_ui_png(size, ppp, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(Style::SURFACE))
                .show(ctx, |ui| {
                    ui.colored_label(Style::TEXT, "mde-egui capture");
                });
        }) {
            Ok(png) => png,
            // A build host with no render ICD at all is a valid environment miss,
            // not a code failure — surface it clearly rather than a bare panic.
            Err(CaptureError::NoAdapter) => {
                eprintln!("SKIP: no wgpu adapter on this host (no software render ICD)");
                return;
            }
            Err(e) => panic!("capture failed: {e}"),
        };

        let decoded = image::load_from_memory(&png)
            .expect("PNG decodes")
            .to_rgba8();
        assert_eq!(decoded.width(), 200);
        assert_eq!(decoded.height(), 120);

        // Not a uniformly-CAPTURE_CLEAR (blank) image: some pixel differs from the
        // clear sentinel, proving the themed panel + label rendered.
        let clear = [
            Style::CAPTURE_CLEAR.r(),
            Style::CAPTURE_CLEAR.g(),
            Style::CAPTURE_CLEAR.b(),
        ];
        let drew_something = decoded
            .pixels()
            .any(|p| [p[0], p[1], p[2]] != clear && [p[0], p[1], p[2]] != [0, 0, 0]);
        assert!(
            drew_something,
            "capture is blank — the panel did not render"
        );
    }
}
