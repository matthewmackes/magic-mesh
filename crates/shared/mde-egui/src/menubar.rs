//! `menubar` — the **shared top menu bar** every E12 surface embeds (MENUBAR-ALL,
//! design: `docs/design/menubar-all.md`).
//!
//! One slim strip, at a single consistent height, that renders three things from
//! the shared [`Style`]/[`Motion`]/`fonts` alone (§4 — no raw hex, no literal
//! durations):
//!
//! 1. a **large UPPERCASE mono DISPLAY-tier title**, accent-tinted (the EDTB-7
//!    heading ramp), on the left — decorative identity only (lock 10);
//! 2. an **inline menu strip** — a caller-supplied tree of drop-downs, each item a
//!    real seam (§7): the caller **omits** an absent feature and passes
//!    `enabled = false` for a context-gated one, so a menu is *comprehensive* yet
//!    never carries a dead entry; and
//! 3. a **right-aligned status-chip cluster** — the surface's live indicators.
//!
//! The governing principle (design §7): the menu bar surfaces **all** of a
//! surface's controls, including the advanced/complex ones — it is the operator's
//! complete, discoverable control surface — but only ever the ones whose seam
//! exists. This crate owns the *rendering*; every surface owns its menu **model**
//! (its own action vocabulary + gating), builds it each frame, hands it here, and
//! dispatches the activated [`Item::id`] to its real seam. No surface behaviour is
//! baked in (§6): [`MenuBar::show`] returns which item the operator picked, nothing
//! more.
//!
//! ## The model
//! [`Menu`] (`title` · `mnemonic` · `entries`) → a tree of [`Entry`]: an
//! activatable [`Item`] (with an optional live shortcut hint, an enable gate, and
//! an optional check-mark), a nested [`Entry::Submenu`], a Word-style
//! [`Entry::Separator`] group break, or a non-interactive [`Entry::Caption`] (a dim
//! header like "Attach a session on…"). The four cover the two hardest existing
//! bars (the terminal's tmux/session trees, the editor's Word-97 set) and the
//! dynamic `IaC` catalog without a per-surface escape hatch.
//!
//! ## Keyboard + motion (locks 9 + 12)
//! Each top-level title underlines its **Alt-mnemonic** ([`resolve_mnemonics`]
//! assigns a unique letter left→right); `Alt`+that letter opens (or toggles) the
//! menu. In-menu arrow/Enter/Escape navigation, drop-down open + hover + press, and
//! click-outside-close all ride egui's battle-tested menu machinery, so the two
//! refactored bars keep byte-identical behaviour. On top of that the strip paints a
//! shared-[`Motion`] hover/open **underline** and fades a drop-down's body in on
//! open, and every menu item carries a **2 px accent focus ring** when keyboard-
//! focused (a11y, Quasar lock 5). All of it is **reduce-motion aware**: a surface
//! that zeroes egui's `animation_time` gets instant transitions here too.

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Button, Color32, FontFamily, FontId, Key, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui,
};

use crate::{Motion, Style};

/// The bar's fixed height — the DISPLAY-tier title (lock 2) plus one base gutter,
/// so every surface's bar reads at the same slim consistent height (lock 11).
pub const BAR_HEIGHT: f32 = Style::DISPLAY + Style::SP_S;

/// Minimum drop-down width so short menus don't collapse into slivers — six shared
/// spacing units (§4), the derivation both refactored bars already used.
const MENU_MIN_W: f32 = Style::SP_XL * 6.0;

// ─────────────────────────────── status chips ───────────────────────────────

/// The semantic tone of a [`StatusChip`], resolved to a [`Style`] palette token so
/// no surface mints a raw colour for a status readout (§4).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ChipTone {
    /// A plain read-out (host, count, codec) — dim body text.
    #[default]
    Neutral,
    /// An interactive / branded value — the Quasar accent.
    Info,
    /// A healthy / connected state — success green.
    Ok,
    /// A degraded / attention state — warning amber.
    Warn,
    /// A failed / offline / error state — danger red.
    Danger,
}

impl ChipTone {
    /// The [`Style`] token this tone paints in (never a raw literal, §4).
    #[must_use]
    pub const fn color(self) -> Color32 {
        match self {
            Self::Neutral => Style::TEXT_DIM,
            Self::Info => Style::ACCENT,
            Self::Ok => Style::OK,
            Self::Warn => Style::WARN,
            Self::Danger => Style::DANGER,
        }
    }
}

