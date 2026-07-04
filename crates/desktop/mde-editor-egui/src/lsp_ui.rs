//! The **LSP → UI glue** (EDITOR-LSP-2).
//!
//! The small, egui-light layer that turns the [`lsp`](crate::lsp) subsystem's
//! honest data into things the editor surface paints — a severity→token color
//! map, the per-document diagnostics overlay the widget draws (gutter markers +
//! underlines + hover), and the honest chrome status for a gated / live server
//! session.
//!
//! It calls only `lsp`'s public seam (never its protocol internals): the folded
//! [`Diagnostic`] store, the [`LspState`] snapshot, and the
//! [`server_spec`](crate::lsp::server_spec) registry. Everything here is pure
//! state + methods over `(&Buffer, &[Diagnostic])` — no `Ui`, no frame — so it
//! is unit-testable without a live egui frame or a real language server (§7).

use std::collections::BTreeMap;
use std::ops::Range;

use mde_egui::egui::Color32;
use mde_egui::Style;

use crate::buffer::Buffer;
use crate::highlight::Language;
use crate::lsp::{server_spec, Diagnostic, LspClient, LspState, Severity};

/// The Carbon status/token color a diagnostic [`Severity`] paints with (§4 — the
/// one severity→token map; no raw hex at any paint site, exactly as
/// [`CodeToken::color`](mde_egui::code::CodeToken::color) is for syntax).
///
/// Error → danger red, Warning → warning amber, Information → the interactive
/// accent, Hint → the dimmed foreground (present, quiet). The four are visually
/// distinct so a glance separates an error underline from a hint.
#[must_use]
pub(crate) const fn severity_color(severity: Severity) -> Color32 {
    match severity {
        Severity::Error => Style::DANGER,
        Severity::Warning => Style::WARN,
        Severity::Information => Style::ACCENT,
        Severity::Hint => Style::TEXT_DIM,
    }
}

/// One resolved diagnostic ready to paint: the rope **char** range it underlines
/// (converted from the LSP wire's zero-based line / UTF-16 column), its
/// severity, and the message shown on hover.
pub(crate) struct DiagMark {
    /// The char-index range in the rope the squiggle underlines.
    pub(crate) chars: Range<usize>,
    /// The severity — drives the underline + gutter color.
    pub(crate) severity: Severity,
    /// The human-readable message, shown when the operator hovers the range.
    pub(crate) message: String,
}

/// The per-document diagnostics the text widget paints.
///
/// Derived from the LSP store **once per diagnostics epoch** — the §7
/// epoch-gate: quiet frames reuse the last build instead of re-resolving every
/// position. The panel rebuilds this only when [`LspClient::diagnostics_epoch`] moves
/// (see [`needs_refresh`](Self::needs_refresh)); the widget reads
/// [`severity_for_line`](Self::severity_for_line) for gutter markers and
/// [`marks`](Self::marks) for the underlines + hover.
#[derive(Default)]
pub struct DiagnosticsOverlay {
    /// The epoch this overlay was last built for.
    epoch: u64,
    /// Whether it has been built at least once (a fresh overlay is empty but
    /// must still rebuild on the first non-gated frame).
    built: bool,
    /// Worst severity per logical line (ascending by line) — the gutter markers.
    lines: Vec<(usize, Severity)>,
    /// Underline ranges + messages — the body squiggles + hover.
    marks: Vec<DiagMark>,
}

impl DiagnosticsOverlay {
    /// Whether the overlay must be rebuilt for `epoch` (never built yet, or the
    /// diagnostics epoch moved). The panel checks this **before** fetching the
    /// diagnostics, so a quiet frame skips both the fetch and the recompute.
    #[must_use]
    pub(crate) const fn needs_refresh(&self, epoch: u64) -> bool {
        !self.built || self.epoch != epoch
    }

    /// Rebuild the overlay from the freshly published `diags` at `epoch`,
    /// resolving each diagnostic's zero-based line / UTF-16 column onto rope
    /// char offsets against `buffer`.
    pub(crate) fn rebuild(&mut self, epoch: u64, buffer: &Buffer, diags: &[Diagnostic]) {
        self.epoch = epoch;
        self.built = true;
        self.lines.clear();
        self.marks.clear();

        // Gutter: the worst severity on each line a diagnostic covers.
        let mut worst: BTreeMap<usize, Severity> = BTreeMap::new();
        let last_line = buffer.len_lines().saturating_sub(1);
        for d in diags {
            let start_line = (d.start_line as usize).min(last_line);
            let end_line = (d.end_line as usize).min(last_line).max(start_line);
            for line in start_line..=end_line {
                worst
                    .entry(line)
                    .and_modify(|s| *s = (*s).max(d.severity))
                    .or_insert(d.severity);
            }

            // Underline: the char range, converting UTF-16 columns to chars.
            let start = resolve_pos(buffer, d.start_line, d.start_character);
            let end = resolve_pos(buffer, d.end_line, d.end_character).max(start);
            let chars = if end > start {
                start..end
            } else {
                // A zero-width diagnostic (a point) still gets a one-cell
                // underline so it is visible, clamped to the buffer.
                start..(start + 1).min(buffer.len_chars()).max(start)
            };
            self.marks.push(DiagMark {
                chars,
                severity: d.severity,
                message: d.message.clone(),
            });
        }
        self.lines = worst.into_iter().collect();
    }

