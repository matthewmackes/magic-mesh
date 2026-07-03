//! FILEMGR-10 — the render-agnostic preview/thumbnail core (locks 18, 22, 23).
//!
//! Everything decision-shaped about previews lives here, with no egui in it:
//! what a file *can* preview as ([`PreviewKind`] — extension-keyed, honest about
//! which decoders the workspace lock actually ships), the bounded LRU caches of
//! decoded [`Pixels`]/[`PreviewData`], and the one background worker thread that
//! does every decode. The paint path **never** decodes: the view requests, draws
//! an icon placeholder, and the worker delivers over a channel
//! ([`Previews::pump`] folds deliveries in once per frame).
//!
//! Honesty contract (§7 / lock 23):
//! - Only formats with a decoder **in the workspace lock** decode: `image` 0.25
//!   (png/jpeg/gif/webp/bmp/tiff) for raster images, Symphonia 0.5 header
//!   probes (flac/mp3/vorbis/aac/isomp4/ogg/wav) for media metadata. Everything
//!   else is an explicit "no built-in viewer/decoder" — never a stub, never an
//!   external-program spawn (§9 / lock 23).
//! - The lock ships **no video codec**, so video files never fake a frame:
//!   isomp4-family containers get an honest duration/codec probe, the rest get
//!   metadata only.
//! - Remote (mesh-mount) files are never bulk-decoded (lock 18): the view only
//!   requests them when selected, and the preview pane offers an explicit
//!   "load on demand" affordance instead of auto-reading over sshfs.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::Path;
use std::sync::{mpsc, Arc};

/// Longest thumbnail side, px. Sized for the Grid tile / List row cells; small
/// enough that the [`THUMB_CAP`]-bounded cache stays a few MB of RGBA.
pub const THUMB_PX: u32 = 96;
/// Longest preview-image side, px (the preview pane + quick-look render).
pub const PREVIEW_PX: u32 = 640;
/// Thumbnail cache cap (entries). Covers a full 4K screen of Grid tiles with
/// headroom, so a visible grid never thrashes its own thumbnails out.
pub const THUMB_CAP: usize = 512;
/// Preview cache cap (entries). The pane + quick-look show at most a couple at
/// a time; a handful of warm entries makes flipping through a selection cheap.
pub const PREVIEW_CAP: usize = 8;
/// Files larger than this are honestly refused ("too large to decode") rather
/// than read into memory on the worker.
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
/// Text previews read at most this many bytes (then honestly say "truncated").
const TEXT_CAP_BYTES: usize = 64 * 1024;
/// Text previews tokenize at most this many lines.
const TEXT_CAP_LINES: usize = 500;
/// A single pathological line (minified JSON…) is cut at this many chars.
const TEXT_CAP_LINE_CHARS: usize = 2000;

// ═══════════════════════════════════════════════════════════════════════════
// What a file can preview as (extension-keyed, honest about locked decoders).
// ═══════════════════════════════════════════════════════════════════════════

/// The language a text preview is highlighted as (extension-keyed; simple
/// keyword/comment/string/number classes — deliberately no parser dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextLang {
    /// `.rs` — Rust.
    Rust,
    /// `.c/.h/.cpp/…` — the C family.
    C,
    /// `.py` — Python.
    Python,
    /// `.sh/.bash/.zsh` + shell dotfiles.
    Shell,
    /// `.toml/.ini/.conf/…` — key-value config.
    Toml,
    /// `.json`.
    Json,
    /// `.yml/.yaml`.
    Yaml,
    /// `.md` — headings/quotes/fences only.
    Markdown,
    /// Plain text — no keyword set.
    Plain,
}

impl TextLang {
    /// The line-comment prefix for this language (`None` = no line comments).
    const fn comment_prefix(self) -> Option<&'static str> {
        match self {
            Self::Rust | Self::C => Some("//"),
            Self::Python | Self::Shell | Self::Toml | Self::Yaml => Some("#"),
            Self::Json | Self::Markdown | Self::Plain => None,
        }
    }

    /// The (small, deliberately incomplete — "syntax-ish") keyword set.
    fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
                "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
                "trait", "type", "unsafe", "use", "where", "while",
            ],
            Self::C => &[
                "break", "case", "char", "const", "continue", "default", "define", "double",
                "else", "enum", "float", "for", "if", "include", "int", "long", "return", "short",
                "signed", "sizeof", "static", "struct", "switch", "typedef", "unsigned", "void",
                "while",
            ],
            Self::Python => &[
                "and", "as", "assert", "break", "class", "continue", "def", "elif", "else",
                "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
                "None", "nonlocal", "not", "or", "pass", "raise", "return", "self", "try", "while",
                "with", "yield", "True", "False",
            ],
            Self::Shell => &[
                "case", "do", "done", "echo", "elif", "else", "esac", "exit", "export", "fi",
                "for", "function", "if", "local", "return", "set", "then", "while",
            ],
            Self::Toml => &["true", "false"],
            Self::Json | Self::Yaml => &["true", "false", "null"],
            Self::Markdown | Self::Plain => &[],
        }
    }
}

