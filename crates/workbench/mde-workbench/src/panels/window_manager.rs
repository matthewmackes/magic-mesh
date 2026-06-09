//! Look & Feel → Window Manager — labwc window-behaviour controls (E0.15).
//!
//! Redesigned 2026-06-07: the prior panel drove sway tiling (gaps + container
//! layout) over sway IPC, which is meaningless under labwc (not a tiling WM)
//! and no-op'd on the live session. This surface exposes the real labwc
//! window-behaviour knobs from `~/.config/labwc/rc.xml` and reconfigures the
//! running compositor — mirroring the shell's Devices ▸ Mouse rc.xml rewriter
//! (`crates/shell/mde/src/mouse.rs`): a pure per-element block builder + a
//! find-or-insert rewriter, then an atomic write + `labwc --reconfigure`.
//!
//! Knobs (each a verified element of the shipped `rc.xml`):
//!   - `<focus><followMouse>` / `<raiseOnFocus>` — focus model
//!   - `<snapping><range><inner|outer>` + `<topMaximize>` — drag-to-edge snap
//!   - `<desktops number="N">` — virtual-desktop count (names regenerated)
//!
//! The rest of `rc.xml` (theme, keybinds, the `<mouse><default/>` invariant,
//! the `<libinput>` block the Mouse page owns) is preserved verbatim.

use std::path::{Path, PathBuf};

use iced::widget::{checkbox, column, row, text, text_input};
use iced::{Element, Length, Task};
use mde_theme::Palette;

use crate::controls::{variant_button, ButtonVariant};

/// labwc's shipped system config — the seed when the user has no personal
/// `rc.xml` yet, so a first Apply doesn't drop the system keybinds.
const SYSTEM_RC_XML: &str = "/usr/share/mde/skel/.config/labwc/rc.xml";

/// A minimal valid document, used only when neither a user nor a system
/// `rc.xml` can be read (so the rewriters always have a `</labwc_config>`).
const SKELETON_RC_XML: &str = "<?xml version=\"1.0\"?>\n<labwc_config>\n</labwc_config>\n";

/// Snap range bound (px). 0 disables edge snapping.
pub const SNAP_RANGE_MAX: u32 = 200;
/// Virtual-desktop count bounds.
pub const DESKTOPS_MIN: u32 = 1;
pub const DESKTOPS_MAX: u32 = 9;

#[derive(Debug, Clone, Default)]
pub struct WindowManagerPanel {
    pub follow_mouse: bool,
    pub raise_on_focus: bool,
    pub snap_range_input: String,
    pub top_maximize: bool,
    pub desktops_input: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        follow_mouse: bool,
        raise_on_focus: bool,
        snap_range: u32,
        top_maximize: bool,
        desktops: u32,
    },
    FollowMouseChanged(bool),
    RaiseOnFocusChanged(bool),
    SnapRangeChanged(String),
    TopMaximizeChanged(bool),
    DesktopsChanged(String),
    ApplyClicked,
    /// Apply finished: Ok(path) persisted + reconfigured, Err(msg) failed.
    Applied(Result<String, String>),
}

