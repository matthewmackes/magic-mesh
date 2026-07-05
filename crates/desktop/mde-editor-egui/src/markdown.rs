//! EDTB-7 — the **split markdown preview** for `mde-editor-egui`.
//!
//! A tiny self-contained markdown subset parser + its egui render: the "other
//! half" of the editor when the View → Preview toggle is on for a markdown /
//! plain-text buffer.
//!
//! No markdown crate is a workspace dependency (the editor's `tree-sitter-md`
//! is a *source highlighter*, not a renderer), so — as the unit allows — this is
//! a small hand-rolled subset covering the elements the acceptance names:
//! **headings, bold, italic, strikethrough, inline code, bullet + numbered
//! lists, block quotes, fenced code blocks, tables, and horizontal rules**. It is
//! deliberately two clean halves:
//!
//! * [`parse`] turns the buffer text into a `Vec` of [`Block`]s — pure, no egui,
//!   so the **markdown → styled-block mapping** is unit-tested without a GPU
//!   (headings resolve their level, `**x**` → a bold [`Span`], a pipe table → a
//!   [`Table`], …). This is the tested contract the acceptance asks for.
//! * [`show`] paints those blocks through the shared Carbon look ONLY: every
//!   colour is a [`Style`] / [`mde_egui::code`] token and every point size is a
//!   [`Style`] type-scale rung ([`Style::heading_size`], `BODY`, …), so the
//!   preview carries no raw hex and no literal size (§4). The one embedded face
//!   (Droid Sans Mono) has no bold cut, so *weight* is the brighter
//!   [`Style::TEXT_STRONG`] tone and *italic* is egui's real synthetic slant —
//!   the honest cues a mono face can give (§7).
//!
//! The panel re-parses only when the buffer revision moves (the debounce lives on
//! `Doc` in `panel.rs`, mirroring the EDTB-6 spell pass), so live typing reflects
//! in the preview without a per-keystroke full-document re-parse.

use std::path::Path;

use mde_egui::code;
use mde_egui::egui::{
    self, text::LayoutJob, Align, FontFamily, FontId, RichText, Stroke, TextFormat, Ui,
};
use mde_egui::Style;

/// Whether `path`'s buffer gets the split preview (EDTB-7 — **markdown / text
/// first**, the acceptance's scope).
///
/// A pathless scratch buffer (a fresh note),
/// an extension-less file (`README`, `NOTES`), or a `.md`/`.markdown`/`.txt`/
/// `.text` file qualifies; a recognised code language (`.rs`, `.py`, …) does not
/// — its View → Preview toggle greys out honestly (§7). The same prose-vs-code
/// split the spell pass draws, kept its own predicate so the two features stay
/// independent.
#[must_use]
pub fn is_previewable(path: Option<&Path>) -> bool {
    let Some(path) = path else {
        return true; // a scratch buffer is prose until saved with a code type
    };
    path.extension().and_then(|e| e.to_str()).is_none_or(|ext| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "txt" | "text"
        )
    })
}

/// The inline emphasis carried by one run of text.
///
/// The flags a [`Span`] resolves from its surrounding markers (`**bold**`,
/// `*italic*`, `~~strike~~`, `` `code` ``). Independent bits: `**_x_**` is both
/// bold and italic.
// `struct_excessive_bools`: these four are independent inline styles that stack
// (bold+italic+strike coexist), not a disguised state machine — an enum would
// misrepresent flags that vary independently.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Emphasis {
    /// Wrapped in `**` / `__` (or the `***`/`___` triple).
    pub bold: bool,
    /// Wrapped in `*` / `_` (or the triple).
    pub italic: bool,
    /// A `` `code` `` span — rendered monospace, its inner markers left literal.
    pub code: bool,
    /// Wrapped in `~~` — struck through.
    pub strike: bool,
}