/// How the shell can preview a file.
///
/// The fold every preview surface (grid thumbnail, preview pane, quick-look)
/// drives off. Extension-keyed and honest: a variant only claims a decode the
/// workspace lock can actually perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    /// A directory — opened, not previewed.
    Folder,
    /// A raster image the locked `image` crate decodes
    /// (png/jpg/jpeg/gif/webp/bmp/tif/tiff): thumbnails + full preview.
    Image,
    /// An image format with **no decoder in the lock** (heic/avif/svg/ico) —
    /// honest icon + note, never a stub decode.
    ImageNoDecoder,
    /// Highlightable text, with the language for the syntax-ish tokenizer.
    Text(TextLang),
    /// Audio Symphonia can header-probe (duration/codec/rate/channels).
    Audio,
    /// A video container Symphonia parses (isomp4: mp4/m4v/mov) — duration and
    /// codec metadata only; the lock ships **no video codec**, so no frame.
    Video,
    /// A video container with no locked parser (mkv/webm/avi/…) — name/size/
    /// modified metadata only, honestly labeled.
    VideoNoProbe,
    /// No built-in viewer (lock 23) — the label names what it is.
    NoViewer(&'static str),
}

impl PreviewKind {
    /// Classify `name` (a row's display name; directories may carry a trailing
    /// `/` but are routed by `is_dir` first).
    #[must_use]
    pub fn detect(name: &str, is_dir: bool) -> Self {
        if is_dir {
            return Self::Folder;
        }
        let lower = name.to_lowercase();
        // Well-known extensionless / dotfile text names first.
        match lower.as_str() {
            "makefile" | "license" | "readme" | "changelog" | "todo" => {
                return Self::Text(TextLang::Plain)
            }
            "dockerfile" => return Self::Text(TextLang::Shell),
            ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" => {
                return Self::Text(TextLang::Shell)
            }
            ".gitignore" | ".gitconfig" => return Self::Text(TextLang::Toml),
            _ => {}
        }
        let ext = Path::new(&lower)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        match ext {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" => Self::Image,
            "heic" | "heif" | "avif" | "svg" | "ico" => Self::ImageNoDecoder,
            "rs" => Self::Text(TextLang::Rust),
            "c" | "h" | "cpp" | "hpp" | "cc" | "hh" => Self::Text(TextLang::C),
            "py" => Self::Text(TextLang::Python),
            "sh" | "bash" | "zsh" => Self::Text(TextLang::Shell),
            "toml" | "ini" | "cfg" | "conf" | "service" | "timer" | "desktop" | "spec" => {
                Self::Text(TextLang::Toml)
            }
            "json" => Self::Text(TextLang::Json),
            "yml" | "yaml" => Self::Text(TextLang::Yaml),
            "md" | "markdown" => Self::Text(TextLang::Markdown),
            "txt" | "log" | "csv" | "xml" | "html" | "css" | "js" | "ts" | "lock" | "sql"
            | "lua" | "go" | "java" | "rb" | "nix" | "patch" | "diff" => {
                Self::Text(TextLang::Plain)
            }
            "mp3" | "flac" | "ogg" | "oga" | "opus" | "wav" | "m4a" | "aac" => Self::Audio,
            "mp4" | "m4v" | "mov" => Self::Video,
            "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpg" | "mpeg" => Self::VideoNoProbe,
            "pdf" => Self::NoViewer("PDF"),
            "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" => Self::NoViewer("archive"),
            "iso" | "qcow2" | "img" | "raw" => Self::NoViewer("disk image"),
            _ => Self::NoViewer("unknown type"),
        }
    }

    /// `true` when this kind has something for the worker to decode/probe —
    /// the only kinds the view ever submits a preview job for.
    #[must_use]
    pub const fn worker_previews(self) -> bool {
        matches!(
            self,
            Self::Image | Self::Text(_) | Self::Audio | Self::Video
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Decoded payloads.
// ═══════════════════════════════════════════════════════════════════════════

/// A decoded RGBA raster, sized for its surface (thumbnail or preview). The
/// bytes are `Arc`-shared so cache reads and texture uploads never copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixels {
    /// `[width, height]` in px.
    pub size: [usize; 2],
    /// Tightly-packed RGBA8, `size[0] * size[1] * 4` bytes.
    pub rgba: Arc<Vec<u8>>,
}

/// One syntax-ish token: the span text and its class.
pub type TokenSpan = (String, TokenKind);

/// The token classes the view maps onto Carbon text tones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Ordinary text.
    Plain,
    /// A language keyword.
    Keyword,
    /// A comment (to end of line).
    Comment,
    /// A quoted string literal.
    Str,
    /// A numeric literal.
    Number,
    /// A Markdown heading line.
    Heading,
}

/// Media metadata probed from container headers (never a decode). Every field
/// is optional and shown only when the container actually carries it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaMeta {
    /// Total duration in seconds, when the headers declare frame counts.
    pub duration_secs: Option<f64>,
    /// The codec short-name Symphonia's registry knows, if any.
    pub codec: Option<String>,
    /// Sample rate (audio tracks).
    pub sample_rate: Option<u32>,
    /// Channel count (audio tracks).
    pub channels: Option<usize>,
}

