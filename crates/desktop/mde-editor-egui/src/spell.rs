//! EDTB-6 — **spell-check via `hunspell`**: red wavy underlines under misspelled
//! words in the editor widget, plus the `F7` walk dialog (design: the Word-97
//! Tools → Spelling group).
//!
//! Like [`print`](crate::print) (EDTB-5), the feature shells out to a standard
//! system tool rather than adding a crate dependency: the ispell-compatible pipe
//! `hunspell -a`. **Packaging note:** the RPM that ships the editor should carry
//! `Requires: hunspell` **and a default dictionary** (e.g. `hunspell-en-US`);
//! `hunspell -a` needs an installed dictionary to flag anything. When neither is
//! present the feature parks in the honest [`SpellState::Unavailable`] state — a
//! greyed control + a truthful "hunspell not installed" note, never a crash and
//! never a fake underline (§7).
//!
//! The module is **egui-free** — pure state + engine over `&str`, so every §7
//! guarantee is unit-testable without a frame or a live `hunspell`:
//!
//! * **Tokenising** ([`tokenize`]) — split a prose buffer into spell-checkable
//!   word spans (rope **char** ranges), skipping code-ish tokens (anything
//!   touching a digit / underscore) so a `v2` or an identifier is never flagged.
//! * **Parsing** ([`parse_hunspell_line`] / [`parse_response`]) — classify each
//!   `hunspell -a` reply line (`*`/`+`/`-` = in-dictionary, `&`/`?` = miss with
//!   suggestions, `#` = miss with none) and fold the misses into a
//!   `word → suggestions` map. The map is keyed by the word `hunspell` echoes, so
//!   resolution never depends on a fragile character offset.
//! * **Resolving** ([`resolve_spans`]) — map every occurrence of a missed word
//!   back to its span, yielding the [`SpellMiss`]es the widget underlines and the
//!   walk steps through.
//! * **Running** ([`run_hunspell`]) — feed the buffer's unique words to
//!   `hunspell -a` off the paint thread ([`SpellWorker`]) and classify the
//!   outcome: a missing binary is [`SpellState::Unavailable`], never a silent
//!   no-op or a faked success.
//!
//! [`SpellChecker`] is the per-document seam the panel owns (the spell analogue
//! of the LSP `DiagnosticsOverlay`): it debounces a background check on the
//! buffer revision, holds the resolved misses, and applies session
//! Ignore-All / Add-to-dictionary as an in-app filter over the last result (so
//! toggling a word never re-runs `hunspell`).

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::ops::Range;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

/// The spell-check program — the standard ispell-compatible checker.
pub const HUNSPELL: &str = "hunspell";

/// The `hunspell` flag for the ispell **pipe** protocol (one reply block per
/// input line) — the seam [`parse_response`] reads.
pub const PIPE_FLAG: &str = "-a";

/// Buffers larger than this are skipped (no squiggles, no UI hitch) — the spell
/// pass is a prose aid, not a whole-repo linter, so an enormous paste stays
/// responsive rather than feeding megabytes to the subprocess.
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// Whether `hunspell` is usable — the honest absent-state (§7), the spell
/// analogue of [`PrintError`](crate::print::PrintError)'s no-CUPS state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellState {
    /// `hunspell` is on `PATH` and can run — the feature drives squiggles.
    Ready,
    /// `hunspell` is not installed — the feature greys out with a truthful note,
    /// never a fake underline (§7).
    Unavailable,
}

impl SpellState {
    /// Whether the checker may run.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// A short human notice for the status strip when the feature is off (§7).
    #[must_use]
    pub const fn notice(self) -> &'static str {
        match self {
            Self::Ready => "",
            Self::Unavailable => "hunspell not installed",
        }
    }
}

/// Probe whether `program` (normally [`HUNSPELL`]) can run.
///
/// Spawn `program -v` (the version banner) and read the outcome: a missing binary
/// is the honest [`SpellState::Unavailable`]; anything that spawns is
/// [`SpellState::Ready`]. Deterministic + side-effect-free (the version query
/// writes nothing).
#[must_use]
pub fn probe(program: &str) -> SpellState {
    match Command::new(program)
        .arg("-v")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => SpellState::Ready,
        Err(_) => SpellState::Unavailable,
    }
}