impl Emphasis {
    /// This emphasis with the bit(s) `marker` sets turned on (the recursion step
    /// as the inline scanner descends into a wrapped run).
    #[must_use]
    fn with_marker(self, marker: &str) -> Self {
        match marker {
            "***" | "___" => Self {
                bold: true,
                italic: true,
                ..self
            },
            "**" | "__" => Self { bold: true, ..self },
            "*" | "_" => Self {
                italic: true,
                ..self
            },
            "~~" => Self {
                strike: true,
                ..self
            },
            _ => self,
        }
    }

    /// This emphasis marked as an inline-code run.
    #[must_use]
    const fn with_code(self) -> Self {
        Self { code: true, ..self }
    }
}

/// One inline run: its text and the emphasis it renders with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// The run's literal text (markers stripped).
    pub text: String,
    /// The emphasis to paint it with.
    pub emphasis: Emphasis,
}

impl Span {
    /// A plain, emphasis-free run — the test constructor for the parse-mapping
    /// assertions (the parser builds spans through [`push_plain`] directly).
    #[cfg(test)]
    #[must_use]
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            emphasis: Emphasis::default(),
        }
    }
}

/// One table cell — its inline spans (a cell is itself inline-formatted markdown).
pub type Cell = Vec<Span>;

/// A parsed pipe table: a header row plus zero or more body rows, each a row of
/// inline [`Cell`]s (EDTB-7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// The header row's cells.
    pub headers: Vec<Cell>,
    /// The body rows, each a row of cells.
    pub rows: Vec<Vec<Cell>>,
}

/// One list item (EDTB-7): its bullet/number marker for display, the nesting
/// level (two leading spaces per level), whether it is an ordered item, and its
/// inline spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    /// True for a `1.` numbered item, false for a `-`/`*`/`+` bullet.
    pub ordered: bool,
    /// Indent depth (two spaces per level); `0` is the outer list.
    pub level: usize,
    /// The display marker: `•` for a bullet, the original `N.` for an ordered item.
    pub marker: String,
    /// The item's inline content.
    pub spans: Vec<Span>,
}

/// One rendered block in the preview — the parsed markdown document is a flat
/// `Vec` of these (EDTB-7). Flat (each list item its own block) keeps the parse
/// and the render one straight loop each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// An ATX heading (`#`..`######`), its level `1..=6` and inline title.
    Heading {
        /// The heading level, `1` (largest) .. `6`.
        level: u8,
        /// The inline title spans.
        spans: Vec<Span>,
    },
    /// A paragraph of inline spans (consecutive non-blank text lines merged).
    Paragraph(Vec<Span>),
    /// A `>` block quote (consecutive quote lines merged), inline-formatted.
    Quote(Vec<Span>),
    /// One list item.
    Item(ListItem),
    /// A fenced (```` ``` ````) code block — its literal inner text, unformatted.
    Code(String),
    /// A pipe table.
    Table(Table),
    /// A `---` / `***` / `___` horizontal rule.
    Rule,
}

// ── Parsing (pure; the tested markdown → block mapping) ─────────────────────