/// A finished preview payload for the pane/quick-look.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewData {
    /// A decoded image scaled to [`PREVIEW_PX`], plus the file's full pixel
    /// dimensions (the "media metadata: dims" line).
    Image {
        /// The scaled RGBA raster.
        pixels: Pixels,
        /// The original `[width, height]` before scaling.
        full: [u32; 2],
    },
    /// Tokenized text lines (worker-side tokenized, so the paint path only
    /// lays out) and whether the read/tokenize was capped.
    Text {
        /// Per-line token spans.
        lines: Vec<Vec<TokenSpan>>,
        /// `true` when the byte/line/line-length caps cut the file short.
        truncated: bool,
    },
    /// Probed media metadata.
    Media(MediaMeta),
}

/// The lifecycle of one thumbnail slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbState {
    /// Submitted to the worker; the view keeps drawing the icon placeholder.
    Pending,
    /// Decoded. `stamp` is a monotonic id the view keys texture uploads on.
    Ready {
        /// Monotonic delivery stamp (texture-cache key).
        stamp: u64,
        /// The decoded thumbnail raster.
        pixels: Pixels,
    },
    /// Decode honestly failed (unreadable/undecodable/too large); the reason
    /// is shown, never a stub image.
    Failed(String),
}

/// The lifecycle of one preview slot (same shape as [`ThumbState`]).
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewState {
    /// Submitted to the worker.
    Pending,
    /// Decoded/probed and ready to render.
    Ready {
        /// Monotonic delivery stamp (texture-cache key).
        stamp: u64,
        /// The decoded payload (`Arc` so per-frame reads never clone it).
        data: Arc<PreviewData>,
    },
    /// Honest failure with the reason.
    Failed(String),
}

// ═══════════════════════════════════════════════════════════════════════════
// A small bounded LRU (path-keyed). Hand-rolled: ~40 lines beats a new dep.
// ═══════════════════════════════════════════════════════════════════════════

/// A bounded least-recently-used map keyed by path string.
///
/// `get` is a pure peek; recency moves on [`touch`](Self::touch)/
/// [`insert`](Self::insert), which the request path drives every frame a slot
/// is actually wanted — so eviction order tracks *visibility*, not insertion.
pub struct Lru<V> {
    cap: usize,
    map: HashMap<String, V>,
    order: VecDeque<String>,
}

impl<V> Lru<V> {
    /// A cache holding at most `cap` entries (minimum 1).
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Peek at a slot without touching recency.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&V> {
        self.map.get(key)
    }

    /// Bump `key` to most-recently-used (no-op when absent).
    pub fn touch(&mut self, key: &str) {
        if self.map.contains_key(key) {
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                self.order.remove(pos);
            }
            self.order.push_back(key.to_string());
        }
    }

    /// Insert (or replace) a slot as most-recently-used, evicting the least-
    /// recently-used entries beyond the cap.
    pub fn insert(&mut self, key: String, value: V) {
        let existed = self.map.insert(key.clone(), value).is_some();
        if existed {
            self.touch(&key);
        } else {
            self.order.push_back(key);
        }
        while self.map.len() > self.cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&oldest);
        }
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// Current entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// `true` when the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Iterate the cached values (order unspecified).
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.map.values()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The worker protocol + the surface-facing state.
// ═══════════════════════════════════════════════════════════════════════════

enum Job {
    Thumb { key: String, epoch: u64 },
    Preview { key: String, epoch: u64 },
}

enum DoneMsg {
    Thumb {
        key: String,
        epoch: u64,
        result: Result<Pixels, String>,
    },
    Preview {
        key: String,
        epoch: u64,
        result: Result<PreviewData, String>,
    },
}

/// The FILEMGR-10 preview state a [`crate::model::FileBrowser`] owns.
///
/// One decode worker, the bounded caches, and the pane/quick-look toggles. No
/// egui in here — the view reads states and uploads textures itself.
pub struct Previews {
    jobs: mpsc::Sender<Job>,
    done: mpsc::Receiver<DoneMsg>,
    thumbs: Lru<ThumbState>,
    preview_cache: Lru<PreviewState>,
    /// Monotonic delivery stamp — the view keys texture re-uploads on it.
    next_stamp: u64,
    /// Bumped on [`clear`](Self::clear) so late deliveries from a busted cache
    /// generation are dropped (a refresh must never resurrect stale pixels).
    epoch: u64,
    pane_open: bool,
    quick_look: bool,
    list_thumbs: bool,
}

impl Previews {
    /// Spawn the decode worker and empty caches. If the thread can't spawn,
    /// every request honestly fails with "preview worker unavailable" instead
    /// of pretending to load.
    #[must_use]
    pub fn spawn() -> Self {
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
        let (done_tx, done_rx) = mpsc::channel::<DoneMsg>();
        // Detached on purpose: the worker exits when the job sender drops
        // (i.e. when this `Previews` — and its browser — is dropped).
        drop(
            std::thread::Builder::new()
                .name("mde-files-preview".to_string())
                .spawn(move || worker(&jobs_rx, &done_tx)),
        );
        Self {
            jobs: jobs_tx,
            done: done_rx,
            thumbs: Lru::new(THUMB_CAP),
            preview_cache: Lru::new(PREVIEW_CAP),
            next_stamp: 1,
            epoch: 0,
            pane_open: true,
            quick_look: false,
            list_thumbs: true,
        }
    }