impl WindowManagerPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            // labwc shipped defaults (match the vendored rc.xml).
            follow_mouse: false,
            raise_on_focus: true,
            snap_range_input: "12".to_string(),
            top_maximize: true,
            desktops_input: "4".to_string(),
            ..Self::default()
        }
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let xml = read_base_rc();
                Message::Loaded {
                    follow_mouse: parse_yes(&xml, "followMouse").unwrap_or(false),
                    raise_on_focus: parse_yes(&xml, "raiseOnFocus").unwrap_or(true),
                    snap_range: parse_snap_range(&xml).unwrap_or(12),
                    top_maximize: parse_yes(&xml, "topMaximize").unwrap_or(true),
                    desktops: parse_desktops(&xml).unwrap_or(4),
                }
            },
            crate::Message::WindowManager,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                follow_mouse,
                raise_on_focus,
                snap_range,
                top_maximize,
                desktops,
            } => {
                self.follow_mouse = follow_mouse;
                self.raise_on_focus = raise_on_focus;
                self.snap_range_input = snap_range.to_string();
                self.top_maximize = top_maximize;
                self.desktops_input = desktops.to_string();
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::FollowMouseChanged(v) => {
                self.follow_mouse = v;
                Task::none()
            }
            Message::RaiseOnFocusChanged(v) => {
                self.raise_on_focus = v;
                Task::none()
            }
            Message::SnapRangeChanged(v) => {
                self.snap_range_input = v;
                Task::none()
            }
            Message::TopMaximizeChanged(v) => {
                self.top_maximize = v;
                Task::none()
            }
            Message::DesktopsChanged(v) => {
                self.desktops_input = v;
                Task::none()
            }
            Message::ApplyClicked => {
                if self.busy {
                    return Task::none();
                }
                let snap_range = match parse_bounded(&self.snap_range_input, 0, SNAP_RANGE_MAX) {
                    Ok(v) => v,
                    Err(()) => {
                        self.status = format!("Snap distance must be 0–{SNAP_RANGE_MAX} pixels.");
                        return Task::none();
                    }
                };
                let desktops = match parse_bounded(&self.desktops_input, DESKTOPS_MIN, DESKTOPS_MAX)
                {
                    Ok(v) => v,
                    Err(()) => {
                        self.status = format!("Desktops must be {DESKTOPS_MIN}–{DESKTOPS_MAX}.");
                        return Task::none();
                    }
                };
                self.busy = true;
                self.status = "Applying…".into();
                let follow_mouse = self.follow_mouse;
                let raise_on_focus = self.raise_on_focus;
                let top_maximize = self.top_maximize;
                Task::perform(
                    async move {
                        let result = apply_rc(
                            follow_mouse,
                            raise_on_focus,
                            snap_range,
                            top_maximize,
                            desktops,
                        );
                        Message::Applied(result)
                    },
                    crate::Message::WindowManager,
                )
            }
            Message::Applied(result) => {
                self.status = match result {
                    Ok(path) => format!("Applied + saved to {path}."),
                    Err(msg) => format!("Could not apply: {msg}"),
                };
                self.busy = false;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let apply_label = if self.busy { "Applying…" } else { "Apply" };
        let apply_btn = variant_button(
            apply_label,
            ButtonVariant::Primary,
            (!self.busy).then_some(crate::Message::WindowManager(Message::ApplyClicked)),
            Palette::dark(),
        );

        column![
            text("Window behaviour").size(18),
            checkbox(self.follow_mouse)
                .label("Focus follows the mouse pointer")
                .on_toggle(|v| crate::Message::WindowManager(Message::FollowMouseChanged(v))),
            checkbox(self.raise_on_focus)
                .label("Raise a window when it gains focus")
                .on_toggle(|v| crate::Message::WindowManager(Message::RaiseOnFocusChanged(v))),
            checkbox(self.top_maximize)
                .label("Maximise when dragged to the top edge")
                .on_toggle(|v| crate::Message::WindowManager(Message::TopMaximizeChanged(v))),
            row![
                text("Edge-snap distance (px)").width(Length::Fixed(220.0)),
                text_input("12", &self.snap_range_input)
                    .on_input(|v| crate::Message::WindowManager(Message::SnapRangeChanged(v))),
            ]
            .spacing(12),
            row![
                text("Virtual desktops").width(Length::Fixed(220.0)),
                text_input("4", &self.desktops_input)
                    .on_input(|v| crate::Message::WindowManager(Message::DesktopsChanged(v))),
            ]
            .spacing(12),
            row![apply_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

// ── Pure helpers (tested without I/O) ────────────────────────────────────────

/// Parse a bounded non-negative integer from a text input. Empty → the low
/// bound. Returns `Err(())` on non-numeric or out-of-range.
fn parse_bounded(input: &str, lo: u32, hi: u32) -> Result<u32, ()> {
    let t = input.trim();
    if t.is_empty() {
        return Ok(lo);
    }
    let v = t.parse::<u32>().map_err(|_| ())?;
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(())
    }
}

/// Read the inner text of `<tag>…</tag>` and return `true` for `yes`.
/// `None` when the tag is absent.
#[must_use]
pub fn parse_yes(xml: &str, tag: &str) -> Option<bool> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().eq_ignore_ascii_case("yes"))
}

/// Parse `<snapping><range><inner>N</inner>…`. Falls back to `<outer>` if
/// `<inner>` is absent. `None` when there's no range.
#[must_use]
pub fn parse_snap_range(xml: &str) -> Option<u32> {
    let snap = xml.find("<snapping")?;
    let rest = &xml[snap..];
    parse_u32_tag(rest, "inner").or_else(|| parse_u32_tag(rest, "outer"))
}

/// Parse the `<desktops number="N">` attribute. `None` when absent.
#[must_use]
pub fn parse_desktops(xml: &str) -> Option<u32> {
    let at = xml.find("<desktops")?;
    let rest = &xml[at..];
    let key = "number=\"";
    let ks = rest.find(key)? + key.len();
    let ke = rest[ks..].find('"')? + ks;
    rest[ks..ke].trim().parse::<u32>().ok()
}

fn parse_u32_tag(xml: &str, tag: &str) -> Option<u32> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    xml[start..end].trim().parse::<u32>().ok()
}

