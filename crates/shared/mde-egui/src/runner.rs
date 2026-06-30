//! The shared **eframe Wayland-client runner** (governance §5, lock 5).
//!
//! Every E12 surface is an independent `eframe` Wayland client on a *pure*
//! compositor — no UI is embedded in the compositor itself. This runner is the
//! one place that knows how to stand such a client up, so surfaces never repeat
//! the eframe/winit/wgpu boilerplate or drift on render-backend choice.

use eframe::{App, CreationContext, NativeOptions, Renderer};

use crate::style::Style;

/// Run an MCNF egui surface as a Wayland client.
///
/// - sets the Wayland **`app_id`** (the compositor groups windows + maps icons by
///   it — pass a reverse-DNS id like `org.magicmesh.Workbench`),
/// - selects the **wgpu** renderer on winit's **Wayland** backend,
/// - installs the shared [`Style`] on the egui context before the app builds,
/// - then hands control to `eframe`.
///
/// `build` constructs the surface's [`App`] from the eframe [`CreationContext`]
/// (use it to read storage, load fonts, etc.). Accessibility (accesskit) is
/// deferred (E12 lock 11) and so is not wired here.
///
/// Returns eframe's run result; the call blocks until the window closes.
///
/// # Errors
/// Propagates any `eframe` startup/run failure — e.g. no Wayland display
/// available, or wgpu adapter/surface initialization failing on the host.
pub fn run_client<A, F>(app_id: &str, title: &str, build: F) -> eframe::Result<()>
where
    A: App + 'static,
    F: FnOnce(&CreationContext<'_>) -> A + 'static,
{
    let options = NativeOptions {
        renderer: Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_app_id(app_id)
            .with_title(title)
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        app_id,
        options,
        Box::new(move |cc| {
            Style::install(&cc.egui_ctx);
            Ok(Box::new(build(cc)))
        }),
    )
}
