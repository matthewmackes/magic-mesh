//! EDTB-5 — **Print via CUPS**: paginate the open buffer as formatted monospace
//! text and shell it to `lp` (design: `docs/design/editor-toolbar.md`, the
//! Word-97 Standard-toolbar Print group).
//!
//! Two seams, split so the §7 guarantees are unit-testable without egui or a live
//! printer:
//!
//! * **Pagination** ([`paginate`]) — the pure engine that splits the document
//!   into fixed-height [`Page`]s of wrapped monospace rows, plus
//!   [`render_print_job`], which lays each page out with a filename header, the
//!   body, and a `Page N of M` footer, separating pages with a form feed so CUPS
//!   starts each on a fresh sheet. Both the on-screen **Print Preview**
//!   ([`draw_page`]) and the print job render from the *same* pages, so the
//!   preview is honest about what will print (§7).
//! * **Submission** ([`submit`]) — spawns `lp`, pipes the job to its stdin, and
//!   classifies the outcome into a real send or an honest [`PrintError`]: `lp`
//!   absent (no CUPS client), no printer configured, or another failure carrying
//!   `lp`'s own words. Never a silent no-op or a faked success (§7). The `lp`
//!   argument vector ([`lp_args`]) is a pure fn so the invocation is testable.
//!
//! Shell-out over a new crate dependency (task scope): CUPS' `lp` is the platform
//! print path, so no `cargo` dep is added.

use std::io::Write;
use std::process::{Command, Stdio};

use mde_egui::egui::{Align, Layout, RichText, Ui};
use mde_egui::Style;

/// The CUPS submission program — the standard System-V print command.
pub const LP: &str = "lp";

/// The form feed CUPS/raw-text printers treat as a hard page break, emitted
/// between pages in [`render_print_job`] so each [`Page`] starts a fresh sheet.
pub const FORM_FEED: char = '\u{000C}';

/// The tab width the monospace layout expands `\t` to before wrapping — a print
/// page has no live tab stops, so tabs become spaces for a stable column grid.
const TAB_WIDTH: usize = 4;

/// The monospace page geometry (US-Letter portrait at a common print size): the
/// body rows per page and the column at which a logical line wraps. `rows_per_page`
/// leaves headroom under the ~66-line sheet for the header + footer.
#[derive(Debug, Clone, Copy)]
pub struct PageLayout {
    /// Wrapped body rows per page (between the header and the footer).
    pub rows_per_page: usize,
    /// The monospace column count at which a logical line wraps to the next row.
    pub cols_per_row: usize,
}

impl Default for PageLayout {
    fn default() -> Self {
        Self {
            rows_per_page: 60,
            cols_per_row: 80,
        }
    }
}

/// One paginated page: its 1-based number and the wrapped monospace body rows
/// (header + footer are derived at render time from the filename + page total).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The 1-based page number.
    pub number: usize,
    /// The wrapped display rows of this page's body.
    pub rows: Vec<String>,
}

/// Wrap one logical line to `cols` monospace columns, hard-splitting on the column
/// boundary (a print page cannot scroll). An empty line yields one empty row so a
/// blank document line still occupies a row.
fn wrap_line(line: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(cols).map(|c| c.iter().collect()).collect()
}