    /// The worst diagnostic severity on logical `line`, or `None` — the gutter
    /// marker's color for that line.
    #[must_use]
    pub(crate) fn severity_for_line(&self, line: usize) -> Option<Severity> {
        self.lines
            .binary_search_by_key(&line, |&(l, _)| l)
            .ok()
            .map(|i| self.lines[i].1)
    }

    /// The resolved underline marks — the body squiggles + hover ranges.
    #[must_use]
    pub(crate) fn marks(&self) -> &[DiagMark] {
        &self.marks
    }
}

/// Resolve an LSP position (zero-based `line`, `utf16_col` = UTF-16 code units,
/// the protocol default) to a rope char index against `buffer`.
///
/// Walks the line's chars accumulating UTF-16 units until the target column,
/// landing on the nearest char boundary — so an astral char (2 UTF-16 units)
/// advances the column by two but the char offset by one, and a column past the
/// line's content clamps to its end. Shared with `lsp_nav` (EDITOR-LSP-3), which
/// resolves the definition / references / edit positions the same way.
pub(crate) fn resolve_pos(buffer: &Buffer, line: u32, utf16_col: u32) -> usize {
    let line = (line as usize).min(buffer.len_lines().saturating_sub(1));
    let line_start = buffer.line_to_char(line);
    let target = utf16_col as usize;
    let mut u16acc = 0usize;
    let mut chars = 0usize;
    for ch in buffer.rope().line(line).chars() {
        if ch == '\n' || ch == '\r' || u16acc >= target {
            break;
        }
        u16acc += ch.len_utf16();
        chars += 1;
    }
    (line_start + chars).min(buffer.len_chars())
}

/// A tiny status the chrome strip shows for the language-server session: the
/// text plus its tone color. Honest by construction (§7) — a gated
/// [`LspState::Unavailable`] surfaces the missing command, never a faked
/// "connected" session.
pub(crate) struct LspStatus {
    /// The short status text (e.g. `rust-analyzer: not found`).
    pub(crate) text: String,
    /// The tone color for the text.
    pub(crate) color: Color32,
}

/// The chrome status for `client`, or `None` when there is nothing honest to
/// say (a serverless language, or a cleanly stopped session).
#[must_use]
pub(crate) fn lsp_status(client: &LspClient) -> Option<LspStatus> {
    status_of(&client.state(), client.language())
}