/// One right-aligned live indicator in the bar's status cluster (lock 6).
///
/// An optional leading glyph, a text value, and a semantic [`ChipTone`]. The
/// surface refills the cluster from its real state each frame (§7 — a chip reflects
/// live state, never a placeholder).
#[derive(Clone, Debug)]
pub struct StatusChip {
    /// An optional leading glyph (e.g. a status dot / codec icon).
    pub icon: Option<String>,
    /// The value text.
    pub text: String,
    /// The semantic tone.
    pub tone: ChipTone,
}

impl StatusChip {
    /// A chip with a text value and tone (no icon).
    #[must_use]
    pub fn new(text: impl Into<String>, tone: ChipTone) -> Self {
        Self {
            icon: None,
            text: text.into(),
            tone,
        }
    }

    /// A chip with a leading glyph, a text value, and a tone.
    #[must_use]
    pub fn with_icon(icon: impl Into<String>, text: impl Into<String>, tone: ChipTone) -> Self {
        Self {
            icon: Some(icon.into()),
            text: text.into(),
            tone,
        }
    }
}

// ─────────────────────────────── the menu model ─────────────────────────────

/// One activatable menu item, generic over the caller's activation id `Id`.
///
/// `Id` is the surface's own action vocabulary — a `Copy`/`Clone` enum. Present
/// only when its seam exists; `enabled = false` renders the authentic disabled grey
/// for a context-gated one (§7). `checked` draws a leading check-mark for a
/// toggle/radio item.
#[derive(Clone, Debug)]
pub struct Item<Id> {
    /// The value returned from [`MenuBar::show`] when the operator activates this
    /// item — the surface maps it to its real seam.
    pub id: Id,
    /// The visible label.
    pub label: String,
    /// The right-aligned live shortcut hint (resolved by the caller from its
    /// keymap so a rebind reflects), or `None` for no chord.
    pub shortcut: Option<String>,
    /// Whether the item is enabled — `false` renders the disabled grey (§7).
    pub enabled: bool,
    /// A leading check-mark state for a toggle/radio item, or `None` for a plain
    /// command.
    pub checked: Option<bool>,
}

impl<Id> Item<Id> {
    /// A plain, enabled command item.
    #[must_use]
    pub fn new(id: Id, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: None,
        }
    }

    /// Set the live shortcut hint.
    #[must_use]
    pub fn shortcut(mut self, hint: impl Into<String>) -> Self {
        self.shortcut = Some(hint.into());
        self
    }

    /// Set the enable gate (§7 — `false` greys the item, never omits its seam).
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the check-mark state for a toggle/radio item.
    #[must_use]
    pub const fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
}

impl<Id: Clone> Item<Id> {
    /// The id this item activates for a `clicked` frame, honouring its enable gate:
    /// `None` for a disabled item **even on a click** (§7 — a disabled entry is
    /// never a silent activation path). The single decision the render and the
    /// tests share, so "disabled ⇒ non-activatable" is proven without egui.
    fn activation(&self, clicked: bool) -> Option<Id> {
        (self.enabled && clicked).then(|| self.id.clone())
    }
}

/// One entry in a menu body — the four shapes that cover every surface's needs
/// without a leaky per-surface escape hatch.
#[derive(Clone, Debug)]
pub enum Entry<Id> {
    /// An activatable command / toggle.
    Item(Item<Id>),
    /// A nested submenu (its own title + optional mnemonic + entries).
    Submenu {
        /// The submenu's label.
        label: String,
        /// Its Alt-mnemonic within the open parent, if any.
        mnemonic: Option<char>,
        /// Its entries.
        entries: Vec<Self>,
    },
    /// A Word-style group separator drawn above the next entry.
    Separator,
    /// A non-interactive dim caption (a section header inside a drop-down, e.g.
    /// "Attach a session on…") — an honest label, never an activatable no-op.
    Caption(String),
}

/// A top-level menu: its title, its Alt-mnemonic (auto-resolved when `None`), and
/// its body entries.
#[derive(Clone, Debug)]
pub struct Menu<Id> {
    /// The top-level title (File, Edit, Terminal, …).
    pub title: String,
    /// An explicit Alt-mnemonic; `None` lets [`resolve_mnemonics`] pick a unique
    /// letter.
    pub mnemonic: Option<char>,
    /// The drop-down's entries.
    pub entries: Vec<Entry<Id>>,
}