/// Whether `path`'s buffer is spell-checked (EDTB-6 v1 scope — **md / text first**).
///
/// A pathless scratch buffer (prose jotting), an extension-less file (`README`), or
/// a plain-text / markdown extension qualifies; a recognised code language
/// (`.rs`, `.py`, …) is skipped — spell-checking source would be all false
/// positives.
#[must_use]
pub fn is_spellcheckable(path: Option<&Path>) -> bool {
    let Some(path) = path else {
        return true; // a scratch buffer is prose until saved with a code type
    };
    // No extension (README, NOTES) is prose; else only the text/markdown types.
    path.extension().and_then(|e| e.to_str()).is_none_or(|ext| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "txt" | "text"
        )
    })
}

/// One spell-checkable word: its rope **char** range and the exact text (case
/// preserved — `hunspell` judges capitalisation, and the walk shows it verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordSpan {
    /// The char-index range of the word in the buffer.
    pub chars: Range<usize>,
    /// The word text as it appears in the buffer.
    pub word: String,
}

/// Whether `c` is a within-word apostrophe glyph (straight or typographic) — so
/// `don't` / `it's` tokenise as one word, but a quote around a word does not.
const fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// Split `text` into spell-checkable [`WordSpan`]s (pure, EDTB-6).
///
/// A word is a maximal run of alphabetic chars with optional **internal**
/// apostrophes; a run touching a digit or underscore on either edge is dropped (a
/// code identifier / version token, not prose). Char indices, so the spans
/// address the rope directly.
#[must_use]
pub fn tokenize(text: &str) -> Vec<WordSpan> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        // Extend over letters and *internal* apostrophes (an apostrophe counts only
        // when a letter follows, else it is a trailing quote and ends the word).
        while i < chars.len() {
            let c = chars[i];
            let internal_apostrophe =
                is_apostrophe(c) && i + 1 < chars.len() && chars[i + 1].is_alphabetic();
            if c.is_alphabetic() || internal_apostrophe {
                i += 1;
            } else {
                break;
            }
        }
        let end = i;
        // Reject a run wedged against a digit/underscore (identifier / `v2`).
        let before_ok = start == 0 || !is_ident_glue(chars[start - 1]);
        let after_ok = end >= chars.len() || !is_ident_glue(chars[end]);
        if before_ok && after_ok {
            spans.push(WordSpan {
                chars: start..end,
                word: chars[start..end].iter().collect(),
            });
        }
    }
    spans
}

/// Whether `c` glues a word into a code identifier / number, so a letter run
/// touching it is not prose (`x2`, `foo_bar`, `utf8`).
fn is_ident_glue(c: char) -> bool {
    c.is_numeric() || c == '_'
}

/// One classified `hunspell -a` reply line (EDTB-6) — the exact ispell pipe
/// vocabulary the task names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineVerdict {
    /// In the dictionary — `*` (root), `+` (found via affix), `-` (compound).
    Ok,
    /// A miss with suggestions — `&` (near-misses) / `?` (guesses): the echoed
    /// word and the parsed suggestion list.
    Miss {
        /// The misspelled word `hunspell` echoed back.
        word: String,
        /// The ordered suggestions (may be empty).
        suggestions: Vec<String>,
    },
    /// A miss with no suggestion — `#`: just the echoed word.
    MissNoSuggestions {
        /// The misspelled word `hunspell` echoed back.
        word: String,
    },
    /// A blank separator, the `@…` version banner, or an unrecognised line — no
    /// verdict.
    None,
}

/// Classify one `hunspell -a` output line (pure, EDTB-6):
///
/// * `*` / `+ root` / `- compound` → [`LineVerdict::Ok`],
/// * `& word N off: s1, s2, …` / `? word N off: …` → [`LineVerdict::Miss`],
/// * `# word off` → [`LineVerdict::MissNoSuggestions`],
/// * blank / `@…` banner / anything else → [`LineVerdict::None`].
#[must_use]
pub fn parse_hunspell_line(line: &str) -> LineVerdict {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut chars = line.chars();
    match chars.next() {
        Some('*' | '+' | '-') => LineVerdict::Ok,
        Some('&' | '?') => parse_miss_with_suggestions(line),
        Some('#') => parse_miss_no_suggestions(line),
        _ => LineVerdict::None,
    }
}