/// Split `text` into fixed-height [`Page`]s of wrapped monospace rows (the pure
/// pagination engine, EDTB-5). Logical lines come from [`str::lines`] (a trailing
/// newline is a terminator, not a phantom blank line); tabs expand to spaces and
/// over-long lines wrap at [`PageLayout::cols_per_row`]. An empty document is one
/// blank page, never zero (§7 — the preview always has a page to show).
#[must_use]
pub fn paginate(text: &str, layout: &PageLayout) -> Vec<Page> {
    let mut rows: Vec<String> = Vec::new();
    for logical in text.lines() {
        let expanded = logical.replace('\t', &" ".repeat(TAB_WIDTH));
        rows.extend(wrap_line(&expanded, layout.cols_per_row));
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows.chunks(layout.rows_per_page.max(1))
        .enumerate()
        .map(|(i, chunk)| Page {
            number: i + 1,
            rows: chunk.to_vec(),
        })
        .collect()
}

/// The header line printed at the top of every page — the document filename.
fn page_header(filename: &str) -> String {
    filename.to_owned()
}

/// The footer line printed at the bottom of every page — `Page N of M`.
fn page_footer(number: usize, total: usize) -> String {
    format!("Page {number} of {total}")
}

/// Render the whole paginated document into the plain-text job piped to `lp`:
/// each page is a filename header, a blank line, the body (padded to
/// [`PageLayout::rows_per_page`] so the footer sits at the page foot), a blank
/// line, and the `Page N of M` footer; a [`FORM_FEED`] separates pages so each
/// prints on a fresh sheet. Pure over the pages, so the layout is testable.
#[must_use]
pub fn render_print_job(pages: &[Page], filename: &str, layout: &PageLayout) -> String {
    let total = pages.len();
    let mut out = String::new();
    for (i, page) in pages.iter().enumerate() {
        out.push_str(&page_header(filename));
        out.push('\n');
        out.push('\n');
        for row in &page.rows {
            out.push_str(row);
            out.push('\n');
        }
        // Pad the body so the footer lands at the foot of the sheet.
        for _ in page.rows.len()..layout.rows_per_page {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&page_footer(page.number, total));
        // A hard page break between pages (never after the last).
        if i + 1 < total {
            out.push(FORM_FEED);
        }
        out.push('\n');
    }
    out
}

/// The options for a single `lp` submission.
#[derive(Debug, Clone, Default)]
pub struct PrintOptions {
    /// The explicit destination printer (`lp -d`), or `None` for the CUPS default.
    pub printer: Option<String>,
    /// The job title (`lp -t`) — the name shown in the CUPS queue.
    pub title: String,
}

/// Build the `lp` argument vector for `opts` (pure — the invocation is testable
/// without spawning). `-d <printer>` only when a destination is named; `-t <title>`
/// only when a title is given; otherwise a bare `lp` (the CUPS default printer).
#[must_use]
pub fn lp_args(opts: &PrintOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(printer) = &opts.printer {
        args.push("-d".to_owned());
        args.push(printer.clone());
    }
    if !opts.title.is_empty() {
        args.push("-t".to_owned());
        args.push(opts.title.clone());
    }
    args
}

/// An honest failure to submit a print job (§7 — never a faked success).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrintError {
    /// `lp` is not installed — the CUPS client tools are absent.
    NoCups,
    /// `lp` ran but there is no usable destination (no default/named printer, or
    /// the CUPS scheduler is not running).
    NoPrinter,
    /// `lp` failed for another reason — carries its own stderr words.
    Failed(String),
}

impl PrintError {
    /// A short human notice for the status bar / Print Preview (§7).
    #[must_use]
    pub fn notice(&self) -> String {
        match self {
            Self::NoCups => "Cannot print: CUPS is not installed (no lp command)".to_owned(),
            Self::NoPrinter => "Cannot print: no printer configured in CUPS".to_owned(),
            Self::Failed(msg) => format!("Print failed: {msg}"),
        }
    }
}

/// Map an `lp` spawn error to an honest [`PrintError`]: a missing binary is the
/// no-CUPS state; anything else carries the OS message.
fn classify_spawn(err: &std::io::Error) -> PrintError {
    if err.kind() == std::io::ErrorKind::NotFound {
        PrintError::NoCups
    } else {
        PrintError::Failed(err.to_string())
    }
}

/// Classify a finished `lp` run into a send or an honest failure (pure over the
/// exit success + its stderr, so it is testable without a live printer). A
/// destination/scheduler complaint is the no-printer state; any other non-zero
/// exit surfaces `lp`'s own words (§7).
fn classify_exit(success: bool, stderr: &str) -> Result<(), PrintError> {
    if success {
        return Ok(());
    }
    let low = stderr.to_lowercase();
    if low.contains("no default destination")
        || low.contains("no destination")
        || low.contains("does not exist")
        || low.contains("scheduler is not running")
    {
        return Err(PrintError::NoPrinter);
    }
    let msg = stderr.trim();
    Err(PrintError::Failed(if msg.is_empty() {
        "lp exited with an error".to_owned()
    } else {
        msg.to_owned()
    }))
}