    // ── requests (the view pushes these through `Action`s every frame a slot
    //    is wanted, which doubles as the LRU recency signal) ─────────────────

    /// Want a thumbnail for `path`: submit a decode when the slot is cold,
    /// otherwise just keep it warm in the LRU.
    pub fn request_thumb(&mut self, path: &str) {
        if self.thumbs.get(path).is_some() {
            self.thumbs.touch(path);
            return;
        }
        let state = if self
            .jobs
            .send(Job::Thumb {
                key: path.to_string(),
                epoch: self.epoch,
            })
            .is_ok()
        {
            ThumbState::Pending
        } else {
            ThumbState::Failed("preview worker unavailable".to_string())
        };
        self.thumbs.insert(path.to_string(), state);
    }

    /// Want a pane/quick-look preview for `path` (same warm/cold contract as
    /// [`request_thumb`](Self::request_thumb)).
    pub fn request_preview(&mut self, path: &str) {
        if self.preview_cache.get(path).is_some() {
            self.preview_cache.touch(path);
            return;
        }
        let state = if self
            .jobs
            .send(Job::Preview {
                key: path.to_string(),
                epoch: self.epoch,
            })
            .is_ok()
        {
            PreviewState::Pending
        } else {
            PreviewState::Failed("preview worker unavailable".to_string())
        };
        self.preview_cache.insert(path.to_string(), state);
    }

    /// Fold the worker's finished decodes into the caches (once per frame).
    /// Returns `true` when anything landed (the view repaints).
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        while let Ok(msg) = self.done.try_recv() {
            let stamp = self.next_stamp;
            match msg {
                DoneMsg::Thumb { epoch, .. } | DoneMsg::Preview { epoch, .. }
                    if epoch != self.epoch =>
                {
                    // A delivery from before a cache bust — stale by definition.
                    continue;
                }
                DoneMsg::Thumb { key, result, .. } => {
                    let state = match result {
                        Ok(pixels) => ThumbState::Ready { stamp, pixels },
                        Err(e) => ThumbState::Failed(e),
                    };
                    self.thumbs.insert(key, state);
                }
                DoneMsg::Preview { key, result, .. } => {
                    let state = match result {
                        Ok(data) => PreviewState::Ready {
                            stamp,
                            data: Arc::new(data),
                        },
                        Err(e) => PreviewState::Failed(e),
                    };
                    self.preview_cache.insert(key, state);
                }
            }
            self.next_stamp += 1;
            any = true;
        }
        any
    }

    // ── reads ────────────────────────────────────────────────────────────────

    /// The thumbnail slot for `path`, if requested this cache generation.
    #[must_use]
    pub fn thumb(&self, path: &str) -> Option<&ThumbState> {
        self.thumbs.get(path)
    }

    /// The preview slot for `path`, if requested this cache generation.
    #[must_use]
    pub fn preview(&self, path: &str) -> Option<&PreviewState> {
        self.preview_cache.get(path)
    }

    /// `true` while any decode is still in flight (drives a repaint heartbeat).
    #[must_use]
    pub fn any_pending(&self) -> bool {
        self.thumbs
            .values()
            .any(|s| matches!(s, ThumbState::Pending))
            || self
                .preview_cache
                .values()
                .any(|s| matches!(s, PreviewState::Pending))
    }

    /// Bust both caches (lock 18 — a manual refresh re-decodes; in-flight
    /// deliveries from the old generation are dropped by the epoch check).
    pub fn clear(&mut self) {
        self.thumbs.clear();
        self.preview_cache.clear();
        self.epoch += 1;
    }

    // ── the FILEMGR-10 presentation toggles ──────────────────────────────────

    /// Whether the right-hand preview pane is shown.
    #[must_use]
    pub fn pane_open(&self) -> bool {
        self.pane_open
    }

    /// Toggle the preview pane.
    pub fn toggle_pane(&mut self) {
        self.pane_open = !self.pane_open;
    }

    /// Whether the quick-look overlay is up.
    #[must_use]
    pub fn quick_look(&self) -> bool {
        self.quick_look
    }

    /// Set the quick-look overlay state.
    pub fn set_quick_look(&mut self, open: bool) {
        self.quick_look = open;
    }

    /// Whether the List view shows its thumbnail column (Grid always does).
    #[must_use]
    pub fn list_thumbs(&self) -> bool {
        self.list_thumbs
    }

    /// Toggle the List view's thumbnail column.
    pub fn toggle_list_thumbs(&mut self) {
        self.list_thumbs = !self.list_thumbs;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The worker — the ONLY place a decode/probe/file-read runs.
// ═══════════════════════════════════════════════════════════════════════════

fn worker(jobs: &mpsc::Receiver<Job>, done: &mpsc::Sender<DoneMsg>) {
    while let Ok(job) = jobs.recv() {
        let msg = match job {
            Job::Thumb { key, epoch } => {
                let result = decode_image_file(Path::new(&key), THUMB_PX).map(|(px, _)| px);
                DoneMsg::Thumb { key, epoch, result }
            }
            Job::Preview { key, epoch } => {
                let result = build_preview(Path::new(&key));
                DoneMsg::Preview { key, epoch, result }
            }
        };
        if done.send(msg).is_err() {
            return; // The surface is gone; stop decoding.
        }
    }
}

/// Build the pane/quick-look payload for `path`, re-detecting the kind from
/// the file name so a misrouted job still resolves honestly.
fn build_preview(path: &Path) -> Result<PreviewData, String> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match PreviewKind::detect(&name, false) {
        PreviewKind::Image => decode_image_file(path, PREVIEW_PX)
            .map(|(pixels, full)| PreviewData::Image { pixels, full }),
        PreviewKind::Text(lang) => read_text_file(path).map(|(text, byte_capped)| {
            let (lines, line_capped) = tokenize(lang, &text);
            PreviewData::Text {
                lines,
                truncated: byte_capped || line_capped,
            }
        }),
        PreviewKind::Audio | PreviewKind::Video => probe_media_file(path).map(PreviewData::Media),
        PreviewKind::ImageNoDecoder => Err("no built-in decoder for this image format".to_string()),
        PreviewKind::Folder | PreviewKind::VideoNoProbe | PreviewKind::NoViewer(_) => {
            Err("nothing to decode for this type".to_string())
        }
    }
}