/// Parse `text` into the flat block list the preview renders (EDTB-7).
///
/// A small markdown subset — headings, emphasis, lists, quotes, fenced code,
/// tables, rules — enough for the acceptance's elements; anything else falls
/// through as a paragraph (§7 — honest plain text, never dropped).
#[must_use]
pub fn parse(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim_end();
        let lead = line.trim_start();

        // A fenced code block swallows everything up to its closing fence.
        if is_fence(lead) {
            flush_paragraph(&mut para, &mut blocks);
            i += 1;
            let mut code = String::new();
            while i < lines.len() && !is_fence(lines[i].trim_start()) {
                code.push_str(lines[i]);
                code.push('\n');
                i += 1;
            }
            if i < lines.len() {
                i += 1; // consume the closing fence
            }
            while code.ends_with('\n') {
                code.pop();
            }
            blocks.push(Block::Code(code));
            continue;
        }

        // A blank line ends the current paragraph.
        if line.trim().is_empty() {
            flush_paragraph(&mut para, &mut blocks);
            i += 1;
            continue;
        }

        // ATX heading.
        if let Some((level, rest)) = atx_heading(lead) {
            flush_paragraph(&mut para, &mut blocks);
            blocks.push(Block::Heading {
                level,
                spans: parse_inline(rest),
            });
            i += 1;
            continue;
        }

        // A pipe table: a `|` row immediately followed by a `---` separator row.
        if lead.contains('|') && i + 1 < lines.len() && is_table_separator(lines[i + 1].trim()) {
            flush_paragraph(&mut para, &mut blocks);
            let headers = parse_table_row(lead);
            i += 2; // header + separator
            let mut rows = Vec::new();
            while i < lines.len() {
                let row = lines[i].trim();
                if row.is_empty() || !row.contains('|') {
                    break;
                }
                rows.push(parse_table_row(row));
                i += 1;
            }
            blocks.push(Block::Table(Table { headers, rows }));
            continue;
        }

        // A horizontal rule (checked after tables, so `|---|` never matches here).
        if is_rule(lead) {
            flush_paragraph(&mut para, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }

        // A block quote merges its consecutive `>` lines.
        if let Some(first) = block_quote(lead) {
            flush_paragraph(&mut para, &mut blocks);
            let mut text = first.to_owned();
            i += 1;
            while i < lines.len() {
                let Some(more) = block_quote(lines[i].trim_start()) else {
                    break;
                };
                text.push(' ');
                text.push_str(more);
                i += 1;
            }
            blocks.push(Block::Quote(parse_inline(&text)));
            continue;
        }

        // A list item.
        if let Some(item) = list_item(line) {
            flush_paragraph(&mut para, &mut blocks);
            blocks.push(Block::Item(item));
            i += 1;
            continue;
        }

        // Otherwise: a paragraph line (merged with its neighbours below).
        para.push(line);
        i += 1;
    }
    flush_paragraph(&mut para, &mut blocks);
    blocks
}

/// Flush the accumulated paragraph lines (joined with a space, markdown's soft
/// wrap) into one [`Block::Paragraph`], clearing the buffer. A no-op when empty.
fn flush_paragraph(para: &mut Vec<&str>, blocks: &mut Vec<Block>) {
    if para.is_empty() {
        return;
    }
    let joined = para.join(" ");
    para.clear();
    blocks.push(Block::Paragraph(parse_inline(&joined)));
}

/// Whether `lead` opens/closes a fenced code block (three backticks, or `~~~`).
fn is_fence(lead: &str) -> bool {
    lead.starts_with("```") || lead.starts_with("~~~")
}

/// The ATX heading level + trimmed title of `lead`, or `None`. A heading is a
/// leading run of 1–6 `#` followed by a space (or the bare `#` line); a trailing
/// `#`-run (Word's closed ATX form) is stripped.
fn atx_heading(lead: &str) -> Option<(u8, &str)> {
    let hashes = lead.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &lead[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // `#word` is not a heading (no space) — a hashtag, say
    }
    let title = rest.trim().trim_end_matches('#').trim_end();
    #[allow(clippy::cast_possible_truncation)]
    Some((hashes as u8, title))
}

/// Whether `lead` is a horizontal rule: three-or-more of a single `-`/`*`/`_`,
/// spaces allowed between them and nothing else.
fn is_rule(lead: &str) -> bool {
    let compact: String = lead.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && (compact.chars().all(|c| c == '-')
            || compact.chars().all(|c| c == '*')
            || compact.chars().all(|c| c == '_'))
}