impl<Id> Menu<Id> {
    /// A menu with a title and entries (mnemonic auto-resolved).
    #[must_use]
    pub fn new(title: impl Into<String>, entries: Vec<Entry<Id>>) -> Self {
        Self {
            title: title.into(),
            mnemonic: None,
            entries,
        }
    }

    /// Set an explicit Alt-mnemonic (overriding the auto-resolution).
    #[must_use]
    pub const fn mnemonic(mut self, mnemonic: char) -> Self {
        self.mnemonic = Some(mnemonic);
        self
    }
}

/// The whole per-frame bar model a surface hands [`MenuBar::show`].
///
/// Its UPPERCASE title, its category accent (a [`Style`] token), its menus, and its
/// live status cluster. Borrowed so the surface can build the pieces from its own
/// static tables + live state without giving up ownership.
pub struct MenuBarModel<'a, Id> {
    /// The workspace title — rendered UPPERCASE, mono, DISPLAY-tier, accent-tinted.
    pub title: &'a str,
    /// The surface's category accent (the dock group's [`Style`] token).
    pub accent: Color32,
    /// The top-level menus, left→right.
    pub menus: &'a [Menu<Id>],
    /// The live status cluster, laid out left→right hugging the right edge.
    pub status: &'a [StatusChip],
}

// ─────────────────────────────── mnemonics ──────────────────────────────────

/// Resolve a unique underline **Alt-mnemonic** for each menu, left→right.
///
/// A menu's explicit [`Menu::mnemonic`] wins; otherwise the first ASCII-alphanumeric
/// char of its title not already claimed; falling back to its first alphanumeric
/// char even if it collides (a shared underline beats none). Case-insensitive; the
/// returned char is uppercase. Pure — the mnemonic assignment is unit-tested without
/// egui.
#[must_use]
pub fn resolve_mnemonics<Id>(menus: &[Menu<Id>]) -> Vec<Option<char>> {
    let mut used: Vec<char> = Vec::with_capacity(menus.len());
    let mut out: Vec<Option<char>> = Vec::with_capacity(menus.len());
    for menu in menus {
        let first_alnum = || {
            menu.title
                .chars()
                .map(|c| c.to_ascii_uppercase())
                .find(|c: &char| c.is_ascii_alphanumeric())
        };
        let chosen = menu
            .mnemonic
            .map(|c| c.to_ascii_uppercase())
            .filter(char::is_ascii_alphanumeric)
            .or_else(|| {
                menu.title
                    .chars()
                    .map(|c| c.to_ascii_uppercase())
                    .find(|c: &char| c.is_ascii_alphanumeric() && !used.contains(c))
            })
            .or_else(first_alnum);
        if let Some(c) = chosen {
            used.push(c);
        }
        out.push(chosen);
    }
    out
}

/// The `Alt`-combo [`Key`] a mnemonic char triggers (`'F'` → [`Key::F`]), or `None`
/// for a char egui has no key for. Split out so the letter→key map is testable.
fn mnemonic_key(mnemonic: char) -> Option<Key> {
    Key::from_name(&mnemonic.to_ascii_uppercase().to_string())
}

// ─────────────────────────────── rendering ──────────────────────────────────

/// The UPPERCASE display form of a workspace title (lock 14) — the exact transform
/// the title header paints, so the casing is testable without a render.
fn display_title(title: &str) -> String {
    title.to_uppercase()
}

/// The animation duration to use, honouring **reduce-motion**: a surface that zeroes
/// egui's `animation_time` (the platform's "no animation" signal) collapses every
/// bar transition to instant.
fn motion_secs(ui: &Ui, secs: f32) -> f32 {
    if ui.style().animation_time <= 0.0 {
        0.0
    } else {
        secs
    }
}

/// The stateless shared menu-bar widget. All transient state (which menu is open)
/// lives in egui's per-frame memory, so a surface just calls [`MenuBar::show`] each
/// frame with a freshly-built model.
pub struct MenuBar;

