//! The shell **dock** — the full-width surface launcher **taskbar** pinned to
//! the bottom edge: the shell's ONE bar (NAVBAR-W10-2, superseding NAVBAR-1's
//! labelled/grouped bar and the E12-3b left rail before it).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The dock
//! is that shell nav: a pixel-per-Win10 taskbar (lock W3 — a 40px bar, 24px app
//! glyphs) that selects which surface fills the shell body — the mesh-control
//! [`Workbench`](Surface::Workbench), the live Mesh Map, the VM surfaces
//! (Instances / Desktop), the embedded app surfaces (Music / Media / Files /
//! Voice / Browser / Terminal / Editor), the unified [`Chat`](Surface::Chat)
//! surface, and the System / Storage / About screens. One surface shows at a
//! time; the Workbench is always one click away.
//!
//! The bar is **one flat icon-only row** (locks W4/W5/W6): every surface as a
//! 24px brand glyph in [`Surface::ALL`] order from the left — no labels, no
//! group dividers, no right-packed system group (the tray owns the right). The
//! active cell wears a **bottom-edge accent underline** + the subtle selection
//! wash; hover is a fill only — no tooltips anywhere. After a flexible gap the
//! bar ends in the right-justified status **tray** + clock (`tray.rs`).
//!
//! The bar is pure chrome: it reads + writes the active [`Surface`] and draws
//! through the shared [`Style`] (§4). It never builds or drives a surface — the
//! shell owns each surface's app and its per-frame pump.

use mde_egui::egui::{self, TextureHandle, TextureOptions};
use mde_egui::Style;
use mde_theme::brand::icons::{icon_image, IconId};

use crate::tray::{self, TrayInputs, TrayState};

/// Which surface fills the shell body.
///
/// [`Workbench`](Self::Workbench) is the default: the shell opens on the
/// mesh-control Workbench — the other surfaces are the panels beside it.
/// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form for
/// crate-visible items in a private module — like `TASKBAR_H` below.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Surface {
    /// The five-plane mesh-control Workbench (This Node → Fleet).
    #[default]
    Workbench,
    /// The live **Mesh Map** — the egui reincarnation of MESHMAP (`mde-mesh-view`):
    /// a procedural canvas of the current mesh (nodes by role + health, the elected
    /// leader, and the links between them), folded from the same world-readable
    /// mesh-status snapshot the Workbench planes read. An all-green onboard
    /// self-test auto-opens it (OW-10).
    MeshView,
    /// The VDI **Desktop** surface — a brokered VM desktop rendered egui-native
    /// (`mde-vdi-rdp` / `mde-vdi-vnc`), the point of E12 "Quasar".
    Desktop,
    /// The **Instances** surface — this workstation's local cloud-hypervisor VMs
    /// (`mde-kvm`): the create / boot / shutdown lifecycle broker (E12-7).
    Instances,
    /// The embedded Music surface (`mde-music-egui`).
    Music,
    /// The embedded Media surface (`mde-media-egui`) — the full media player
    /// (Sources / Library / Player / Queue) over the real `mde_media_core`
    /// backend (MEDIA-18).
    Media,
    /// The embedded Files surface (`mde-files-egui`).
    Files,
    /// The embedded Voice / SIP surface (`mde-voice-egui`).
    Voice,
    /// The Browser surface — the sandboxed Servo browser (`mde-web-preview`)
    /// rendered egui-native over the BOOKMARKS-6 IPC + shm texture bridge.
    Browser,
    /// The embedded Terminal surface (`mde-term-egui`) — the full Terminator-class
    /// terminal (tabs / splits / broadcast / a shell on any mesh peer, TERM-4/5/8)
    /// over a real local PTY, mounted as an in-shell panel (TERM-16).
    Terminal,
    /// The embedded Editor surface (`mde-editor-egui`) — the native Zed-style code
    /// editor (EDITOR epic). EDITOR-1 mounts the scaffold: the editor chrome + the
    /// honest "No file open" empty state (§7); the rope buffer + text widget +
    /// tree-sitter highlighting + tabs/splits land in EDITOR-2 onward.
    Editor,
    /// The Chat surface — the ONE unified notification interface (NOTIFY-CHAT):
    /// every mesh host is a contact, and its alerts + clipboard copies are its
    /// messages, over the `state/chat/roster` + `state/chat/conversation/<key>`
    /// worker read-model. Subsumes the retired standalone Notifications +
    /// Clipboard surfaces (NOTIFY-CHAT-6 cutover).
    Chat,
    /// The System surface — this seat's host controls (audio mixer, Bluetooth,
    /// displays, power & battery, backlight, hotkeys), folded from `mde-seat`
    /// (E12-15). Owns ALL host-control interaction (lock 3); the taskbar tray
    /// keeps only read-only status icons.
    System,
    /// The Storage surface — GParted-authentic disk/partition management (E12-21),
    /// folded from `state/storage/<node>` and driven back via `action/storage/<node>`.
    /// Segment bars + partition tables + a typed-armed pending-op queue, for this
    /// node and any mesh peer; the `mackesd` storage worker owns the walls + executor.
    Storage,
    /// The About surface — the canonical "about this platform" screen (QBRAND-6,
    /// placement lock #13): the official `MDE-QUAZAR-MAIN.png` lockup, the product
    /// name + tagline, the full build identity (version · git hash · date · channel),
    /// and the shipped legal docs + source URL. A pure renderer of the
    /// [`mde_theme::brand`] constants (`crate::about`).
    About,
}