/// Build the `<focus>` block (2-space indented for the top level of rc.xml).
#[must_use]
pub fn focus_block(follow_mouse: bool, raise_on_focus: bool) -> String {
    format!(
        "  <focus>\n    <followMouse>{}</followMouse>\n    <raiseOnFocus>{}</raiseOnFocus>\n  </focus>",
        yes_no(follow_mouse),
        yes_no(raise_on_focus),
    )
}

/// Build the `<snapping>` block. A range of 0 still writes the element (labwc
/// treats 0 as "disabled"), keeping the document explicit.
#[must_use]
pub fn snapping_block(range: u32, top_maximize: bool) -> String {
    format!(
        "  <snapping>\n    <range>\n      <inner>{range}</inner>\n      <outer>{range}</outer>\n    </range>\n    <topMaximize>{}</topMaximize>\n  </snapping>",
        yes_no(top_maximize),
    )
}

/// Build the `<desktops>` block with regenerated `Desktop N` names.
#[must_use]
pub fn desktops_block(count: u32) -> String {
    let n = count.clamp(DESKTOPS_MIN, DESKTOPS_MAX);
    let mut names = String::new();
    for i in 1..=n {
        names.push_str(&format!("      <name>Desktop {i}</name>\n"));
    }
    format!("  <desktops number=\"{n}\">\n    <names>\n{names}    </names>\n  </desktops>")
}

const fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Swap `<tag …>…</tag>` in `xml` for `block`, preserving everything else.
/// When the element is absent, insert `block` just before `</labwc_config>`.
/// Pure — unit-tested for swap / insert / idempotence.
#[must_use]
pub fn rewrite_element(xml: &str, tag: &str, block: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    if let (Some(start), Some(end_inner)) = (xml.find(&open), xml.find(&close)) {
        let end = end_inner + close.len();
        // Back up to the start of the opening line so its indentation is
        // replaced too; keep whatever followed the close tag verbatim.
        let line_start = xml[..start].rfind('\n').map_or(0, |i| i + 1);
        return format!("{}{}{}", &xml[..line_start], block, &xml[end..]);
    }
    match xml.rfind("</labwc_config>") {
        Some(pos) => {
            let line_start = xml[..pos].rfind('\n').map_or(0, |i| i + 1);
            format!("{}{block}\n{}", &xml[..line_start], &xml[line_start..])
        }
        None => format!("{xml}\n{block}"),
    }
}

/// Apply all three window-behaviour blocks to `base`, returning the new rc.xml.
#[must_use]
pub fn rewrite_all(
    base: &str,
    follow_mouse: bool,
    raise_on_focus: bool,
    snap_range: u32,
    top_maximize: bool,
    desktops: u32,
) -> String {
    let out = rewrite_element(base, "focus", &focus_block(follow_mouse, raise_on_focus));
    let out = rewrite_element(&out, "snapping", &snapping_block(snap_range, top_maximize));
    rewrite_element(&out, "desktops", &desktops_block(desktops))
}

// ── I/O (test-seamed via MDE_LABWC_RC) ───────────────────────────────────────

/// The rc.xml path: `MDE_LABWC_RC` if set (test seam — also suppresses the live
/// reconfigure), else `$XDG_CONFIG_HOME/labwc/rc.xml` (honouring `HOME`).
fn rc_path() -> Option<PathBuf> {
    if let Some(seam) = std::env::var_os("MDE_LABWC_RC") {
        return Some(PathBuf::from(seam));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("labwc/rc.xml"))
}

/// Read the rc.xml to rewrite: the user's file if it exists and is non-empty,
/// else the shipped system file (so a first Apply preserves system keybinds),
/// else a minimal valid skeleton.
fn read_base_rc() -> String {
    if let Some(path) = rc_path() {
        if let Ok(s) = std::fs::read_to_string(&path) {
            if s.contains("<labwc_config") {
                return s;
            }
        }
    }
    if let Ok(s) = std::fs::read_to_string(SYSTEM_RC_XML) {
        if s.contains("<labwc_config") {
            return s;
        }
    }
    SKELETON_RC_XML.to_string()
}