impl MenuBar {
    /// Render the bar and return the [`Item::id`] the operator activated this frame,
    /// if any. The surface maps that id to its real seam (§6). Generic over the
    /// caller's id type so every surface keeps its own action vocabulary.
    pub fn show<Id: Clone>(ui: &mut Ui, model: &MenuBarModel<'_, Id>) -> Option<Id> {
        let mnemonics = resolve_mnemonics(model.menus);
        let mut picked: Option<Id> = None;
        ui.horizontal(|ui| {
            ui.set_min_height(BAR_HEIGHT);
            // Title — decorative identity only (lock 10), left-anchored.
            title_header(ui, model.title, model.accent);
            ui.add_space(Style::SP_M);
            // Inline menu strip.
            menu_strip(ui, model, &mnemonics, &mut picked);
            // Live status cluster, hugging the right edge (lock 3/6).
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for chip in model.status.iter().rev() {
                    status_chip(ui, chip);
                }
            });
        });
        picked
    }
}

/// Paint the large UPPERCASE mono DISPLAY-tier accent title (lock 2/14).
fn title_header(ui: &mut Ui, title: &str, accent: Color32) {
    ui.label(
        RichText::new(display_title(title))
            .family(FontFamily::Monospace)
            .size(Style::DISPLAY)
            .color(accent),
    );
}

/// Render the inline menu strip: each top-level drop-down, its underlined
/// mnemonic + animated hover/open indicator + open-fade body, then the
/// Alt-mnemonic key handling. Records the activated id into `picked`.
fn menu_strip<Id: Clone>(
    ui: &mut Ui,
    model: &MenuBarModel<'_, Id>,
    mnemonics: &[Option<char>],
    picked: &mut Option<Id>,
) {
    // Flat, chrome-free top-level buttons — the menu-bar look (egui's own menu-bar
    // style): a transparent resting fill so a title reads as a label until hovered.
    flatten_menu_buttons(ui);

    let bar_id = ui.id();
    // The menu open *before* this frame's interaction — drives the open-spring and
    // tells the Alt handler which menu to toggle.
    let open_before = egui::menu::BarState::load(ui.ctx(), bar_id)
        .as_ref()
        .map(|root| root.id);

    let mut rects: Vec<(egui::Id, Rect)> = Vec::with_capacity(model.menus.len());
    for (menu, mnemonic) in model.menus.iter().zip(mnemonics) {
        let menu_id = bar_id.with(menu.title.as_str());
        let is_open = open_before == Some(menu_id);
        let secs = motion_secs(ui, Motion::FAST);
        let fade = Motion::animate(ui.ctx(), menu_id.with("open-fade"), is_open, secs);

        let job = menu_label_job(&menu.title, *mnemonic);
        let response = egui::menu::menu_button(ui, job, |ui| {
            ui.set_min_width(MENU_MIN_W);
            // Open-spring: fade the body in (clamped so it never fully vanishes).
            if fade < 1.0 {
                ui.multiply_opacity(fade.max(0.2));
            }
            render_entries(ui, &menu.entries, picked);
        })
        .response;
        rects.push((menu_id, response.rect));

        // The shared-motion hover/open underline indicator (reduce-motion aware).
        let hot = response.hovered() || is_open;
        let grow = Motion::animate(ui.ctx(), menu_id.with("underline"), hot, secs);
        paint_underline(ui, response.rect, model.accent, grow);
    }

    handle_alt_mnemonics(ui, bar_id, &rects, mnemonics);
}

/// Flatten the top-level menu buttons to the flat menu-bar look — a transparent
/// resting fill and no resting/hover strokes (egui's own menu-bar styling), so the
/// titles read as labels until hovered/open.
fn flatten_menu_buttons(ui: &mut Ui) {
    let widgets = &mut ui.style_mut().visuals.widgets;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.hovered.bg_stroke = Stroke::NONE;
    widgets.active.bg_stroke = Stroke::NONE;
}