/// Parse a `& word count offset: s1, s2, …` (or `? …`) miss line.
fn parse_miss_with_suggestions(line: &str) -> LineVerdict {
    // Everything after the first ':' is the comma-separated suggestion list; the
    // word is the second whitespace field of the head (`& word count offset`).
    let (head, tail) = match line.split_once(':') {
        Some((h, t)) => (h, t),
        None => (line, ""),
    };
    let Some(word) = head.split_whitespace().nth(1) else {
        return LineVerdict::None;
    };
    let suggestions = tail
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    LineVerdict::Miss {
        word: word.to_owned(),
        suggestions,
    }
}

/// Parse a `# word offset` miss-with-no-suggestions line.
fn parse_miss_no_suggestions(line: &str) -> LineVerdict {
    line.split_whitespace()
        .nth(1)
        .map_or(LineVerdict::None, |word| LineVerdict::MissNoSuggestions {
            word: word.to_owned(),
        })
}

/// Fold a whole `hunspell -a` transcript into a `word → suggestions` map (pure, EDTB-6).
///
/// Every miss line contributes its echoed word (keyed exact — `hunspell` judges
/// case); `Ok` / banner / blank lines are ignored, so the result never depends on
/// positional alignment with the fed words. A `#` miss maps to an empty suggestion
/// list. Later suggestions for an already-seen word do not clobber a non-empty
/// earlier list.
#[must_use]
pub fn parse_response(output: &str) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in output.lines() {
        match parse_hunspell_line(line) {
            LineVerdict::Miss { word, suggestions } => {
                let entry = map.entry(word).or_default();
                if entry.is_empty() {
                    *entry = suggestions;
                }
            }
            LineVerdict::MissNoSuggestions { word } => {
                map.entry(word).or_default();
            }
            LineVerdict::Ok | LineVerdict::None => {}
        }
    }
    map
}

/// One resolved misspelling ready to underline + walk: its rope **char** range,
/// the word, and its suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellMiss {
    /// The char-index range in the rope the red squiggle underlines.
    pub chars: Range<usize>,
    /// The misspelled word.
    pub word: String,
    /// The ordered suggestions (may be empty — a `#` miss).
    pub suggestions: Vec<String>,
}

/// Map every span whose word is missed onto a [`SpellMiss`] (pure, EDTB-6).
///
/// Preserves `spans` order (ascending). A word that appears many times yields a
/// miss per occurrence, all carrying the same suggestions — so every instance
/// squiggles and the walk visits each.
#[must_use]
pub fn resolve_spans(spans: &[WordSpan], misses: &BTreeMap<String, Vec<String>>) -> Vec<SpellMiss> {
    spans
        .iter()
        .filter_map(|s| {
            misses.get(&s.word).map(|sugg| SpellMiss {
                chars: s.chars.clone(),
                word: s.word.clone(),
                suggestions: sugg.clone(),
            })
        })
        .collect()
}

/// Build the `hunspell -a` stdin: one word per line. Words are already
/// letter-initial (so none is read as an ispell control command), so no `^`
/// escape is needed, and the transcript is matched back by the echoed word — not
/// by line position — so a benign reordering never mis-maps a suggestion.
fn build_input<'a>(words: impl Iterator<Item = &'a str>) -> String {
    let mut input = String::new();
    for w in words {
        input.push_str(w);
        input.push('\n');
    }
    input
}