/// Rewrite + atomically persist + reconfigure. Returns the written path.
fn apply_rc(
    follow_mouse: bool,
    raise_on_focus: bool,
    snap_range: u32,
    top_maximize: bool,
    desktops: u32,
) -> Result<String, String> {
    let path = rc_path().ok_or_else(|| "cannot resolve rc.xml path".to_string())?;
    let base = read_base_rc();
    let out = rewrite_all(
        &base,
        follow_mouse,
        raise_on_focus,
        snap_range,
        top_maximize,
        desktops,
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    write_rc(&path, &out).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Atomic write (temp sibling + rename), then `labwc --reconfigure` — unless
/// `MDE_LABWC_RC` is set (a test; no live labwc to signal).
fn write_rc(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("xml.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    if std::env::var_os("MDE_LABWC_RC").is_none() {
        let _ = std::process::Command::new("labwc")
            .arg("--reconfigure")
            .status();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<?xml version=\"1.0\"?>\n<labwc_config>\n  <theme>\n    <name>Win2000-MDE</name>\n  </theme>\n\n  <focus>\n    <followMouse>no</followMouse>\n    <raiseOnFocus>yes</raiseOnFocus>\n  </focus>\n\n  <snapping>\n    <range>\n      <inner>12</inner>\n      <outer>12</outer>\n    </range>\n    <topMaximize>yes</topMaximize>\n  </snapping>\n\n  <desktops number=\"4\">\n    <names>\n      <name>Desktop 1</name>\n    </names>\n  </desktops>\n\n  <mouse>\n    <default/>\n  </mouse>\n</labwc_config>\n";

    #[test]
    fn parse_yes_reads_focus_model() {
        assert_eq!(parse_yes(SAMPLE, "followMouse"), Some(false));
        assert_eq!(parse_yes(SAMPLE, "raiseOnFocus"), Some(true));
        assert_eq!(parse_yes(SAMPLE, "topMaximize"), Some(true));
        assert_eq!(parse_yes(SAMPLE, "nope"), None);
    }

    #[test]
    fn parse_snap_range_and_desktops() {
        assert_eq!(parse_snap_range(SAMPLE), Some(12));
        assert_eq!(parse_desktops(SAMPLE), Some(4));
        assert_eq!(parse_snap_range("<labwc_config></labwc_config>"), None);
        assert_eq!(parse_desktops("<labwc_config></labwc_config>"), None);
    }

    #[test]
    fn blocks_render_expected_elements() {
        assert!(focus_block(true, false).contains("<followMouse>yes</followMouse>"));
        assert!(focus_block(true, false).contains("<raiseOnFocus>no</raiseOnFocus>"));
        assert!(snapping_block(20, true).contains("<inner>20</inner>"));
        assert!(snapping_block(20, true).contains("<outer>20</outer>"));
        assert!(snapping_block(20, false).contains("<topMaximize>no</topMaximize>"));
        let d = desktops_block(3);
        assert!(d.contains("number=\"3\""));
        assert_eq!(d.matches("<name>").count(), 3);
        assert!(d.contains("<name>Desktop 3</name>"));
    }

    #[test]
    fn desktops_block_clamps_count() {
        assert!(desktops_block(99).contains(&format!("number=\"{DESKTOPS_MAX}\"")));
        assert!(desktops_block(0).contains(&format!("number=\"{DESKTOPS_MIN}\"")));
    }

    #[test]
    fn parse_bounded_validates() {
        assert_eq!(parse_bounded("", 0, 200), Ok(0));
        assert_eq!(parse_bounded("12", 0, 200), Ok(12));
        assert_eq!(parse_bounded("9", 1, 9), Ok(9));
        assert!(parse_bounded("10", 1, 9).is_err());
        assert!(parse_bounded("garbage", 0, 200).is_err());
        assert!(parse_bounded("-1", 0, 200).is_err());
    }

    #[test]
    fn rewrite_swaps_in_place_and_preserves_neighbours() {
        let out = rewrite_element(SAMPLE, "focus", &focus_block(true, false));
        // Exactly one focus block (no duplication).
        assert_eq!(out.matches("<focus>").count(), 1);
        assert!(out.contains("<followMouse>yes</followMouse>"));
        // Neighbours preserved.
        assert!(out.contains("<name>Win2000-MDE</name>"));
        assert!(out.contains("<default/>"));
        assert!(out.contains("</labwc_config>"));
    }

    #[test]
    fn rewrite_inserts_when_absent_before_root_close() {
        let no_focus = "<?xml version=\"1.0\"?>\n<labwc_config>\n  <mouse>\n    <default/>\n  </mouse>\n</labwc_config>\n";
        let out = rewrite_element(no_focus, "focus", &focus_block(false, true));
        assert_eq!(out.matches("<focus>").count(), 1);
        assert!(out.find("<focus>").unwrap() < out.find("</labwc_config>").unwrap());
        assert!(out.contains("<default/>"));
    }

    #[test]
    fn rewrite_all_is_idempotent() {
        let once = rewrite_all(SAMPLE, true, false, 20, false, 3);
        let twice = rewrite_all(&once, true, false, 20, false, 3);
        assert_eq!(once, twice);
        assert_eq!(twice.matches("<focus>").count(), 1);
        assert_eq!(twice.matches("<snapping>").count(), 1);
        assert_eq!(twice.matches("<desktops").count(), 1);
        // The mouse invariant + theme survive the full rewrite.
        assert!(twice.contains("<default/>"));
        assert!(twice.contains("Win2000-MDE"));
    }

    #[test]
    fn rewrite_all_round_trips_through_the_parsers() {
        let out = rewrite_all(SAMPLE, true, false, 30, false, 6);
        assert_eq!(parse_yes(&out, "followMouse"), Some(true));
        assert_eq!(parse_yes(&out, "raiseOnFocus"), Some(false));
        assert_eq!(parse_snap_range(&out), Some(30));
        assert_eq!(parse_yes(&out, "topMaximize"), Some(false));
        assert_eq!(parse_desktops(&out), Some(6));
    }

    #[test]
    fn loaded_populates_fields() {
        let mut panel = WindowManagerPanel::new();
        let _ = panel.update(Message::Loaded {
            follow_mouse: true,
            raise_on_focus: false,
            snap_range: 24,
            top_maximize: false,
            desktops: 2,
        });
        assert!(panel.follow_mouse);
        assert!(!panel.raise_on_focus);
        assert_eq!(panel.snap_range_input, "24");
        assert!(!panel.top_maximize);
        assert_eq!(panel.desktops_input, "2");
    }

    #[test]
    fn apply_clicked_with_bad_snap_surfaces_validation() {
        let mut panel = WindowManagerPanel::new();
        panel.snap_range_input = "9999".into();
        let _ = panel.update(Message::ApplyClicked);
        assert!(panel.status.contains("Snap distance"));
        assert!(!panel.busy);
    }

    #[test]
    fn apply_clicked_with_bad_desktops_surfaces_validation() {
        let mut panel = WindowManagerPanel::new();
        panel.desktops_input = "50".into();
        let _ = panel.update(Message::ApplyClicked);
        assert!(panel.status.contains("Desktops"));
        assert!(!panel.busy);
    }

    #[test]
    fn apply_clicked_while_busy_is_noop() {
        let mut panel = WindowManagerPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::ApplyClicked);
        assert_eq!(panel.status, "Applying…");
    }

    #[test]
    fn input_messages_mutate_fields() {
        let mut panel = WindowManagerPanel::new();
        let _ = panel.update(Message::FollowMouseChanged(true));
        assert!(panel.follow_mouse);
        let _ = panel.update(Message::SnapRangeChanged("8".into()));
        assert_eq!(panel.snap_range_input, "8");
        let _ = panel.update(Message::DesktopsChanged("3".into()));
        assert_eq!(panel.desktops_input, "3");
    }

    #[test]
    fn view_builds_without_panic() {
        let _ = WindowManagerPanel::new().view();
    }

    #[test]
    fn apply_rc_writes_via_seam_and_round_trips() {
        // Drive the full I/O path against a temp rc.xml via the MDE_LABWC_RC
        // seam (which also suppresses the live `labwc --reconfigure`).
        let dir = std::env::temp_dir().join(format!("mde-wm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rc.xml");
        std::fs::write(&path, SAMPLE).unwrap();
        std::env::set_var("MDE_LABWC_RC", &path);

        let res = apply_rc(true, false, 40, false, 5);
        assert!(res.is_ok(), "apply failed: {res:?}");
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse_yes(&written, "followMouse"), Some(true));
        assert_eq!(parse_snap_range(&written), Some(40));
        assert_eq!(parse_desktops(&written), Some(5));
        // Invariant preserved end-to-end.
        assert!(written.contains("<default/>"));

        std::env::remove_var("MDE_LABWC_RC");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