/// Whether `row` is a table separator line (`|---|:--:|`): it has a `-` and every
/// char is one of `| - : ` (space).
fn is_table_separator(row: &str) -> bool {
    row.contains('-') && row.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Split a `|`-delimited table row into inline-parsed cells, dropping the empty
/// edges a leading/trailing pipe leaves.
fn parse_table_row(row: &str) -> Vec<Cell> {
    let row = row.trim();
    let inner = row.strip_prefix('|').unwrap_or(row);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(|c| parse_inline(c.trim())).collect()
}

/// The text after a `>` quote marker on `lead`, or `None` when it is not a quote.
fn block_quote(lead: &str) -> Option<&str> {
    lead.strip_prefix('>')
        .map(|r| r.strip_prefix(' ').unwrap_or(r))
}

/// Parse a list item from `raw` (indent preserved), or `None`. Handles `-`/`*`/`+`
/// bullets and `N.`/`N)` ordered items; the nesting level is two leading spaces
/// per level.
fn list_item(raw: &str) -> Option<ListItem> {
    let indent = raw.len() - raw.trim_start().len();
    let body = raw.trim_start();
    let level = indent / 2;

    // Bullet: `- ` / `* ` / `+ `.
    if let Some(rest) = body
        .strip_prefix("- ")
        .or_else(|| body.strip_prefix("* "))
        .or_else(|| body.strip_prefix("+ "))
    {
        return Some(ListItem {
            ordered: false,
            level,
            marker: "\u{2022}".to_owned(),
            spans: parse_inline(rest.trim_start()),
        });
    }

    // Ordered: `<digits>.` / `<digits>)` then a space.
    let digits: String = body.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let after = &body[digits.len()..];
        if let Some(rest) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            return Some(ListItem {
                ordered: true,
                level,
                marker: format!("{digits}."),
                spans: parse_inline(rest.trim_start()),
            });
        }
    }
    None
}

// ── Inline emphasis scanning ────────────────────────────────────────────────

/// Parse one line/paragraph's text into emphasis [`Span`]s (EDTB-7).
///
/// Recursive: a matched marker pair descends into its inner text with the added
/// emphasis; an unmatched marker stays literal (§7 — never eats the rest of the
/// line).
#[must_use]
pub fn parse_inline(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    scan_inline(text, Emphasis::default(), &mut out);
    coalesce(&mut out);
    out
}

/// The inline markers, longest first so `**` wins over `*` at the same spot.
const MARKERS: [&str; 8] = ["***", "___", "**", "__", "~~", "`", "*", "_"];

/// The marker starting exactly at byte `idx` in `text`, or `None`. An underscore
/// marker is skipped when it sits *inside* a word (`snake_case`), so code-ish
/// prose does not turn italic.
fn marker_at(text: &str, idx: usize) -> Option<&'static str> {
    let rest = &text[idx..];
    for m in MARKERS {
        if rest.starts_with(m) {
            if m.starts_with('_')
                && text[..idx]
                    .chars()
                    .last()
                    .is_some_and(char::is_alphanumeric)
            {
                continue; // intraword underscore — literal, not emphasis
            }
            return Some(m);
        }
    }
    None
}

/// Scan `text` under the ambient `emph`, appending spans to `out`.
fn scan_inline(text: &str, emph: Emphasis, out: &mut Vec<Span>) {
    let mut plain = String::new();
    let mut idx = 0;
    while idx < text.len() {
        if let Some(marker) = marker_at(text, idx) {
            let open_end = idx + marker.len();
            if let Some(rel) = text[open_end..].find(marker) {
                if rel > 0 {
                    // A non-empty matched pair: emit the pending plain run, then
                    // the wrapped inner (code stays literal, everything else
                    // recurses with the added emphasis).
                    push_plain(&mut plain, emph, out);
                    let inner = &text[open_end..open_end + rel];
                    if marker == "`" {
                        out.push(Span {
                            text: inner.to_owned(),
                            emphasis: emph.with_code(),
                        });
                    } else {
                        scan_inline(inner, emph.with_marker(marker), out);
                    }
                    idx = open_end + rel + marker.len();
                    continue;
                }
            }
            // No close (or an empty pair): the marker is literal text.
            plain.push_str(marker);
            idx = open_end;
        } else {
            let ch = text[idx..].chars().next().unwrap_or('\u{0}');
            plain.push(ch);
            idx += ch.len_utf8();
        }
    }
    push_plain(&mut plain, emph, out);
}

/// Emit the pending plain run (if any) as a span with the ambient emphasis,
/// clearing the buffer.
fn push_plain(plain: &mut String, emph: Emphasis, out: &mut Vec<Span>) {
    if !plain.is_empty() {
        out.push(Span {
            text: std::mem::take(plain),
            emphasis: emph,
        });
    }
}