/// Submit `job` to CUPS through `program` (normally [`LP`]): spawn it with
/// [`lp_args`], pipe the job to its stdin, and classify the outcome. `Ok(())` means
/// the job was accepted onto the queue; an [`Err`] is one of the honest
/// [`PrintError`] states — never a silent no-op (§7).
///
/// # Errors
/// Returns [`PrintError::NoCups`] when `program` is not installed,
/// [`PrintError::NoPrinter`] when there is no usable destination, and
/// [`PrintError::Failed`] for any other `lp` error.
pub fn submit(program: &str, job: &str, opts: &PrintOptions) -> Result<(), PrintError> {
    let mut child = Command::new(program)
        .args(lp_args(opts))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| classify_spawn(&e))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A destination-less `lp` may exit before reading; the broken-pipe write
        // error is then not the real cause, so ignore it and let the exit status
        // + stderr classify the failure honestly below.
        let _ = stdin.write_all(job.as_bytes());
        // stdin drops here → EOF to `lp`.
    }
    let output = child
        .wait_with_output()
        .map_err(|e| PrintError::Failed(e.to_string()))?;
    classify_exit(
        output.status.success(),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Draw one page as a Construct-styled preview card (EDTB-5): a bordered plate over
/// the page background with a dimmed filename header, the monospace body, and a
/// right-aligned `Page N of M` footer — the same [`Page`] the print job renders,
/// so the preview is honest (§7). Carbon `Style` tokens throughout (§4).
pub fn draw_page(ui: &mut Ui, page: &Page, filename: &str, total: usize) {
    // A recessed bordered plate over the page background — the shared `inset()`
    // primitive (BG fill · hairline edge · tight corner · snug padding), so the
    // preview page's look reads only from `mde_egui` (§4).
    mde_egui::inset().show(ui, |ui| {
        ui.label(
            RichText::new(page_header(filename))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM)
                .monospace(),
        );
        ui.add_space(Style::SP_XS);
        // One multiline monospace label keeps the body cheap (one galley per
        // page) while preserving blank rows via the join.
        ui.label(
            RichText::new(page.rows.join("\n"))
                .size(Style::SMALL)
                .color(Style::TEXT)
                .monospace(),
        );
        ui.add_space(Style::SP_XS);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(page_footer(page.number, total))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM)
                    .monospace(),
            );
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{
        classify_exit, lp_args, paginate, render_print_job, submit, wrap_line, Page, PageLayout,
        PrintError, PrintOptions, FORM_FEED, LP,
    };

    fn layout(rows: usize, cols: usize) -> PageLayout {
        PageLayout {
            rows_per_page: rows,
            cols_per_row: cols,
        }
    }

    #[test]
    fn paginate_splits_a_buffer_into_pages_of_rows_per_page() {
        // 250 single lines at 60 rows/page → 5 pages (60·4 + 10).
        let text = (0..250)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let pages = paginate(&text, &layout(60, 80));
        assert_eq!(pages.len(), 5, "ceil(250/60) pages");
        for page in &pages[..4] {
            assert_eq!(page.rows.len(), 60, "full pages carry rows_per_page rows");
        }
        assert_eq!(pages[4].rows.len(), 10, "the last page holds the remainder");
        // Numbers are 1-based and contiguous, and no row is lost.
        assert_eq!(pages[0].number, 1);
        assert_eq!(pages[4].number, 5);
        let total_rows: usize = pages.iter().map(|p| p.rows.len()).sum();
        assert_eq!(
            total_rows, 250,
            "every logical line lands on exactly one row"
        );
    }

    #[test]
    fn paginate_wraps_over_long_lines_at_the_column_width() {
        // A 200-char line at 80 cols wraps to 3 rows (80 + 80 + 40).
        let text = "x".repeat(200);
        let rows = wrap_line(&text, 80);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].chars().count(), 80);
        assert_eq!(rows[2].chars().count(), 40);
        // …and those wrapped rows flow through pagination.
        let pages = paginate(&text, &layout(2, 80));
        assert_eq!(pages.len(), 2, "3 wrapped rows over 2 rows/page → 2 pages");
    }

    #[test]
    fn paginate_empty_document_is_one_blank_page_not_zero() {
        let pages = paginate("", &PageLayout::default());
        assert_eq!(pages.len(), 1, "an empty document still previews one page");
        assert_eq!(pages[0].rows, vec![String::new()]);
    }

    #[test]
    fn a_trailing_newline_is_a_terminator_not_a_phantom_line() {
        // "a\nb\n" is two lines, not three — the final newline terminates line 2.
        let pages = paginate("a\nb\n", &layout(60, 80));
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].rows, vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn lp_args_default_is_a_bare_lp() {
        assert!(
            lp_args(&PrintOptions::default()).is_empty(),
            "no destination + no title → a bare `lp` (the CUPS default printer)"
        );
    }

    #[test]
    fn lp_args_carry_the_printer_and_title() {
        let opts = PrintOptions {
            printer: Some("HP_LaserJet".to_owned()),
            title: "main.rs".to_owned(),
        };
        assert_eq!(
            lp_args(&opts),
            vec!["-d", "HP_LaserJet", "-t", "main.rs"],
            "the right `lp -d <printer> -t <title>` invocation"
        );
    }

    #[test]
    fn render_print_job_form_feeds_between_pages_and_footers_each() {
        let pages = vec![
            Page {
                number: 1,
                rows: vec!["one".to_owned()],
            },
            Page {
                number: 2,
                rows: vec!["two".to_owned()],
            },
        ];
        let job = render_print_job(&pages, "notes.txt", &layout(3, 80));
        assert_eq!(
            job.matches(FORM_FEED).count(),
            1,
            "exactly one page break between two pages (never a trailing one)"
        );
        assert!(job.contains("Page 1 of 2"), "page 1 footer present");
        assert!(job.contains("Page 2 of 2"), "page 2 footer present");
        assert!(job.contains("notes.txt"), "the filename header is printed");
        assert!(
            job.contains("one") && job.contains("two"),
            "both bodies present"
        );
    }

    #[test]
    fn submit_with_a_missing_lp_is_the_honest_no_cups_state() {
        // The no-CUPS path (§7): spawning a non-existent print command classifies
        // to NoCups, not a silent no-op or a faked success — deterministic and
        // side-effect-free (spawn fails before any process starts).
        let err = submit(
            "mcnf-no-such-lp-binary-xyzzy",
            "job",
            &PrintOptions::default(),
        )
        .expect_err("a missing lp must be an honest error");
        assert_eq!(err, PrintError::NoCups);
        assert!(err.notice().to_lowercase().contains("cups"));
    }

    #[test]
    fn classify_exit_maps_a_no_destination_to_no_printer() {
        assert_eq!(
            classify_exit(false, "lp: Error - no default destination available."),
            Err(PrintError::NoPrinter),
        );
        // A scheduler-down complaint is likewise the no-printer honest state.
        assert_eq!(
            classify_exit(false, "lp: Error - The scheduler is not running."),
            Err(PrintError::NoPrinter),
        );
    }

    #[test]
    fn classify_exit_success_is_ok_and_other_errors_carry_lps_words() {
        assert_eq!(classify_exit(true, ""), Ok(()));
        assert_eq!(
            classify_exit(false, "lp: some other failure"),
            Err(PrintError::Failed("lp: some other failure".to_owned())),
        );
    }

    #[test]
    fn print_errors_have_named_notices() {
        // §7 — every failure state is a named notice, not a blank.
        assert!(!PrintError::NoCups.notice().is_empty());
        assert!(!PrintError::NoPrinter.notice().is_empty());
        assert!(PrintError::Failed("boom".to_owned())
            .notice()
            .contains("boom"));
    }

    #[test]
    fn lp_is_the_system_v_print_command() {
        assert_eq!(LP, "lp");
    }
}