// This nav enum spells its variants `Surface::Music` rather than `Self::Music` on
// purpose: the explicit type keeps the `ALL` table and the glyph map scannable
// side by side (a launcher reads clearer than a wall of `Self::`). Opt the block
// out of `clippy::use_self` rather than thread `Self::` through every arm.
#[allow(clippy::use_self)]
impl Surface {
    /// The dock entries in nav order — the one ordering authority (lock W4: the
    /// bar renders exactly this, flat, from the left): the Workbench
    /// (mesh-control home) first, then the live Mesh Map, the local VM Instances
    /// broker + the brokered Desktop, the app surfaces, the unified Chat surface
    /// (the ONE notification interface), and the System / Storage / About
    /// screens as ordinary row icons at the end (the tray owns the right edge).
    pub(crate) const ALL: [Surface; 15] = [
        Surface::Workbench,
        Surface::MeshView,
        Surface::Instances,
        Surface::Desktop,
        Surface::Music,
        Surface::Media,
        Surface::Files,
        Surface::Voice,
        Surface::Browser,
        Surface::Terminal,
        Surface::Editor,
        Surface::Chat,
        Surface::System,
        Surface::Storage,
        Surface::About,
    ];

    /// The [`brand::icons`](mde_theme::brand::icons) glyph this surface draws in
    /// the bar (QBRAND-7). A 1:1 map by name onto the Quasar brand set — every
    /// dock surface has a dedicated line-art glyph and `MeshView` folds onto the
    /// topology-map glyph. The dock never re-draws a glyph; it tints this one
    /// through the shared loader (§6).
    pub(crate) const fn icon_id(self) -> IconId {
        match self {
            Surface::Workbench => IconId::Workbench,
            Surface::MeshView => IconId::MeshView,
            Surface::Instances => IconId::Instances,
            Surface::Desktop => IconId::Desktop,
            Surface::Music => IconId::Music,
            Surface::Media => IconId::Media,
            Surface::Files => IconId::Files,
            Surface::Voice => IconId::Voice,
            Surface::Browser => IconId::Browser,
            Surface::Terminal => IconId::Terminal,
            Surface::Editor => IconId::Editor,
            Surface::Chat => IconId::Chat,
            Surface::System => IconId::System,
            Surface::Storage => IconId::Storage,
            // The About surface wears the product **mark** — the mesh-node
            // constellation glyph that IS the platform identity — fitting the
            // "about this platform" screen and distinct from every surface glyph.
            Surface::About => IconId::Mark,
        }
    }
}

/// The taskbar height in logical points — the pixel-per-Win10 40px bar (lock
/// W3), on the 8px grid (`SP_XL + SP_S`); `main.rs` mounts the bottom panel at
/// exactly this height. (`pub`, not `pub(crate)`, is the
/// `clippy::redundant_pub_crate` form for a crate-visible item in a private
/// module.)
pub const TASKBAR_H: f32 = Style::SP_XL + Style::SP_S;