/// Merge adjacent spans that share an emphasis (the scanner can emit runs that
/// abut) so the span list is minimal — steadier to assert against and to paint.
fn coalesce(spans: &mut Vec<Span>) {
    let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        if let Some(last) = merged.last_mut() {
            if last.emphasis == span.emphasis {
                last.text.push_str(&span.text);
                continue;
            }
        }
        merged.push(span);
    }
    *spans = merged;
}

// ── Rendering (token-styled; the shared Carbon look) ────────────────────────

/// Render the parsed `blocks` into `ui` as the split preview (EDTB-7). Scrolls
/// vertically for a long document; every colour + size is a shared token (§4).
pub fn show(ui: &mut Ui, blocks: &[Block]) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (idx, block) in blocks.iter().enumerate() {
                render_block(ui, idx, block);
            }
        });
}

/// The colour a span paints with under `strong_base` (headings paint their whole
/// title strong): inline code is the shared code-string green, bold is the
/// brighter emphasis tone, everything else the body foreground — all tokens (§4).
const fn span_color(emph: Emphasis, strong_base: bool) -> egui::Color32 {
    if emph.code {
        code::STRING
    } else if emph.bold || strong_base {
        Style::TEXT_STRONG
    } else {
        Style::TEXT
    }
}

/// Append `spans` to `job` at `size`, mapping each emphasis to a token-styled
/// [`TextFormat`] (`strong_base` forces the strong tone for whole-title runs).
fn append_spans(job: &mut LayoutJob, spans: &[Span], size: f32, strong_base: bool) {
    for span in spans {
        let color = span_color(span.emphasis, strong_base);
        let family = if span.emphasis.code {
            FontFamily::Monospace
        } else {
            FontFamily::Proportional
        };
        let mut fmt = TextFormat {
            font_id: FontId::new(size, family),
            color,
            italics: span.emphasis.italic,
            ..Default::default()
        };
        if span.emphasis.strike {
            fmt.strikethrough = Stroke::new(1.0, color);
        }
        if span.emphasis.code {
            fmt.background = Style::SURFACE;
        }
        job.append(&span.text, 0.0, fmt);
    }
}

/// Build a width-wrapped [`LayoutJob`] for `spans` at `size`.
fn spans_job(ui: &Ui, spans: &[Span], size: f32, strong_base: bool) -> LayoutJob {
    let mut job = LayoutJob {
        halign: Align::LEFT,
        ..Default::default()
    };
    job.wrap.max_width = ui.available_width();
    append_spans(&mut job, spans, size, strong_base);
    job
}

/// Render one block (`idx` gives table grids a stable id).
fn render_block(ui: &mut Ui, idx: usize, block: &Block) {
    match block {
        Block::Heading { level, spans } => {
            ui.add_space(Style::SP_S);
            let job = spans_job(ui, spans, Style::heading_size(*level), true);
            ui.label(job);
            ui.add_space(Style::SP_XS);
        }
        Block::Paragraph(spans) => {
            let job = spans_job(ui, spans, Style::BODY, false);
            ui.label(job);
            ui.add_space(Style::SP_XS);
        }
        Block::Quote(spans) => {
            egui::Frame::default()
                .fill(Style::SURFACE)
                .inner_margin(Style::SP_S)
                .show(ui, |ui| {
                    let mut job = spans_job(ui, spans, Style::BODY, false);
                    // A quote reads a touch quieter than body prose.
                    for section in &mut job.sections {
                        if section.format.color == Style::TEXT {
                            section.format.color = Style::TEXT_DIM;
                        }
                    }
                    ui.label(job);
                });
            ui.add_space(Style::SP_XS);
        }
        Block::Item(item) => render_item(ui, item),
        Block::Code(text) => {
            egui::Frame::default()
                .fill(Style::SURFACE)
                .inner_margin(Style::SP_S)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(text)
                            .monospace()
                            .size(Style::BODY)
                            .color(code::PLAIN),
                    );
                });
            ui.add_space(Style::SP_XS);
        }
        Block::Table(table) => {
            render_table(ui, idx, table);
            ui.add_space(Style::SP_XS);
        }
        Block::Rule => {
            ui.add_space(Style::SP_XS);
            ui.separator();
            ui.add_space(Style::SP_XS);
        }
    }
}