/// The chrome status for a given [`LspState`] + language — the pure core of
/// [`lsp_status`], unit-testable without a live client.
#[must_use]
pub(crate) fn status_of(state: &LspState, language: Language) -> Option<LspStatus> {
    match state {
        // A serverless prose surface (Markdown) or a cleanly closed session has
        // no honest status worth the chrome clutter.
        LspState::NoServer { .. } | LspState::Stopped => None,
        // The honest gated state (§7): the binary is missing — name the command
        // the operator can install, never fake a session.
        LspState::Unavailable { cmd, .. } => Some(LspStatus {
            text: format!("{cmd}: not found"),
            color: Style::WARN,
        }),
        LspState::Failed { .. } => Some(LspStatus {
            text: "language server: error".to_owned(),
            color: Style::DANGER,
        }),
        LspState::Initializing => Some(LspStatus {
            text: "language server: starting…".to_owned(),
            color: Style::TEXT_DIM,
        }),
        // A live session: name the server, quietly.
        LspState::Running => Some(LspStatus {
            text: server_spec(language)
                .map_or_else(|| "language server".to_owned(), |s| s.program.to_owned()),
            color: Style::OK,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{lsp_status, resolve_pos, severity_color, status_of, DiagnosticsOverlay};
    use crate::buffer::Buffer;
    use crate::highlight::Language;
    use crate::lsp::{Diagnostic, LspClient, LspState, Severity};
    use mde_egui::Style;
    use std::path::Path;

    /// Build a diagnostic over a single line at the given UTF-16 columns.
    fn diag(severity: Severity, line: u32, c0: u32, c1: u32, msg: &str) -> Diagnostic {
        Diagnostic {
            severity,
            start_line: line,
            start_character: c0,
            end_line: line,
            end_character: c1,
            message: msg.to_owned(),
            source: None,
        }
    }

    #[test]
    fn severity_maps_to_distinct_status_tokens() {
        // Each severity resolves to a shared Style token (§4, no raw hex), and
        // the four are visually distinct so the gutter reads at a glance.
        assert_eq!(severity_color(Severity::Error), Style::DANGER);
        assert_eq!(severity_color(Severity::Warning), Style::WARN);
        assert_eq!(severity_color(Severity::Information), Style::ACCENT);
        assert_eq!(severity_color(Severity::Hint), Style::TEXT_DIM);
        let all = [
            severity_color(Severity::Error),
            severity_color(Severity::Warning),
            severity_color(Severity::Information),
            severity_color(Severity::Hint),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "severities must paint distinct colors");
            }
        }
    }

    #[test]
    fn overlay_resolves_a_diagnostic_to_its_line_and_char_range() {
        let buf = Buffer::from_text("abc\nhello\n");
        // Line 1 ("hello") starts at char 4; a warning over cols 1..4 = "ell".
        let d = diag(Severity::Warning, 1, 1, 4, "unused");
        let mut o = DiagnosticsOverlay::default();
        o.rebuild(1, &buf, std::slice::from_ref(&d));

        assert_eq!(
            o.severity_for_line(1),
            Some(Severity::Warning),
            "the gutter marks line 1"
        );
        assert_eq!(o.severity_for_line(0), None, "no marker on a clean line");
        let mark = o.marks().first().expect("one underline mark");
        assert_eq!(mark.chars, 5..8, "the underline covers 'ell'");
        assert_eq!(buf.rope().slice(mark.chars.clone()).to_string(), "ell");
        assert_eq!(mark.message, "unused");
    }

    #[test]
    fn worst_severity_wins_per_line() {
        let buf = Buffer::from_text("let x = 1;\n");
        let diags = [
            diag(Severity::Hint, 0, 0, 1, "hint"),
            diag(Severity::Error, 0, 4, 5, "error"),
        ];
        let mut o = DiagnosticsOverlay::default();
        o.rebuild(1, &buf, &diags);
        assert_eq!(
            o.severity_for_line(0),
            Some(Severity::Error),
            "the worst severity paints the line's gutter marker"
        );
    }

    #[test]
    fn utf16_columns_resolve_over_astral_chars() {
        // "😀" is one char but two UTF-16 code units, so a column of 2 lands on
        // the char *after* it, not two chars in.
        let buf = Buffer::from_text("😀xy\n");
        assert_eq!(resolve_pos(&buf, 0, 0), 0);
        assert_eq!(
            resolve_pos(&buf, 0, 2),
            1,
            "col 2 (past the 2-unit emoji) = char 1"
        );
        assert_eq!(resolve_pos(&buf, 0, 3), 2, "col 3 = the 'y'");
        // A column past the line content clamps to the line's end.
        assert_eq!(
            resolve_pos(&buf, 0, 99),
            3,
            "clamped to end of 'xy' content"
        );
    }

    #[test]
    fn the_epoch_gate_skips_rebuild_on_quiet_frames() {
        let buf = Buffer::from_text("a\nb\n");
        let diags = [diag(Severity::Error, 0, 0, 1, "e")];
        let mut o = DiagnosticsOverlay::default();

        // The panel's per-frame loop: only rebuild when the epoch moved.
        let mut rebuilds = 0;
        for epoch in [1_u64, 1, 1, 2] {
            if o.needs_refresh(epoch) {
                o.rebuild(epoch, &buf, &diags);
                rebuilds += 1;
            }
        }
        // Four frames at epochs 1,1,1,2 → exactly two rebuilds: the three
        // epoch-1 frames coalesce, epoch 2 (the republish) rebuilds once.
        assert_eq!(rebuilds, 2, "quiet frames skip the recompute");
        assert!(!o.needs_refresh(2), "the last build is current at epoch 2");
        assert!(o.needs_refresh(3), "a newer epoch needs a rebuild");
    }

    #[test]
    fn status_of_unavailable_is_honest_not_a_fake_session() {
        // §7: an absent binary names the command to install — never a fake
        // "connected" status.
        let s = status_of(
            &LspState::Unavailable {
                language: Language::Rust,
                cmd: "rust-analyzer".to_owned(),
            },
            Language::Rust,
        )
        .expect("unavailable shows a status");
        assert!(
            s.text.contains("rust-analyzer") && s.text.contains("not found"),
            "the status names the missing command: {}",
            s.text
        );
        assert_eq!(s.color, Style::WARN);
    }

    #[test]
    fn status_of_running_names_the_server_and_serverless_is_quiet() {
        let running = status_of(&LspState::Running, Language::Rust).expect("running status");
        assert_eq!(running.text, "rust-analyzer");
        assert_eq!(running.color, Style::OK);
        // A serverless language (Markdown) has no honest status to show.
        assert!(
            status_of(
                &LspState::NoServer {
                    language: Language::Markdown
                },
                Language::Markdown
            )
            .is_none(),
            "a prose surface shows no server clutter"
        );
    }

    #[test]
    fn lsp_status_reads_a_live_gated_client() {
        // A serverless client (Markdown → NoServer, no OS process) reports no
        // status — the honest quiet path, driven through the real client seam.
        let client = LspClient::start(Language::Markdown, Path::new("/tmp"));
        assert!(lsp_status(&client).is_none());
    }
}