/// The fixed width of one icon-only glyph cell (lock W4 — no caption, so the
/// cell shrinks to suit the 24px glyph): `SP_XL + SP_M` on the 8px grid.
/// Private: only the bar's own layout + tests read it.
const CELL_W: f32 = Style::SP_XL + Style::SP_M;

/// The app glyph edge in logical points — the Win10 24px taskbar icon (lock
/// W3, `SP_L`). Rasterized crisp at the physical pixel size by `icon_texture`.
const ICON_LOGICAL: f32 = Style::SP_L;

/// The active cell's **bottom-edge accent underline** (lock W5 — the Win10
/// running/active idiom, replacing the old top strip): a full-width strip,
/// `SP_XS` tall, hugging the cell's bottom edge. Pure geometry, unit-tested.
fn underline(cell: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(cell.left(), cell.bottom() - Style::SP_XS),
        egui::vec2(cell.width(), Style::SP_XS),
    )
}

/// Render the surface launcher as the full-width bottom **taskbar** into `ui`,
/// selecting the active [`Surface`] and rendering the right-justified status
/// tray (NAVBAR-W10-2). A click on a cell makes that surface active; the
/// active one reads as selected (bottom accent underline + selection wash).
///
/// The layout is the Win10 anatomy: one flat icon-only row in [`Surface::ALL`]
/// order from the left (no labels, no dividers — locks W4/W6), a flexible gap,
/// then the tray (chevron · status icons · clock) against the right edge.
/// Returns `true` when any click routed this frame (a cell or a tray icon) so
/// the shell can surface the body behind a session.
pub fn taskbar(
    ui: &mut egui::Ui,
    active: &mut Surface,
    tray: &mut TrayState,
    inputs: &TrayInputs<'_>,
) -> bool {
    // A hairline top divider on the seam between the surface body above and the
    // bar, drawn from the installed BORDER stroke (a Style token, not a raw
    // colour/width — §4). The flat SURFACE fill is the panel frame (`main.rs`).
    let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .hline(ui.max_rect().x_range(), ui.max_rect().top(), hairline);

    let mut clicked = false;
    ui.horizontal(|ui| {
        // Cells breathe with a small horizontal gap; each cell still carries its
        // own internal padding around the centred glyph.
        ui.spacing_mut().item_spacing = egui::vec2(Style::SP_XS, 0.0);

        // Lock W4 — one flat icon row, ALL order, from the left. System /
        // Storage / About are ordinary row icons (the tray owns the right).
        for surface in Surface::ALL {
            if cell(ui, surface, active) {
                clicked = true;
            }
        }

        // Lock W2 — flexible space, then the right-justified tray + clock: a
        // right-to-left sub-layout consumes the remaining width.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            if tray::tray(ui, tray, active, inputs) {
                clicked = true;
            }
        });
    });
    clicked
}

/// One taskbar entry — an icon-only glyph cell (locks W4/W5/W6): the 24px brand
/// glyph centred in the cell, the accent bottom underline + selection wash when
/// active, a hover fill only (NO tooltip), and a click that selects the
/// surface (returned so the shell can surface the body).
fn cell(ui: &mut egui::Ui, surface: Surface, active: &mut Surface) -> bool {
    let selected = *active == surface;
    // Fill the full bar height so the whole column is clickable.
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(CELL_W, ui.available_height()),
        egui::Sense::click(),
    );
    let hovered = response.hovered();

    // A painter clone so `egui::Image::paint_at` can still borrow `ui` (splash.rs).
    let painter = ui.painter().clone();

    // Cell background: the selected cell wears the accent selection wash, a
    // hovered one the raised SURFACE_HI — both Style tokens (§4); hover is the
    // fill alone (lock W6 — no tooltip).
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }

    // Active mark (lock W5): the accent underline along the cell's bottom edge.
    if selected {
        painter.rect_filled(underline(rect), egui::CornerRadius::ZERO, Style::ACCENT);
    }

    // Two-tone tint: the active glyph reads solid in the brand ACCENT, a hovered
    // one brightens to full TEXT, the rest sit dim at TEXT_DIM. The brand SVG
    // set is a single `currentColor` variant (no separate outline artwork), so
    // "filled vs outline" is approximated by tint intensity — every value a
    // Style token, never a raw colour (§4).
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };

    // The glyph, centred in the cell (lock W4 — no caption beneath it). A glyph
    // load failure fails soft to the bare cell (§7).
    if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), ICON_LOGICAL, tint) {
        let icon_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(ICON_LOGICAL, ICON_LOGICAL));
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }

    if response.clicked() {
        *active = surface;
        return true;
    }
    false
}