/// Feed `input` to `program -a` and return its transcript, or the honest
/// [`SpellState::Unavailable`] when the binary is absent / cannot run.
fn run_pipe(program: &str, input: &str) -> Result<String, SpellState> {
    let mut child = Command::new(program)
        .arg(PIPE_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| SpellState::Unavailable)?;
    if let Some(mut stdin) = child.stdin.take() {
        // A dictionary-less hunspell may exit early; ignore the broken-pipe write
        // error and let the (empty) transcript classify honestly below.
        let _ = stdin.write_all(input.as_bytes());
        // stdin drops here → EOF to hunspell.
    }
    let output = child
        .wait_with_output()
        .map_err(|_| SpellState::Unavailable)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Spell-check `text` through `program` (normally [`HUNSPELL`]) and return the
/// resolved misses (EDTB-6).
///
/// Tokenises, feeds the **unique** words to `hunspell -a`, and maps the reported
/// misses back to every occurrence's span.
///
/// # Errors
/// Returns [`SpellState::Unavailable`] when `program` is not installed / cannot
/// run — the honest gated state (§7), never a silent empty result masquerading
/// as "all correct".
pub fn run_hunspell(program: &str, text: &str) -> Result<Vec<SpellMiss>, SpellState> {
    if text.len() > MAX_TEXT_BYTES {
        return Ok(Vec::new());
    }
    let spans = tokenize(text);
    if spans.is_empty() {
        return Ok(Vec::new());
    }
    // Unique words (BTreeSet → stable order) keep the fed input small on a
    // repetitive buffer.
    let unique: std::collections::BTreeSet<&str> = spans.iter().map(|s| s.word.as_str()).collect();
    let input = build_input(unique.into_iter());
    let transcript = run_pipe(program, &input)?;
    let misses = parse_response(&transcript);
    Ok(resolve_spans(&spans, &misses))
}

/// The spell misses the widget underlines this frame.
///
/// A cheap, `Copy`, borrowed slice, exactly like
/// [`MatchHighlights`](crate::widget::MatchHighlights). The [`Default`] (empty)
/// means "no squiggles", so every non-spell paint site is unchanged.
#[derive(Clone, Copy, Default)]
pub struct SpellMarks<'a> {
    /// The resolved misses to underline — ascending char ranges into the rope.
    pub misses: &'a [SpellMiss],
}

/// The resolved-miss store the widget paints + the walk steps through.
///
/// Rebuilt only when a background check completes (the spell analogue of the LSP
/// `DiagnosticsOverlay`). Holds the last raw result and a **visible** view with
/// the session-ignored / personal-dictionary words filtered out, so toggling a
/// word re-filters instantly without re-running `hunspell`.
#[derive(Default)]
pub struct SpellOverlay {
    /// Every miss from the last completed check (unfiltered).
    raw: Vec<SpellMiss>,
    /// `raw` minus the ignored / personal words — what is painted + walked.
    visible: Vec<SpellMiss>,
    /// The buffer revision `raw` was checked at (the panel's staleness gate).
    checked_rev: u64,
    /// Whether a check has ever completed (a fresh overlay is empty but valid).
    built: bool,
}

impl SpellOverlay {
    /// Replace the raw misses from a completed check at `rev`, then re-derive the
    /// visible set against the current ignore / personal words.
    fn set(
        &mut self,
        raw: Vec<SpellMiss>,
        rev: u64,
        ignored: &HashSet<String>,
        personal: &HashSet<String>,
    ) {
        self.raw = raw;
        self.checked_rev = rev;
        self.built = true;
        self.refilter(ignored, personal);
    }

    /// Re-derive the visible set from the raw misses (called on a new result and
    /// after Ignore-All / Add-to-dictionary) — a word in either set is dropped.
    fn refilter(&mut self, ignored: &HashSet<String>, personal: &HashSet<String>) {
        self.visible = self
            .raw
            .iter()
            .filter(|m| !ignored.contains(&m.word) && !personal.contains(&m.word))
            .cloned()
            .collect();
    }

    /// Drop every mark, e.g. when the doc becomes non-spell-checkable or the
    /// checker is unavailable — no stale squiggles linger (§7).
    fn clear(&mut self) {
        self.raw.clear();
        self.visible.clear();
        self.built = false;
    }

    /// The visible misses — the widget's squiggles + the walk's items.
    #[must_use]
    pub fn misses(&self) -> &[SpellMiss] {
        &self.visible
    }
}

/// A background `hunspell` run in flight (EDTB-6): the channel a worker thread
/// sends its outcome on, plus the buffer revision it was launched for. The panel
/// polls [`take`](Self::take) each frame; the thread is fire-and-forget (it sends
/// once and exits, so a dropped [`SpellChecker`] leaks nothing).
struct SpellWorker {
    /// The completed result (`Ok` misses / `Err` the gated state) once ready.
    rx: Receiver<Result<Vec<SpellMiss>, SpellState>>,
    /// The buffer revision this check reflects.
    rev: u64,
}