/// Build a top-level (or submenu) label as a [`LayoutJob`] with its mnemonic char
/// underlined. Its `.text()` equals `label`, so egui derives the same menu id the
/// Alt handler computes from `label`.
fn menu_label_job(label: &str, mnemonic: Option<char>) -> LayoutJob {
    let color = Style::TEXT;
    let font = FontId::new(Style::BODY, FontFamily::Proportional);
    let underline = Stroke::new(1.0, color);
    let mut job = LayoutJob::default();
    let mut buf = [0u8; 4];
    let mut underlined = false;
    for ch in label.chars() {
        let is_mnemonic = !underlined && mnemonic.is_some_and(|m| ch.eq_ignore_ascii_case(&m));
        let format = TextFormat {
            font_id: font.clone(),
            color,
            underline: if is_mnemonic { underline } else { Stroke::NONE },
            ..Default::default()
        };
        job.append(ch.encode_utf8(&mut buf), 0.0, format);
        underlined |= is_mnemonic;
    }
    job
}

/// Paint the shared-motion accent underline under a top-level menu title: a 2 px
/// line that grows from the centre to `t` of the title width (a no-op at `t == 0`).
fn paint_underline(ui: &Ui, rect: Rect, accent: Color32, t: f32) {
    if t <= 0.0 {
        return;
    }
    let width = rect.width() * t;
    let y = rect.bottom() - 1.0;
    let x0 = width.mul_add(-0.5, rect.center().x);
    ui.painter().line_segment(
        [egui::pos2(x0, y), egui::pos2(x0 + width, y)],
        Stroke::new(2.0, accent),
    );
}

/// Render a menu body's entries into an open drop-down.
fn render_entries<Id: Clone>(ui: &mut Ui, entries: &[Entry<Id>], picked: &mut Option<Id>) {
    for entry in entries {
        match entry {
            Entry::Separator => {
                ui.separator();
            }
            Entry::Caption(text) => {
                crate::widgets::muted_note(ui, text.as_str());
            }
            Entry::Item(item) => render_item(ui, item, picked),
            Entry::Submenu {
                label,
                mnemonic,
                entries,
            } => {
                let job = menu_label_job(label, *mnemonic);
                ui.menu_button(job, |ui| {
                    ui.set_min_width(MENU_MIN_W);
                    render_entries(ui, entries, picked);
                });
            }
        }
    }
}

/// Render one item as a button — a leading check-mark for a toggle/radio item, its
/// right-aligned shortcut hint, disabled-grey when gated (§7), and a 2 px accent
/// focus ring when keyboard-focused (a11y, lock 5). Records the activated id.
fn render_item<Id: Clone>(ui: &mut Ui, item: &Item<Id>, picked: &mut Option<Id>) {
    let label = match item.checked {
        Some(true) => format!("\u{2713} {}", item.label),
        Some(false) => format!("\u{2003}{}", item.label),
        None => item.label.clone(),
    };
    let mut button = Button::new(label);
    if let Some(hint) = &item.shortcut {
        button = button.shortcut_text(hint);
    }
    let response = ui.add_enabled(item.enabled, button);
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            Style::RADIUS,
            Stroke::new(2.0, Style::ACCENT),
            StrokeKind::Outside,
        );
    }
    if let Some(id) = item.activation(response.clicked()) {
        *picked = Some(id);
        ui.close_menu();
    }
}

/// Handle `Alt`+mnemonic: open (or toggle closed) the matching top-level menu by
/// driving egui's own menu-bar [`BarState`], so a keyboard open behaves exactly
/// like a click. Runs after the strip renders, since it keys off this frame's
/// button rects.
fn handle_alt_mnemonics(
    ui: &Ui,
    bar_id: egui::Id,
    rects: &[(egui::Id, Rect)],
    mnemonics: &[Option<char>],
) {
    if !ui.input(|i| i.modifiers.alt) {
        return;
    }
    for (mnemonic, (menu_id, rect)) in mnemonics.iter().zip(rects) {
        let Some(key) = mnemonic.and_then(mnemonic_key) else {
            continue;
        };
        if !ui.input(|i| i.key_pressed(key)) {
            continue;
        }
        let mut state = egui::menu::BarState::load(ui.ctx(), bar_id);
        let already_open = state.as_ref().map(|root| root.id) == Some(*menu_id);
        let response = if already_open {
            egui::menu::MenuResponse::Close
        } else {
            egui::menu::MenuResponse::Create(rect.left_bottom(), *menu_id)
        };
        egui::menu::MenuRoot::handle_menu_response(&mut state, response);
        state.store(ui.ctx(), bar_id);
        ui.ctx().request_repaint();
        break;
    }
}