/// Rasterize + upload a brand glyph, cached in egui memory so a given
/// `(glyph, physical-size, tint)` triple is rasterized through `resvg` **once**
/// and then shared as a cheap ref-counted [`TextureHandle`] — never re-rasterized
/// per frame (the backdrop.rs lock-7 pattern). A failed rasterize caches `None`,
/// so a broken asset fails soft (§7) without retrying every frame. Shared with
/// the tray (`tray.rs`), which rasters the 16px tray set through the same cache.
///
/// The glyph is rasterized at the physical pixel size (`logical × ppp`) and drawn
/// back at the logical size, so it stays DPI-crisp at any `HiDPI` scale — the
/// loader honours the exact requested px.
#[allow(
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 → u32
    clippy::cast_sign_loss            // size_px ≥ 1.0 by the .max(1.0) clamp
)]
pub fn icon_texture(
    ctx: &egui::Context,
    id: IconId,
    logical_px: f32,
    tint: egui::Color32,
) -> Option<TextureHandle> {
    let size_px = (logical_px * ctx.pixels_per_point()).round().max(1.0) as u32;
    let tint = tint.to_array();
    let key = egui::Id::new(("qbrand7-dock-icon", id.name(), size_px, tint));

    // Fast path: the resolved texture (or a cached `None` from an earlier failed
    // decode) is already in egui memory — a cheap ref-counted clone.
    if let Some(cached) = ctx.data_mut(|d| d.get_temp::<Option<TextureHandle>>(key)) {
        return cached;
    }
    // Slow path (first paint of this variant): rasterize + upload OUTSIDE the
    // `data_mut` lock. `load_texture` read-locks the context that `data_mut`
    // write-locks, so uploading inside would deadlock the frame (backdrop.rs) —
    // resolve first, then cache the handle.
    let texture = icon_image(id, size_px, tint).ok().map(|img| {
        let color = egui::ColorImage::from_rgba_unmultiplied(img.size_usize(), &img.rgba);
        ctx.load_texture(id.name(), color, TextureOptions::LINEAR)
    });
    ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
    texture
}

#[cfg(test)]
mod tests {
    use super::{icon_texture, taskbar, underline, Surface, CELL_W, ICON_LOGICAL, TASKBAR_H};
    use crate::chrome::MeshSummary;
    use crate::tray::{TrayInputs, TrayState};
    use mde_egui::egui;
    use mde_egui::Style;
    use mde_theme::brand::icons::{icon_image, IconId};

    #[test]
    fn the_dock_lists_the_workbench_vm_surfaces_app_surfaces_and_info_surfaces() {
        // Fifteen entries: Workbench first, the live Mesh Map (OW-10, `mde-mesh-view`),
        // two VM surfaces (Instances / Desktop), the app surfaces (Music / Media — the
        // full media player, MEDIA-18 / Files / Voice / Browser — the sandboxed Servo
        // browser, BOOKMARKS-6 / Terminal — the Terminator-class terminal over a real
        // PTY, TERM-16 / Editor — the native Zed-style code editor, EDITOR-1), the
        // unified Chat surface (the ONE notification interface — the standalone
        // Notifications + Clipboard surfaces are retired, NOTIFY-CHAT-6), the
        // host-controls System surface, the Storage surface (GParted-authentic disk
        // mgmt, E12-21), and the About surface (the platform-identity screen, QBRAND-6).
        assert_eq!(Surface::ALL.len(), 15);
        assert_eq!(Surface::ALL[0], Surface::Workbench);
        for s in [
            Surface::MeshView,
            Surface::Instances,
            Surface::Desktop,
            Surface::Music,
            Surface::Media,
            Surface::Files,
            Surface::Voice,
            Surface::Browser,
            Surface::Terminal,
            Surface::Editor,
            Surface::Chat,
            Surface::System,
            Surface::Storage,
            Surface::About,
        ] {
            assert!(Surface::ALL.contains(&s), "{s:?} missing from the dock");
        }
    }