impl SpellWorker {
    /// Spawn a background check of `text` through `program`, tagged `rev`.
    fn spawn(program: String, text: String, rev: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        // A named thread so it shows honestly in a stack dump; failure to spawn
        // (never seen in practice) just leaves an eternally-pending worker, which
        // the next revision replaces — no panic, no fake result.
        let _ = std::thread::Builder::new()
            .name("mde-spell".to_owned())
            .spawn(move || {
                let outcome = run_hunspell(&program, &text);
                let _ = tx.send(outcome);
            });
        Self { rx, rev }
    }

    /// The result if the check has finished, else `None` (still running).
    fn take(&self) -> Option<Result<Vec<SpellMiss>, SpellState>> {
        self.rx.try_recv().ok()
    }
}

/// The per-document spell-check seam the panel owns (EDTB-6).
///
/// The spell analogue of the LSP client + `DiagnosticsOverlay` on a
/// [`Doc`](crate::panel): it debounces a background check on the buffer revision,
/// holds the resolved misses, and applies session Ignore-All / Add-to-dictionary
/// as an in-app filter (per document, this session — not persisted, and honestly
/// scoped as such).
#[derive(Default)]
pub struct SpellChecker {
    /// The resolved misses (raw + visible).
    overlay: SpellOverlay,
    /// The check in flight, if any.
    worker: Option<SpellWorker>,
    /// The buffer revision the last check was **launched** for — dedups spawns so
    /// one revision is checked at most once.
    launched_rev: Option<u64>,
    /// Words Ignore-All'd this session (per document).
    ignored: HashSet<String>,
    /// Words added to the personal dictionary this session (per document).
    personal: HashSet<String>,
}