/// Paint one right-aligned status chip: a rounded [`Style::SURFACE_HI`] plate with
/// the tone-coloured `icon + text` at the caption size.
fn status_chip(ui: &mut Ui, chip: &StatusChip) {
    let color = chip.tone.color();
    let text = chip
        .icon
        .as_ref()
        .map_or_else(|| chip.text.clone(), |icon| format!("{icon} {}", chip.text));
    let galley = ui.painter().layout_no_wrap(
        text,
        FontId::new(Style::SMALL, FontFamily::Proportional),
        color,
    );
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let (rect, _) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    ui.painter()
        .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    ui.painter().galley(rect.min + pad, galley, color);
    ui.add_space(Style::SP_XS);
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::panic, clippy::assertions_on_constants)]
mod tests {
    use super::{
        display_title, mnemonic_key, motion_secs, resolve_mnemonics, ChipTone, Entry, Item, Menu,
        MenuBar, MenuBarModel, StatusChip, BAR_HEIGHT,
    };
    use crate::{Motion, Style};

    /// A small four-menu editor-shaped model over `&str` ids for the model tests.
    fn sample_menus() -> Vec<Menu<&'static str>> {
        vec![
            Menu::new(
                "File",
                vec![
                    Entry::Item(Item::new("new", "New")),
                    Entry::Item(Item::new("open", "Open\u{2026}").shortcut("Ctrl+P")),
                    Entry::Separator,
                    Entry::Item(Item::new("save", "Save").enabled(false)),
                ],
            ),
            Menu::new(
                "Edit",
                vec![Entry::Item(Item::new("copy", "Copy").enabled(false))],
            ),
            Menu::new(
                "View",
                vec![
                    Entry::Caption("Panels".to_owned()),
                    Entry::Item(Item::new("tree", "Project Tree").checked(true)),
                    Entry::Submenu {
                        label: "Colour Scheme".to_owned(),
                        mnemonic: None,
                        entries: vec![Entry::Item(Item::new("quasar", "Quasar").checked(false))],
                    },
                ],
            ),
            Menu::new("Format", vec![Entry::Item(Item::new("bold", "Bold"))]),
        ]
    }

    // ── the model ────────────────────────────────────────────────────────────

    #[test]
    fn model_holds_the_menu_tree_and_reads_back() {
        let menus = sample_menus();
        assert_eq!(menus.len(), 4);
        // File carries a shortcut, a separator, and a disabled (gated) item — the
        // §7 shapes.
        let Entry::Item(open) = &menus[0].entries[1] else {
            panic!("File[1] is Open");
        };
        assert_eq!(open.shortcut.as_deref(), Some("Ctrl+P"));
        assert!(matches!(menus[0].entries[2], Entry::Separator));
        let Entry::Item(save) = &menus[0].entries[3] else {
            panic!("File[3] is Save");
        };
        assert!(
            !save.enabled,
            "a context-gated item ships disabled, not omitted"
        );
        // View carries a caption, a checked toggle, and a nested submenu.
        assert!(matches!(menus[2].entries[0], Entry::Caption(_)));
        let Entry::Item(tree) = &menus[2].entries[1] else {
            panic!("View[1] is the tree toggle");
        };
        assert_eq!(tree.checked, Some(true));
        assert!(matches!(menus[2].entries[2], Entry::Submenu { .. }));
    }

    #[test]
    fn status_chip_constructors_carry_tone_and_icon() {
        let plain = StatusChip::new("nyc3", ChipTone::Neutral);
        assert!(plain.icon.is_none());
        assert_eq!(plain.text, "nyc3");
        let with_icon = StatusChip::with_icon("\u{25CF}", "3 peers", ChipTone::Ok);
        assert_eq!(with_icon.icon.as_deref(), Some("\u{25CF}"));
        assert_eq!(with_icon.tone, ChipTone::Ok);
    }

    #[test]
    fn chip_tones_map_to_distinct_style_tokens() {
        // Each tone resolves to a shared Style token (no raw literal, §4), and the
        // five are visually distinct so a status read-out never reads ambiguously.
        let tones = [
            ChipTone::Neutral,
            ChipTone::Info,
            ChipTone::Ok,
            ChipTone::Warn,
            ChipTone::Danger,
        ];
        for (i, a) in tones.iter().enumerate() {
            for b in &tones[i + 1..] {
                assert_ne!(a.color(), b.color(), "chip tones must be distinct");
            }
        }
        assert_eq!(ChipTone::Ok.color(), Style::OK);
        assert_eq!(ChipTone::Danger.color(), Style::DANGER);
        assert_eq!(ChipTone::default(), ChipTone::Neutral);
    }

    // ── mnemonic resolution ──────────────────────────────────────────────────

    #[test]
    fn mnemonics_are_unique_and_prefer_the_first_free_letter() {
        let menus = sample_menus();
        let m = resolve_mnemonics(&menus);
        assert_eq!(m[0], Some('F'), "File → F");
        assert_eq!(m[1], Some('E'), "Edit → E");
        assert_eq!(m[2], Some('V'), "View → V");
        // Format's 'F' is taken by File, so it falls to the next free letter, 'O'.
        assert_eq!(m[3], Some('O'), "Format → O (F taken)");
        // Every assigned mnemonic is unique.
        let assigned: Vec<char> = m.iter().flatten().copied().collect();
        for (i, c) in assigned.iter().enumerate() {
            assert!(
                !assigned[i + 1..].contains(c),
                "mnemonic {c} assigned twice"
            );
        }
    }

    #[test]
    fn an_explicit_mnemonic_overrides_the_auto_letter() {
        let menus = vec![Menu::new("Format", vec![Entry::<()>::Separator]).mnemonic('t')];
        assert_eq!(resolve_mnemonics(&menus)[0], Some('T'), "explicit 't' → T");
    }

    #[test]
    fn mnemonic_key_maps_letters_to_egui_keys() {
        assert_eq!(mnemonic_key('F'), Some(egui::Key::F));
        assert_eq!(mnemonic_key('t'), Some(egui::Key::T), "case-insensitive");
        assert_eq!(mnemonic_key('#'), None, "no key for a symbol");
    }

    // ── disabled ⇒ non-activatable (§7) ──────────────────────────────────────

    #[test]
    fn a_disabled_item_never_activates_even_on_a_click() {
        let enabled = Item::new(7u8, "Save");
        let disabled = Item::new(7u8, "Save").enabled(false);
        // A click on an enabled item yields its id; on a disabled one, nothing —
        // the exact decision `render_item` makes, so a disabled entry is proven a
        // dead click path, never a silent no-op.
        assert_eq!(enabled.activation(true), Some(7));
        assert_eq!(enabled.activation(false), None, "no click, no activation");
        assert_eq!(
            disabled.activation(true),
            None,
            "disabled swallows the click"
        );
    }

    // ── title casing (lock 14) ───────────────────────────────────────────────

    #[test]
    fn the_title_renders_uppercase() {
        assert_eq!(display_title("Voice"), "VOICE");
        assert_eq!(display_title("Mesh View"), "MESH VIEW");
        assert_eq!(display_title("EDITOR"), "EDITOR");
    }

    // ── reduce-motion ────────────────────────────────────────────────────────

    #[test]
    fn reduce_motion_collapses_transitions_to_instant() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(
                    motion_secs(ui, Motion::FAST),
                    Motion::FAST,
                    "default animation_time keeps the shared duration"
                );
                ui.style_mut().animation_time = 0.0;
                assert_eq!(
                    motion_secs(ui, Motion::FAST),
                    0.0,
                    "a zeroed animation_time collapses to instant (reduce-motion)"
                );
            });
        });
    }

    // ── the bar renders headless (title + menus + status) ────────────────────

    #[test]
    fn menu_bar_renders_headless_and_is_idle_without_a_click() {
        use egui::{pos2, vec2, Rect};
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let menus = sample_menus();
        let status = [
            StatusChip::new("nyc3", ChipTone::Neutral),
            StatusChip::with_icon("\u{25CF}", "online", ChipTone::Ok),
        ];
        let mut picked = Some("unset");
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("shared-menu-bar").show(ctx, |ui| {
                let model = MenuBarModel {
                    title: "Editor",
                    accent: Style::ACCENT,
                    menus: &menus,
                    status: &status,
                };
                picked = MenuBar::show(ui, &model);
            });
        });
        assert!(picked.is_none(), "nothing activates without a click");
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the bar produced no draw primitives");
        assert!(
            BAR_HEIGHT > Style::DISPLAY,
            "the bar clears its display title"
        );
    }
}