    #[test]
    fn the_shell_opens_on_the_workbench_surface() {
        assert_eq!(Surface::default(), Surface::Workbench);
    }

    // --- NAVBAR-W10-2: the pixel-per-Win10 metrics + active mark ------------------

    #[test]
    fn the_bar_wears_the_win10_metrics() {
        // Lock W3 @100%: a 40px bar, 24px app glyphs, and the icon-only cell
        // shrunk to 48px — all on the 8px grid, straight from Style tokens.
        assert!((TASKBAR_H - 40.0).abs() < f32::EPSILON, "bar height");
        assert!((ICON_LOGICAL - 24.0).abs() < f32::EPSILON, "app glyph edge");
        assert!((CELL_W - 48.0).abs() < f32::EPSILON, "icon-only cell width");
    }

    #[test]
    fn the_active_underline_hugs_the_cells_bottom_edge() {
        // Lock W5 — the accent mark moved from the cell's top strip to the
        // Win10 bottom-edge underline: full cell width, SP_XS tall, flush with
        // the bottom edge.
        let cell = egui::Rect::from_min_size(
            egui::pos2(96.0, 600.0 - TASKBAR_H),
            egui::vec2(CELL_W, TASKBAR_H),
        );
        let strip = underline(cell);
        assert!(
            (strip.bottom() - cell.bottom()).abs() < f32::EPSILON,
            "flush bottom"
        );
        assert!(
            (strip.height() - Style::SP_XS).abs() < f32::EPSILON,
            "strip height"
        );
        assert!(
            (strip.width() - cell.width()).abs() < f32::EPSILON,
            "full width"
        );
        assert!(
            (strip.left() - cell.left()).abs() < f32::EPSILON,
            "flush left"
        );
        assert!(
            strip.top() > cell.center().y,
            "an underline, not a top strip"
        );
    }

    // --- QBRAND-7: every dock surface renders a brand::icons glyph ----------------

    #[test]
    fn every_surface_maps_to_a_named_brand_glyph() {
        // The map is 1:1 by name (Workbench→Workbench … MeshView→MeshView), and no
        // surface folds onto the blank text wordmark.
        let cases = [
            (Surface::Workbench, IconId::Workbench),
            (Surface::MeshView, IconId::MeshView),
            (Surface::Instances, IconId::Instances),
            (Surface::Desktop, IconId::Desktop),
            (Surface::Music, IconId::Music),
            (Surface::Media, IconId::Media),
            (Surface::Files, IconId::Files),
            (Surface::Voice, IconId::Voice),
            (Surface::Browser, IconId::Browser),
            (Surface::Terminal, IconId::Terminal),
            (Surface::Editor, IconId::Editor),
            (Surface::Chat, IconId::Chat),
            (Surface::System, IconId::System),
            (Surface::Storage, IconId::Storage),
            (Surface::About, IconId::Mark),
        ];
        assert_eq!(cases.len(), Surface::ALL.len(), "a surface is unmapped");
        for (surface, id) in cases {
            assert_eq!(surface.icon_id(), id, "{surface:?} → wrong glyph");
            assert_ne!(
                id,
                IconId::Wordmark,
                "{surface:?} maps to the blank wordmark"
            );
        }
        // The map is injective — 15 surfaces, 15 distinct glyph names.
        let mut names: Vec<&str> = Surface::ALL.iter().map(|s| s.icon_id().name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Surface::ALL.len(), "surface→glyph map not 1:1");
    }

    #[test]
    fn every_surface_glyph_rasterizes_nonempty() {
        // Each surface's glyph resolves to real ink through the shared loader,
        // tinted by a Style token (no raw hex) — so the bar never draws an empty
        // square.
        let tint = Style::TEXT_DIM.to_array();
        for surface in Surface::ALL {
            let img = icon_image(surface.icon_id(), 32, tint).expect("surface glyph rasterizes");
            let inked = img.rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
            assert!(inked > 0, "{surface:?} glyph rasterized empty");
        }
    }