impl SpellChecker {
    /// The visible misses to underline this frame (§4-styled by the widget).
    #[must_use]
    pub fn marks(&self) -> SpellMarks<'_> {
        SpellMarks {
            misses: self.overlay.misses(),
        }
    }

    /// The visible misses (the F7 walk's items).
    #[must_use]
    pub fn misses(&self) -> &[SpellMiss] {
        self.overlay.misses()
    }

    /// Whether a background check is currently running (the panel requests a
    /// repaint so its result is picked up promptly).
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        self.worker.is_some()
    }

    /// Drain a finished worker into the overlay, if one completed. Returns
    /// whether the visible misses may have changed (so the panel repaints).
    pub fn poll(&mut self) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        let Some(outcome) = worker.take() else {
            return false;
        };
        let rev = worker.rev;
        self.worker = None;
        match outcome {
            Ok(raw) => {
                self.overlay.set(raw, rev, &self.ignored, &self.personal);
                true
            }
            Err(SpellState::Unavailable) => {
                // hunspell vanished mid-session — drop to the honest empty state.
                self.overlay.clear();
                true
            }
            Err(SpellState::Ready) => false,
        }
    }

    /// Whether a fresh check should be launched for `rev` — no check in flight
    /// and this revision was not already launched.
    #[must_use]
    pub fn wants_check(&self, rev: u64) -> bool {
        self.worker.is_none() && self.launched_rev != Some(rev)
    }

    /// Launch a background check of `text` (the buffer snapshot at `rev`).
    pub fn spawn(&mut self, program: &str, text: String, rev: u64) {
        self.launched_rev = Some(rev);
        self.worker = Some(SpellWorker::spawn(program.to_owned(), text, rev));
    }

    /// Drop all misses + any in-flight check — the doc is no longer
    /// spell-checkable / the checker is unavailable. `launched_rev` resets so the
    /// pass re-runs cleanly if the doc becomes checkable again.
    pub fn disable(&mut self) {
        self.overlay.clear();
        self.worker = None;
        self.launched_rev = None;
    }

    /// Ignore every occurrence of `word` for this session (Ignore-All) — filtered
    /// out of the visible misses at once, no `hunspell` re-run.
    pub fn ignore_all(&mut self, word: &str) {
        self.ignored.insert(word.to_owned());
        self.overlay.refilter(&self.ignored, &self.personal);
    }

    /// Add `word` to the session personal dictionary (Add-to-dictionary) — as
    /// above. Session + per-document scoped (v1): honest, not persisted.
    pub fn add_to_dictionary(&mut self, word: &str) {
        self.personal.insert(word.to_owned());
        self.overlay.refilter(&self.ignored, &self.personal);
    }

    /// Whether `word` is in this document's session personal dictionary.
    #[must_use]
    pub fn is_added(&self, word: &str) -> bool {
        self.personal.contains(word)
    }

    /// Test seam: inject a completed result without spawning `hunspell`, so the
    /// paint / walk / replace wiring is exercised deterministically on the
    /// airgapped farm (no dictionary needed).
    #[cfg(test)]
    pub fn set_misses_for_test(&mut self, raw: Vec<SpellMiss>, rev: u64) {
        self.overlay.set(raw, rev, &self.ignored, &self.personal);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_spellcheckable, parse_hunspell_line, parse_response, resolve_spans, run_hunspell,
        tokenize, LineVerdict, SpellChecker, SpellMiss, SpellState, WordSpan,
    };
    use std::path::Path;

    #[test]
    fn tokenize_splits_prose_and_keeps_internal_apostrophes() {
        let spans = tokenize("The quick don't fox.");
        let words: Vec<&str> = spans.iter().map(|s| s.word.as_str()).collect();
        assert_eq!(words, vec!["The", "quick", "don't", "fox"]);
        // The char ranges address the rope directly.
        assert_eq!(spans[0].chars, 0..3);
        assert_eq!(spans[2].word, "don't");
    }

    #[test]
    fn tokenize_skips_code_ish_tokens() {
        // A letter run touching a digit / underscore is an identifier, not prose.
        let spans = tokenize("use foo_bar and utf8 and x2 plain");
        let words: Vec<&str> = spans.iter().map(|s| s.word.as_str()).collect();
        assert_eq!(
            words,
            vec!["use", "and", "and", "plain"],
            "foo_bar / utf8 / x2 are dropped as code-ish"
        );
    }

    #[test]
    fn is_spellcheckable_is_md_text_first() {
        assert!(is_spellcheckable(None), "a scratch buffer is prose");
        assert!(is_spellcheckable(Some(Path::new("/n/notes.md"))));
        assert!(is_spellcheckable(Some(Path::new("/n/notes.txt"))));
        assert!(is_spellcheckable(Some(Path::new("/n/README"))));
        assert!(
            !is_spellcheckable(Some(Path::new("/src/main.rs"))),
            "source is skipped in v1"
        );
        assert!(!is_spellcheckable(Some(Path::new("/src/app.py"))));
    }

    #[test]
    fn parse_line_classifies_the_ispell_vocabulary() {
        // In-dictionary forms.
        assert_eq!(parse_hunspell_line("*"), LineVerdict::Ok);
        assert_eq!(parse_hunspell_line("+ root"), LineVerdict::Ok);
        assert_eq!(parse_hunspell_line("- compound"), LineVerdict::Ok);
        // Miss with suggestions (&) — word echoed, list parsed.
        assert_eq!(
            parse_hunspell_line("& wrold 3 6: world, wold, word"),
            LineVerdict::Miss {
                word: "wrold".to_owned(),
                suggestions: vec!["world".to_owned(), "wold".to_owned(), "word".to_owned()],
            }
        );
        // Guess (?) parses the same shape.
        assert_eq!(
            parse_hunspell_line("? xyzzy 1 0: fizzy"),
            LineVerdict::Miss {
                word: "xyzzy".to_owned(),
                suggestions: vec!["fizzy".to_owned()],
            }
        );
        // Miss with no suggestions (#).
        assert_eq!(
            parse_hunspell_line("# qwzxv 0"),
            LineVerdict::MissNoSuggestions {
                word: "qwzxv".to_owned(),
            }
        );
        // Banner / blank are no-verdict.
        assert_eq!(
            parse_hunspell_line("@(#) International Ispell Version 3.4"),
            LineVerdict::None
        );
        assert_eq!(parse_hunspell_line(""), LineVerdict::None);
    }

    #[test]
    fn parse_response_folds_misses_into_a_word_map() {
        // A realistic `hunspell -a` transcript for the words: hello wrold qwzxv.
        let transcript = "\
@(#) International Ispell Version 3.4
*

& wrold 3 6: world, wold, word

# qwzxv 0

";
        let map = parse_response(transcript);
        assert_eq!(map.len(), 2, "only the two misses are keyed");
        assert!(
            !map.contains_key("hello"),
            "an in-dictionary word is absent"
        );
        assert_eq!(
            map.get("wrold").unwrap(),
            &vec!["world".to_owned(), "wold".to_owned(), "word".to_owned()]
        );
        assert_eq!(
            map.get("qwzxv").unwrap(),
            &Vec::<String>::new(),
            "a # miss maps to an empty suggestion list"
        );
    }

    #[test]
    fn resolve_maps_every_occurrence_to_a_span() {
        // "wrold" twice → two misses, both with the suggestions; "ok" is clean.
        let spans = tokenize("wrold ok wrold");
        let map = parse_response("& wrold 1 0: world\n");
        let misses = resolve_spans(&spans, &map);
        assert_eq!(misses.len(), 2, "both occurrences squiggle");
        assert_eq!(misses[0].chars, 0..5);
        assert_eq!(misses[1].chars, 9..14);
        assert!(misses
            .iter()
            .all(|m| m.suggestions == vec!["world".to_owned()]));
    }

    #[test]
    fn run_hunspell_with_a_missing_binary_is_the_honest_unavailable_state() {
        // §7: spawning a non-existent checker classifies to Unavailable — never a
        // silent empty "all correct". Deterministic + side-effect-free (the spawn
        // fails before any process runs), so the gate is green without a live
        // dictionary on the farm.
        let err = run_hunspell("mcnf-no-such-hunspell-xyzzy", "some prose here")
            .expect_err("a missing hunspell must be an honest error");
        assert_eq!(err, SpellState::Unavailable);
        assert_eq!(err.notice(), "hunspell not installed");
        assert!(!err.is_ready());
    }

    #[test]
    fn run_hunspell_on_empty_or_wordless_text_is_ok_and_empty() {
        // No words → no subprocess, an honest empty result (not Unavailable).
        assert_eq!(run_hunspell("mcnf-no-such-hunspell", ""), Ok(Vec::new()));
        assert_eq!(
            run_hunspell("mcnf-no-such-hunspell", "123 :: 456"),
            Ok(Vec::new())
        );
    }

    #[test]
    fn checker_paints_misses_and_ignore_all_filters_every_occurrence() {
        let mut checker = SpellChecker::default();
        let misses = vec![
            SpellMiss {
                chars: 0..5,
                word: "wrold".to_owned(),
                suggestions: vec!["world".to_owned()],
            },
            SpellMiss {
                chars: 9..14,
                word: "wrold".to_owned(),
                suggestions: vec!["world".to_owned()],
            },
            SpellMiss {
                chars: 15..19,
                word: "teh".to_owned(),
                suggestions: vec!["the".to_owned()],
            },
        ];
        checker.set_misses_for_test(misses, 1);
        assert_eq!(checker.misses().len(), 3, "all three squiggle");

        // Ignore-All drops every occurrence of the word, no re-run.
        checker.ignore_all("wrold");
        let words: Vec<&str> = checker.misses().iter().map(|m| m.word.as_str()).collect();
        assert_eq!(words, vec!["teh"], "both wrolds gone, teh remains");

        // Add-to-dictionary drops the last one → clean.
        checker.add_to_dictionary("teh");
        assert!(
            checker.misses().is_empty(),
            "adding teh clears the last miss"
        );
        assert!(checker.is_added("teh"));
    }

    #[test]
    fn word_span_orders_ascending() {
        // Sanity: spans come back ascending, so the widget's monotonic per-row
        // walk over the marks is valid.
        let spans = tokenize("alpha beta gamma");
        let starts: Vec<usize> = spans.iter().map(|s| s.chars.start).collect();
        assert_eq!(starts, vec![0, 6, 11]);
        let _ = WordSpan {
            chars: 0..1,
            word: "x".to_owned(),
        };
    }
}