/// Read + decode + downscale an image file. Refuses over-cap files honestly.
///
/// # Errors
/// A human-readable reason: unreadable, over the size cap, or undecodable.
fn decode_image_file(path: &Path, max_px: u32) -> Result<(Pixels, [u32; 2]), String> {
    let len = std::fs::metadata(path)
        .map_err(|e| format!("unreadable: {e}"))?
        .len();
    if len > MAX_DECODE_BYTES {
        return Err(format!(
            "too large to decode ({} MB > {} MB cap)",
            len / (1024 * 1024),
            MAX_DECODE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("unreadable: {e}"))?;
    decode_image_bytes(&bytes, max_px)
}

/// Decode image `bytes` (format sniffed from magic bytes) and scale the long
/// side down to `max_px`. Returns the raster plus the original dimensions.
///
/// # Errors
/// A human-readable reason when the bytes aren't a decodable image.
#[allow(clippy::cast_possible_truncation)] // u32 → usize: the shell never targets <32-bit.
pub(crate) fn decode_image_bytes(bytes: &[u8], max_px: u32) -> Result<(Pixels, [u32; 2]), String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("unreadable image: {e}"))?;
    let img = reader
        .decode()
        .map_err(|e| format!("undecodable image: {e}"))?;
    let full = [img.width(), img.height()];
    let scaled = if img.width().max(img.height()) > max_px {
        img.thumbnail(max_px, max_px)
    } else {
        img
    };
    let rgba = scaled.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok((
        Pixels {
            size,
            rgba: Arc::new(rgba.into_raw()),
        },
        full,
    ))
}

/// Read up to [`TEXT_CAP_BYTES`] of a text file. A NUL byte marks it binary —
/// an honest refusal, not a garbled render. Returns the text and whether the
/// byte cap cut it short.
///
/// # Errors
/// A human-readable reason: unreadable, or a binary file.
pub(crate) fn read_text_file(path: &Path) -> Result<(String, bool), String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("unreadable: {e}"))?;
    let mut buf = vec![0_u8; TEXT_CAP_BYTES + 1];
    let mut read = 0;
    loop {
        let n = file
            .read(&mut buf[read..])
            .map_err(|e| format!("unreadable: {e}"))?;
        if n == 0 {
            break;
        }
        read += n;
        if read > TEXT_CAP_BYTES {
            break;
        }
    }
    let truncated = read > TEXT_CAP_BYTES;
    let slice = &buf[..read.min(TEXT_CAP_BYTES)];
    if slice.contains(&0) {
        return Err("binary file \u{2014} no text preview".to_string());
    }
    Ok((String::from_utf8_lossy(slice).into_owned(), truncated))
}