    // --- the bar mounts, renders icon-only, and switches surface on a click -------

    /// Mount the real bottom bar (with a default tray over an unseen mesh) for
    /// one headless frame and return the frame output.
    fn run_taskbar(
        ctx: &egui::Context,
        active: &mut Surface,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mesh = MeshSummary::default();
        let inputs = TrayInputs {
            mesh: &mesh,
            seat: None,
            unread: 0,
            session_active: false,
        };
        let mut tray = TrayState::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::TopBottomPanel::bottom("shell-taskbar")
                .exact_height(TASKBAR_H)
                .frame(egui::Frame::default().fill(Style::SURFACE))
                .show(ctx, |ui| {
                    let _ = taskbar(ui, active, &mut tray, &inputs);
                });
        })
    }

    #[test]
    fn the_taskbar_renders_and_caches_the_glyphs_headless() {
        // Drive one headless frame of the full-width bottom taskbar (the same
        // Context::run → tessellate path the DRM runner uses, minus the GPU): it
        // must draw primitives without panicking, and every surface glyph must
        // resolve to a real texture through the memory-cached loader.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::default();
        let out = run_taskbar(&ctx, &mut active, Vec::new());
        let ppp = out.pixels_per_point;
        let prims = ctx.tessellate(out.shapes, ppp);
        assert!(!prims.is_empty(), "the taskbar drew nothing");

        for surface in Surface::ALL {
            assert!(
                icon_texture(&ctx, surface.icon_id(), ICON_LOGICAL, Style::TEXT_DIM).is_some(),
                "{surface:?} glyph failed to rasterize + upload"
            );
        }
    }

    /// Count the text shapes in a frame's output, recursing into shape groups.
    fn count_text_shapes(shape: &egui::Shape, n: &mut usize) {
        match shape {
            egui::Shape::Text(_) => *n += 1,
            egui::Shape::Vec(v) => {
                for s in v {
                    count_text_shapes(s, n);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn the_bar_is_icon_only_no_captions_no_tooltips() {
        // Locks W4/W6 — no labels under the app glyphs, no tooltips anywhere.
        // The ONLY text on a quiet bar (no unread badge, flyout closed) is the
        // tray clock's two stacked lines: HH:MM over the date.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::default();
        let out = run_taskbar(&ctx, &mut active, Vec::new());
        let mut texts = 0;
        for clipped in &out.shapes {
            count_text_shapes(&clipped.shape, &mut texts);
        }
        assert_eq!(
            texts, 2,
            "the quiet bar must carry exactly the clock's two lines, no captions"
        );
    }

    #[test]
    fn clicking_a_taskbar_cell_selects_that_surface() {
        // The click→select behaviour survives the icon-only relayout. Mount the
        // real bottom bar and click the leftmost cell (Workbench, the nav head).
        // egui hit-tests a press against the settled widget rects, so prime a
        // couple of no-event frames first, then press one frame + release the
        // next (the egui click model), and the active surface follows the click.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::About;
        // The first cell is flush-left and fills the bar's height, so its centre
        // — half a cell in, half the bar up from the bottom — is the retargeted
        // click point (derived from the layout constants, not a magic number).
        let click = egui::pos2(CELL_W / 2.0, 600.0 - TASKBAR_H / 2.0);
        let press = egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let release = egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        };
        // Prime: settle the layout so egui has the cell rects registered.
        let _ = run_taskbar(&ctx, &mut active, Vec::new());
        let _ = run_taskbar(&ctx, &mut active, Vec::new());
        // Move onto the Workbench cell + press, then release the next frame.
        let _ = run_taskbar(
            &ctx,
            &mut active,
            vec![egui::Event::PointerMoved(click), press],
        );
        let _ = run_taskbar(&ctx, &mut active, vec![release]);
        assert_eq!(
            active,
            Surface::Workbench,
            "clicking the Workbench cell selected it"
        );
    }
}