/// Render one list item: its indent, its dim marker, then its inline content.
fn render_item(ui: &mut Ui, item: &ListItem) {
    #[allow(clippy::cast_precision_loss)]
    let indent = Style::SP_M.mul_add(item.level as f32, Style::SP_S);
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.label(
            RichText::new(&item.marker)
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
        let job = spans_job(ui, &item.spans, Style::BODY, false);
        ui.label(job);
    });
}

/// Render a pipe table as a token-styled grid: a strong header row over the body
/// rows, striped through the shared theme.
fn render_table(ui: &mut Ui, idx: usize, table: &Table) {
    egui::Grid::new(("md-preview-table", idx))
        .striped(true)
        .spacing(egui::vec2(Style::SP_M, Style::SP_XS))
        .show(ui, |ui| {
            for cell in &table.headers {
                let job = spans_job(ui, cell, Style::BODY, true);
                ui.label(job);
            }
            ui.end_row();
            for row in &table.rows {
                for cell in row {
                    let job = spans_job(ui, cell, Style::BODY, false);
                    ui.label(job);
                }
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{is_previewable, parse, parse_inline, Block, Emphasis, ListItem, Span};
    use mde_egui::Style;
    use std::path::Path;

    /// The single inline span `parse_inline` should yield for a one-run line.
    fn only_span(text: &str) -> Span {
        let spans = parse_inline(text);
        assert_eq!(spans.len(), 1, "expected one span for {text:?}: {spans:?}");
        spans.into_iter().next().unwrap()
    }

    #[test]
    fn headings_resolve_their_level_and_map_to_the_shared_type_ramp() {
        // The markdown → styled mapping the acceptance names: a `#`-run resolves
        // its level and the render sizes it through the ONE shared ramp — a
        // deeper heading is never larger (H1 is the display rung).
        for level in 1..=6u8 {
            let hashes = "#".repeat(level as usize);
            let blocks = parse(&format!("{hashes} Title {level}"));
            assert_eq!(
                blocks,
                vec![Block::Heading {
                    level,
                    spans: vec![Span::plain(format!("Title {level}"))],
                }],
                "H{level} parses to a heading of that level",
            );
        }
        // The render sizes every level through the ONE shared ramp — a deeper
        // heading is never larger (H1 is the display rung).
        assert!(
            Style::heading_size(1) >= Style::heading_size(6),
            "H1 is at least as large as H6",
        );
        // `#word` (no space) is a hashtag, not a heading.
        assert_eq!(
            parse("#notaheading"),
            vec![Block::Paragraph(vec![Span::plain("#notaheading")])]
        );
    }

    #[test]
    fn bold_italic_strike_and_code_each_set_their_emphasis_bit() {
        assert_eq!(
            only_span("**bold**").emphasis,
            Emphasis {
                bold: true,
                ..Default::default()
            },
        );
        assert_eq!(
            only_span("*italic*").emphasis,
            Emphasis {
                italic: true,
                ..Default::default()
            },
        );
        assert_eq!(
            only_span("~~gone~~").emphasis,
            Emphasis {
                strike: true,
                ..Default::default()
            },
        );
        let code = only_span("`snippet`");
        assert_eq!(code.text, "snippet");
        assert!(code.emphasis.code, "backticks mark an inline code run");
        // The triple sets both weight bits.
        let both = only_span("***loud***").emphasis;
        assert!(both.bold && both.italic, "*** is bold + italic");
    }

    #[test]
    fn emphasis_sits_inline_among_plain_text() {
        let spans = parse_inline("a **b** c");
        assert_eq!(
            spans,
            vec![
                Span::plain("a "),
                Span {
                    text: "b".to_owned(),
                    emphasis: Emphasis {
                        bold: true,
                        ..Default::default()
                    },
                },
                Span::plain(" c"),
            ],
        );
    }

    #[test]
    fn an_unmatched_marker_and_a_snake_case_underscore_stay_literal() {
        // §7 — a lone `*` never swallows the rest of the line…
        assert_eq!(parse_inline("2 * 3 = 6"), vec![Span::plain("2 * 3 = 6")]);
        // …and an intraword underscore is not italic (code-ish prose stays put).
        assert_eq!(
            parse_inline("call snake_case_name here"),
            vec![Span::plain("call snake_case_name here")],
        );
    }

    #[test]
    fn bullet_and_numbered_lists_parse_to_items() {
        let blocks = parse("- one\n- two");
        assert_eq!(
            blocks,
            vec![
                Block::Item(ListItem {
                    ordered: false,
                    level: 0,
                    marker: "\u{2022}".to_owned(),
                    spans: vec![Span::plain("one")],
                }),
                Block::Item(ListItem {
                    ordered: false,
                    level: 0,
                    marker: "\u{2022}".to_owned(),
                    spans: vec![Span::plain("two")],
                }),
            ],
        );
        let ordered = parse("1. first\n2. second");
        assert_eq!(
            ordered,
            vec![
                Block::Item(ListItem {
                    ordered: true,
                    level: 0,
                    marker: "1.".to_owned(),
                    spans: vec![Span::plain("first")],
                }),
                Block::Item(ListItem {
                    ordered: true,
                    level: 0,
                    marker: "2.".to_owned(),
                    spans: vec![Span::plain("second")],
                }),
            ],
        );
        // A nested (two-space) bullet lands one level deeper.
        let nested = parse("  - deep");
        assert_eq!(
            nested,
            vec![Block::Item(ListItem {
                ordered: false,
                level: 1,
                marker: "\u{2022}".to_owned(),
                spans: vec![Span::plain("deep")],
            })],
        );
    }

    #[test]
    fn a_pipe_table_parses_its_header_and_body_rows() {
        let blocks = parse("| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |");
        let Block::Table(table) = &blocks[0] else {
            unreachable!("expected a table, got {blocks:?}");
        };
        assert_eq!(
            table.headers,
            vec![vec![Span::plain("a")], vec![Span::plain("b")]],
            "two header cells",
        );
        assert_eq!(table.rows.len(), 2, "two body rows");
        assert_eq!(
            table.rows[1],
            vec![vec![Span::plain("3")], vec![Span::plain("4")]]
        );
        // Cells are inline-formatted: `**x**` in a cell is bold.
        let rich = parse("| **h** |\n| - |\n| x |");
        let Block::Table(t) = &rich[0] else {
            unreachable!("expected a table");
        };
        assert!(t.headers[0][0].emphasis.bold, "a table cell formats inline");
    }

    #[test]
    fn fenced_code_and_rules_and_quotes_parse() {
        let code = parse("```\nlet x = 1;\nlet y = 2;\n```");
        assert_eq!(code, vec![Block::Code("let x = 1;\nlet y = 2;".to_owned())]);
        assert_eq!(parse("---"), vec![Block::Rule]);
        assert_eq!(parse("***"), vec![Block::Rule]);
        assert_eq!(
            parse("> quoted\n> line"),
            vec![Block::Quote(vec![Span::plain("quoted line")])],
        );
    }

    #[test]
    fn consecutive_text_lines_merge_into_one_paragraph() {
        assert_eq!(
            parse("one\ntwo\n\nthree"),
            vec![
                Block::Paragraph(vec![Span::plain("one two")]),
                Block::Paragraph(vec![Span::plain("three")]),
            ],
        );
    }

    #[test]
    fn previewable_is_markdown_and_text_first_not_code() {
        assert!(is_previewable(None), "a scratch buffer previews as prose");
        assert!(is_previewable(Some(Path::new("notes.md"))));
        assert!(is_previewable(Some(Path::new("readme.txt"))));
        assert!(
            is_previewable(Some(Path::new("README"))),
            "no extension is prose"
        );
        assert!(
            !is_previewable(Some(Path::new("main.rs"))),
            "code is not previewable"
        );
        assert!(!is_previewable(Some(Path::new("app.py"))));
    }
}