/// Probe media container headers with Symphonia — duration/codec/rate/channels
/// only, never a decode. Runs on the worker thread (it reads the file).
///
/// # Errors
/// A human-readable reason: unreadable, or a container Symphonia can't parse.
#[allow(clippy::cast_precision_loss)] // durations are far below 2^52 frames.
pub(crate) fn probe_media_file(path: &Path) -> Result<MediaMeta, String> {
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| format!("unreadable: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("unreadable container: {e}"))?;
    let mut meta = MediaMeta::default();
    for track in probed.format.tracks() {
        let p = &track.codec_params;
        if let (Some(tb), Some(n)) = (p.time_base, p.n_frames) {
            let t = tb.calc_time(n);
            let secs = t.seconds as f64 + t.frac;
            if meta.duration_secs.is_none_or(|cur| secs > cur) {
                meta.duration_secs = Some(secs);
            }
        }
        if meta.sample_rate.is_none() {
            meta.sample_rate = p.sample_rate;
        }
        if meta.channels.is_none() {
            meta.channels = p.channels.map(symphonia::core::audio::Channels::count);
        }
        if meta.codec.is_none() {
            meta.codec = symphonia::default::get_codecs()
                .get_codec(p.codec)
                .map(|d| d.short_name.to_string());
        }
    }
    Ok(meta)
}

// ═══════════════════════════════════════════════════════════════════════════
// The syntax-ish tokenizer (worker-side, so the paint path only lays out).
// ═══════════════════════════════════════════════════════════════════════════

/// Tokenize `text` into per-line spans under the line/line-length caps.
/// Returns the lines and whether a cap cut the text short.
#[must_use]
pub fn tokenize(lang: TextLang, text: &str) -> (Vec<Vec<TokenSpan>>, bool) {
    let mut lines = Vec::new();
    let mut capped = false;
    for (i, raw) in text.lines().enumerate() {
        if i >= TEXT_CAP_LINES {
            capped = true;
            break;
        }
        let line: String = if raw.chars().count() > TEXT_CAP_LINE_CHARS {
            capped = true;
            raw.chars().take(TEXT_CAP_LINE_CHARS).collect()
        } else {
            raw.to_string()
        };
        lines.push(tokenize_line(lang, &line));
    }
    (lines, capped)
}

fn tokenize_line(lang: TextLang, line: &str) -> Vec<TokenSpan> {
    if lang == TextLang::Markdown {
        let trimmed = line.trim_start();
        let kind = if trimmed.starts_with('#') {
            TokenKind::Heading
        } else if trimmed.starts_with('>') || trimmed.starts_with("```") {
            TokenKind::Comment
        } else {
            TokenKind::Plain
        };
        return vec![(line.to_string(), kind)];
    }

    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<TokenSpan> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Line comment → the rest of the line is one span.
        if let Some(prefix) = lang.comment_prefix() {
            if starts_with_at(&chars, i, prefix) {
                flush_plain(&mut spans, &mut plain);
                spans.push((chars[i..].iter().collect(), TokenKind::Comment));
                return spans;
            }
        }
        // String literal (simple: to the matching quote or end of line).
        if c == '"' || c == '\'' {
            flush_plain(&mut spans, &mut plain);
            let (lit, next) = scan_string(&chars, i, c);
            spans.push((lit, TokenKind::Str));
            i = next;
            continue;
        }
        // Numeric literal (only reachable at token start — the word scanner
        // below consumes digits inside identifiers).
        if c.is_ascii_digit() {
            flush_plain(&mut spans, &mut plain);
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            spans.push((chars[start..i].iter().collect(), TokenKind::Number));
            continue;
        }
        // Word: keyword or plain.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if lang.keywords().contains(&word.as_str()) {
                flush_plain(&mut spans, &mut plain);
                spans.push((word, TokenKind::Keyword));
            } else {
                plain.push_str(&word);
            }
            continue;
        }
        plain.push(c);
        i += 1;
    }
    flush_plain(&mut spans, &mut plain);
    spans
}

fn flush_plain(spans: &mut Vec<TokenSpan>, plain: &mut String) {
    if !plain.is_empty() {
        spans.push((std::mem::take(plain), TokenKind::Plain));
    }
}

fn starts_with_at(chars: &[char], at: usize, prefix: &str) -> bool {
    let mut idx = at;
    for pc in prefix.chars() {
        if chars.get(idx) != Some(&pc) {
            return false;
        }
        idx += 1;
    }
    true
}

/// Scan a quoted literal from `open` at `at`; returns the span (quotes
/// included) and the index just past it. `\` escapes the next char.
fn scan_string(chars: &[char], at: usize, quote: char) -> (String, usize) {
    let mut i = at + 1;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == quote {
            i += 1;
            break;
        }
        i += 1;
    }
    let end = i.min(chars.len());
    (chars[at..end].iter().collect(), end)
}

// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, Instant};

    // ── PreviewKind detection folds ──────────────────────────────────────────

    #[test]
    fn detect_folds_extensions_to_honest_kinds() {
        assert_eq!(PreviewKind::detect("dir/", true), PreviewKind::Folder);
        assert_eq!(PreviewKind::detect("Photo.PNG", false), PreviewKind::Image);
        assert_eq!(PreviewKind::detect("scan.tiff", false), PreviewKind::Image);
        // Image formats with no decoder in the lock are honestly split out.
        assert_eq!(
            PreviewKind::detect("live.heic", false),
            PreviewKind::ImageNoDecoder
        );
        assert_eq!(
            PreviewKind::detect("icon.svg", false),
            PreviewKind::ImageNoDecoder
        );
        assert_eq!(
            PreviewKind::detect("main.rs", false),
            PreviewKind::Text(TextLang::Rust)
        );
        assert_eq!(
            PreviewKind::detect("Makefile", false),
            PreviewKind::Text(TextLang::Plain)
        );
        assert_eq!(PreviewKind::detect("song.flac", false), PreviewKind::Audio);
        // isomp4 containers probe; other containers are metadata-only.
        assert_eq!(PreviewKind::detect("clip.mp4", false), PreviewKind::Video);
        assert_eq!(
            PreviewKind::detect("clip.mkv", false),
            PreviewKind::VideoNoProbe
        );
        assert_eq!(
            PreviewKind::detect("doc.pdf", false),
            PreviewKind::NoViewer("PDF")
        );
        assert_eq!(
            PreviewKind::detect("bundle.tar.gz", false),
            PreviewKind::NoViewer("archive")
        );
        assert_eq!(
            PreviewKind::detect("vm.qcow2", false),
            PreviewKind::NoViewer("disk image")
        );
        assert_eq!(
            PreviewKind::detect("mystery", false),
            PreviewKind::NoViewer("unknown type")
        );
        // Only decodable/probeable kinds go to the worker.
        assert!(PreviewKind::Image.worker_previews());
        assert!(PreviewKind::Text(TextLang::Plain).worker_previews());
        assert!(PreviewKind::Audio.worker_previews());
        assert!(PreviewKind::Video.worker_previews());
        assert!(!PreviewKind::ImageNoDecoder.worker_previews());
        assert!(!PreviewKind::VideoNoProbe.worker_previews());
        assert!(!PreviewKind::NoViewer("PDF").worker_previews());
        assert!(!PreviewKind::Folder.worker_previews());
    }

    // ── the bounded LRU: cap, hit, miss, recency ─────────────────────────────

    #[test]
    fn lru_is_bounded_and_evicts_least_recently_used() {
        let mut lru: Lru<u32> = Lru::new(3);
        lru.insert("a".into(), 1);
        lru.insert("b".into(), 2);
        lru.insert("c".into(), 3);
        assert_eq!(lru.len(), 3);
        // Hit: `a` is present; touching it makes `b` the eviction victim.
        assert_eq!(lru.get("a"), Some(&1));
        lru.touch("a");
        lru.insert("d".into(), 4);
        assert_eq!(lru.len(), 3, "cap holds");
        assert!(lru.get("b").is_none(), "least-recently-used evicted");
        assert_eq!(lru.get("a"), Some(&1), "touched entry survived");
        assert_eq!(lru.get("d"), Some(&4));
        // Miss: an unknown key is a plain None.
        assert!(lru.get("zzz").is_none());
        // Replacing a key must not grow the order queue (no double-entry).
        lru.insert("a".into(), 10);
        assert_eq!(lru.get("a"), Some(&10));
        assert_eq!(lru.len(), 3);
        lru.clear();
        assert!(lru.is_empty());
    }

    // ── image decode (in-memory: encode a PNG, decode + downscale it) ────────

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([180, 40, 40, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encode test png");
        bytes
    }

    #[test]
    fn decode_scales_the_long_side_down_and_reports_full_dims() {
        let (px, full) = decode_image_bytes(&png_bytes(32, 16), 8).expect("decode");
        assert_eq!(full, [32, 16]);
        assert_eq!(px.size, [8, 4], "aspect preserved at the cap");
        assert_eq!(px.rgba.len(), 8 * 4 * 4, "tight RGBA8");
        // Already-small images pass through unscaled.
        let (px, full) = decode_image_bytes(&png_bytes(6, 4), 8).expect("decode small");
        assert_eq!(full, [6, 4]);
        assert_eq!(px.size, [6, 4]);
    }

    #[test]
    fn decode_refuses_garbage_honestly() {
        let err = decode_image_bytes(b"not an image at all", 32).expect_err("must fail");
        assert!(!err.is_empty());
    }

    // ── text read: caps + binary refusal (real files on the worker's path) ───

    fn scratch_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mde-files-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir scratch");
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create scratch file");
        f.write_all(bytes).expect("write scratch file");
        path
    }

    #[test]
    fn text_read_returns_content_and_refuses_binary() {
        let path = scratch_file("note.txt", b"hello preview\nline two\n");
        let (text, truncated) = read_text_file(&path).expect("read text");
        assert!(text.contains("hello preview"));
        assert!(!truncated);

        let bin = scratch_file("blob.bin", &[0x7f, b'E', b'L', b'F', 0, 1, 2]);
        let err = read_text_file(&bin).expect_err("binary must refuse");
        assert!(err.contains("binary"), "honest reason, got: {err}");
    }

    // ── tokenizer folds ──────────────────────────────────────────────────────

    fn kinds_of(spans: &[TokenSpan]) -> Vec<(String, TokenKind)> {
        spans.to_vec()
    }

    #[test]
    fn tokenizer_classifies_keywords_strings_numbers_comments() {
        let (lines, capped) = tokenize(TextLang::Rust, "let x = 42; // answer\n");
        assert!(!capped);
        let spans = kinds_of(&lines[0]);
        assert!(spans.contains(&("let".to_string(), TokenKind::Keyword)));
        assert!(spans.contains(&("42".to_string(), TokenKind::Number)));
        assert!(spans.contains(&("// answer".to_string(), TokenKind::Comment)));

        let (lines, _) = tokenize(TextLang::Python, "name = \"mesh\"  # tag");
        let spans = kinds_of(&lines[0]);
        assert!(spans.contains(&("\"mesh\"".to_string(), TokenKind::Str)));
        assert!(spans.contains(&("# tag".to_string(), TokenKind::Comment)));

        // Markdown headings are line-classified.
        let (lines, _) = tokenize(TextLang::Markdown, "# Title\nbody");
        assert_eq!(lines[0], vec![("# Title".to_string(), TokenKind::Heading)]);
        assert_eq!(lines[1], vec![("body".to_string(), TokenKind::Plain)]);

        // Identifiers with digits stay plain (the digit is mid-word).
        let (lines, _) = tokenize(TextLang::Rust, "sha256sum");
        assert_eq!(lines[0], vec![("sha256sum".to_string(), TokenKind::Plain)]);
    }

    #[test]
    fn tokenizer_caps_lines_honestly() {
        let many = "x\n".repeat(TEXT_CAP_LINES + 10);
        let (lines, capped) = tokenize(TextLang::Plain, &many);
        assert_eq!(lines.len(), TEXT_CAP_LINES);
        assert!(capped);
        let long = "y".repeat(TEXT_CAP_LINE_CHARS + 5);
        let (lines, capped) = tokenize(TextLang::Plain, &long);
        assert!(capped);
        assert_eq!(lines[0][0].0.chars().count(), TEXT_CAP_LINE_CHARS);
    }

    // ── media probe: a real (hand-built) WAV through the Symphonia path ─────

    /// A minimal valid WAV: PCM s16le, mono, 8 kHz, exactly 1 s of silence.
    fn wav_bytes() -> Vec<u8> {
        let sample_rate: u32 = 8000;
        let data_len: u32 = sample_rate * 2; // 1 s of 16-bit mono
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1_u16.to_le_bytes()); // mono
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        out.extend_from_slice(&2_u16.to_le_bytes()); // block align
        out.extend_from_slice(&16_u16.to_le_bytes()); // bits/sample
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.resize(out.len() + data_len as usize, 0);
        out
    }

    #[test]
    fn media_probe_reads_duration_rate_and_channels_from_headers() {
        let path = scratch_file("tone.wav", &wav_bytes());
        let meta = probe_media_file(&path).expect("probe wav");
        let dur = meta.duration_secs.expect("duration from headers");
        assert!((dur - 1.0).abs() < 0.05, "1 s of 8 kHz mono, got {dur}");
        assert_eq!(meta.sample_rate, Some(8000));
        assert_eq!(meta.channels, Some(1));
    }

    #[test]
    fn media_probe_refuses_non_media_honestly() {
        let path = scratch_file("not-media.txt", b"just words");
        assert!(probe_media_file(&path).is_err());
    }

    // ── the worker round-trip: request → off-thread decode → pump → Ready ────

    fn pump_until<F: Fn(&Previews) -> bool>(p: &mut Previews, ok: F) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            p.pump();
            if ok(p) {
                return;
            }
            assert!(Instant::now() < deadline, "worker never delivered");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn thumb_request_decodes_off_thread_and_lands_ready() {
        let path = scratch_file("photo.png", &png_bytes(64, 32));
        let key = path.to_string_lossy().into_owned();
        let mut p = Previews::spawn();
        p.request_thumb(&key);
        assert_eq!(p.thumb(&key), Some(&ThumbState::Pending));
        assert!(p.any_pending());
        pump_until(&mut p, |p| {
            matches!(p.thumb(&key), Some(ThumbState::Ready { .. }))
        });
        let Some(ThumbState::Ready { pixels, .. }) = p.thumb(&key) else {
            unreachable!("pump_until proved the Ready state");
        };
        assert_eq!(pixels.size, [64, 32], "under the cap → unscaled");
        // A re-request of a warm slot is a cache hit (no new Pending).
        p.request_thumb(&key);
        assert!(matches!(p.thumb(&key), Some(ThumbState::Ready { .. })));
        assert!(!p.any_pending());
        // A refresh busts the cache (lock 18).
        p.clear();
        assert!(p.thumb(&key).is_none());
    }

    #[test]
    fn preview_request_fails_honestly_for_a_missing_file() {
        let mut p = Previews::spawn();
        p.request_preview("/nonexistent/mde-files-preview.png");
        pump_until(&mut p, |p| {
            matches!(
                p.preview("/nonexistent/mde-files-preview.png"),
                Some(PreviewState::Failed(_))
            )
        });
    }

    #[test]
    fn text_preview_round_trip_tokenizes_off_thread() {
        let path = scratch_file("snippet.rs", b"fn main() { let n = 7; } // demo\n");
        let key = path.to_string_lossy().into_owned();
        let mut p = Previews::spawn();
        p.request_preview(&key);
        pump_until(&mut p, |p| {
            matches!(p.preview(&key), Some(PreviewState::Ready { .. }))
        });
        let Some(PreviewState::Ready { data, .. }) = p.preview(&key) else {
            unreachable!("pump_until proved the Ready state");
        };
        let PreviewData::Text { lines, truncated } = data.as_ref() else {
            unreachable!("a .rs file previews as text");
        };
        assert!(!truncated);
        assert!(lines[0].contains(&("fn".to_string(), TokenKind::Keyword)));
    }
}
