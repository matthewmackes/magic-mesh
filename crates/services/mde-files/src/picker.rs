//! The Open / Save file chooser — folded onto the one mde-files engine (E10.3).
//!
//! `mde-files --pick [--save] [--title T] [--dir D] [--filename F]
//!                   [--filter "Images:png,jpg;All Files:*"]`
//!
//! A self-contained chooser that reuses the same local listing engine the
//! manager browses with (`LocalFsBackend::list_dir`) and the same IBM Carbon
//! Gray-100 theme, so the platform has ONE file engine rather than a separate dialog.
//! It prints the chosen absolute path to stdout and exits 0, or exits non-zero
//! on Cancel — the exact stdout/exit contract a portal/file-dialog caller
//! spawns this binary against.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cosmic_compat::{ButtonSty, ContainerSty, SvgSty, TextSty};
use cosmic::app::ApplicationExt;
use cosmic::iced::widget::{
    button, container, pick_list, scrollable, svg, text, text_input, Column, Row,
};
use cosmic::iced::{Background, Border, Color, Length, Task};
use cosmic::{Application, Element, Theme};

use crate::backend::LocalFsBackend;
use crate::theme as t;

// ── model ──────────────────────────────────────────────────────────────────

struct Entry {
    /// Display name (no trailing `/`).
    name: String,
    path: PathBuf,
    is_dir: bool,
}

#[derive(Clone)]
struct Filter {
    label: String,
    /// Lowercased extensions, or a single `*` meaning all files.
    exts: Vec<String>,
}

impl Filter {
    fn accepts(&self, name: &str) -> bool {
        if self.exts.iter().any(|e| e == "*") {
            return true;
        }
        let ext = Path::new(name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        self.exts.contains(&ext)
    }

    fn pattern(&self) -> String {
        if self.exts.iter().any(|e| e == "*") {
            "*.*".to_string()
        } else {
            self.exts
                .iter()
                .map(|e| format!("*.{e}"))
                .collect::<Vec<_>>()
                .join(";")
        }
    }
}

/// pick_list option for the "Look in:" ancestor dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathChoice(PathBuf);
impl std::fmt::Display for PathChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.file_name().and_then(|s| s.to_str()) {
            Some(n) => f.write_str(n),
            None => f.write_str("/ (Computer)"),
        }
    }
}

/// pick_list option for the "Files of type:" filter dropdown.
#[derive(Debug, Clone)]
struct FilterChoice {
    idx: usize,
    label: String,
}
impl PartialEq for FilterChoice {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}
impl std::fmt::Display for FilterChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

struct Picker {
    /// GUI-7 — the libcosmic application core (set from `init`).
    core: cosmic::app::Core,
    save: bool,
    title: String,
    current: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    last_click: Option<(usize, Instant)>,
    filename: String,
    filters: Vec<Filter>,
    filter_idx: usize,
}

/// GUI-7 — the parsed chooser config handed to `Application::init` as the flag.
struct PickerFlags {
    save: bool,
    title: String,
    current: PathBuf,
    entries: Vec<Entry>,
    filename: String,
    filters: Vec<Filter>,
}

#[derive(Debug, Clone)]
enum Message {
    LookIn(PathChoice),
    Place(usize),
    Up,
    ClickEntry(usize),
    FilenameChanged(String),
    SetFilter(FilterChoice),
    Accept,
    Cancel,
}

// ── CLI dispatch ────────────────────────────────────────────────────────────

/// Parse the `--pick …` argv tail and run the chooser. The `--pick` flag itself
/// (consumed by `main`) may still be present; it is ignored here.
pub fn run(args: &[String]) -> cosmic::iced::Result {
    let mut save = false;
    let mut title = String::new();
    let mut dir: Option<PathBuf> = None;
    let mut filename = String::new();
    let mut filter_spec = String::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pick" => {}
            "--save" => save = true,
            "--title" => title = it.next().cloned().unwrap_or_default(),
            "--dir" => dir = it.next().map(PathBuf::from),
            "--filename" => filename = it.next().cloned().unwrap_or_default(),
            "--filter" => filter_spec = it.next().cloned().unwrap_or_default(),
            _ => {}
        }
    }
    let filters = parse_filters(&filter_spec);
    let current = dir
        .filter(|d| d.is_dir())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let title = if title.is_empty() {
        if save {
            "Save As".to_string()
        } else {
            "Open".to_string()
        }
    } else {
        title
    };

    let entries = read_entries(&current, &filters[0]);
    let flags = PickerFlags {
        save,
        title,
        current,
        entries,
        filename,
        filters,
    };
    cosmic::app::run::<Picker>(cosmic::app::Settings::default(), flags)
}

impl Application for Picker {
    type Executor = cosmic::executor::Default;
    type Flags = PickerFlags;
    type Message = Message;
    const APP_ID: &'static str = "com.mackes.MagicMeshFilePicker";

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::app::Core,
        flags: Self::Flags,
    ) -> (Self, cosmic::app::Task<Self::Message>) {
        let mut picker = Picker {
            core,
            save: flags.save,
            title: flags.title,
            current: flags.current,
            entries: flags.entries,
            selected: None,
            last_click: None,
            filename: flags.filename,
            filters: flags.filters,
            filter_idx: 0,
        };
        // Keep the chooser's own chrome; suppress Cosmic's headerbar. The
        // parsed title still names the window (Open / Save As).
        picker.core.window.show_headerbar = false;
        let win_title = picker.title.clone();
        picker.set_header_title(win_title);
        (picker, cosmic::app::Task::none())
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Self::Message> {
        // Delegate to the free reducer, then lift into the cosmic Action space.
        update(self, message).map(cosmic::Action::App)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        view(self)
    }
}

/// Parse `"Images:png,jpg;All Files:*"` into filters; default to All Files.
fn parse_filters(spec: &str) -> Vec<Filter> {
    let mut out = Vec::new();
    for group in spec.split(';').filter(|s| !s.trim().is_empty()) {
        let (label, exts) = group.split_once(':').unwrap_or((group, "*"));
        let exts: Vec<String> = exts
            .split(',')
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        let exts = if exts.is_empty() {
            vec!["*".to_string()]
        } else {
            exts
        };
        let pattern = Filter {
            label: String::new(),
            exts: exts.clone(),
        }
        .pattern();
        out.push(Filter {
            label: format!("{} ({pattern})", label.trim()),
            exts,
        });
    }
    if out.is_empty() {
        out.push(Filter {
            label: "All Files (*.*)".to_string(),
            exts: vec!["*".to_string()],
        });
    }
    out
}

// ── filesystem (via the shared engine) ──────────────────────────────────────

/// List `dir` through `LocalFsBackend::list_dir` (the same engine the manager
/// browses with), then apply the classic-dialog presentation: folders first,
/// then files matching the active filter, each group sorted by name. The
/// backend tags dir rows with a trailing `/`, which we strip for display.
fn read_entries(dir: &Path, filter: &Filter) -> Vec<Entry> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for row in LocalFsBackend::list_dir(dir) {
        let Some(path) = row.path.clone() else {
            continue;
        };
        let is_dir = row.is_dir();
        let name = row.name.trim_end_matches('/').to_string();
        if name.starts_with('.') {
            continue; // classic dialogs hide dotfiles
        }
        let entry = Entry {
            name,
            path: PathBuf::from(path),
            is_dir,
        };
        if is_dir {
            dirs.push(entry);
        } else if filter.accepts(&entry.name) {
            files.push(entry);
        }
    }
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);
    dirs
}

fn navigate(state: &mut Picker, dir: PathBuf) {
    state.current = dir;
    state.entries = read_entries(&state.current, &state.filters[state.filter_idx]);
    state.selected = None;
    state.last_click = None;
    state.filename.clear();
}

/// Look-in dropdown options: the current folder, then each ancestor up to root.
fn ancestors(dir: &Path) -> Vec<PathChoice> {
    dir.ancestors()
        .map(|p| PathChoice(p.to_path_buf()))
        .collect()
}

/// Quick-access destinations that exist, as (label, icon, path).
fn places() -> Vec<(&'static str, &'static [u8], PathBuf)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    [
        ("Home", crate::icons::HOME, home.clone()),
        ("Documents", crate::icons::FOLDER, home.join("Documents")),
        ("Downloads", crate::icons::DOWNLOAD, home.join("Downloads")),
        ("Pictures", crate::icons::IMAGE_FILE, home.join("Pictures")),
        ("Computer", crate::icons::HDD, PathBuf::from("/")),
    ]
    .into_iter()
    .filter(|(_, _, p)| p.is_dir())
    .collect()
}

/// Resolve the path the Open/Save button commits to: a typed absolute path wins,
/// a typed relative name joins the current dir, and an empty box falls back to
/// the highlighted entry. `None` means there is nothing to commit.
fn resolve_target(filename: &str, current: &Path, selected: Option<&Path>) -> Option<PathBuf> {
    let name = filename.trim();
    if name.is_empty() {
        selected.map(Path::to_path_buf)
    } else if Path::new(name).is_absolute() {
        Some(PathBuf::from(name))
    } else {
        Some(current.join(name))
    }
}

/// Finish: print the chosen path and exit (the contract with the caller).
fn accept_path(p: &Path) -> ! {
    println!("{}", p.display());
    std::process::exit(0)
}

// ── update ──────────────────────────────────────────────────────────────────

fn update(state: &mut Picker, message: Message) -> Task<Message> {
    match message {
        Message::LookIn(PathChoice(p)) => navigate(state, p),
        Message::Place(i) => {
            if let Some((_, _, p)) = places().into_iter().nth(i) {
                navigate(state, p);
            }
        }
        Message::Up => {
            if let Some(parent) = state.current.parent().map(Path::to_path_buf) {
                navigate(state, parent);
            }
        }
        Message::ClickEntry(i) => {
            let now = Instant::now();
            let dbl = state
                .last_click
                .map(|(li, lt)| li == i && now.duration_since(lt) < Duration::from_millis(400))
                .unwrap_or(false);
            state.selected = Some(i);
            if let Some(e) = state.entries.get(i) {
                if !e.is_dir {
                    state.filename = e.name.clone();
                }
                if dbl {
                    if e.is_dir {
                        let path = e.path.clone();
                        navigate(state, path);
                        return Task::none();
                    }
                    accept_path(&e.path);
                }
            }
            state.last_click = Some((i, now));
        }
        Message::FilenameChanged(s) => state.filename = s,
        Message::SetFilter(FilterChoice { idx, .. }) => {
            state.filter_idx = idx.min(state.filters.len().saturating_sub(1));
            state.entries = read_entries(&state.current, &state.filters[state.filter_idx]);
            state.selected = None;
        }
        Message::Accept => {
            let selected = state
                .selected
                .and_then(|i| state.entries.get(i))
                .map(|e| e.path.as_path());
            let target = resolve_target(&state.filename, &state.current, selected);
            if let Some(target) = target {
                if target.is_dir() {
                    navigate(state, target); // a folder: drill in rather than choose
                } else {
                    accept_path(&target); // a file (existing, or to-create on Save)
                }
            }
        }
        Message::Cancel => std::process::exit(1),
    }
    Task::none()
}

// ── view ────────────────────────────────────────────────────────────────────

const ROW_PX: f32 = 13.0;

fn glyph<'a>(bytes: &'static [u8], color: Color) -> Element<'a, Message> {
    svg(crate::icons::handle(bytes))
        .width(Length::Fixed(16.0))
        .height(Length::Fixed(16.0))
        .sty(move |_t: &Theme| svg::Style { color: Some(color) })
        .into()
}

fn surface_style(bg: Color, border: Color) -> impl Fn(&Theme) -> container::Style {
    move |_t| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 2.0.into(),
        },
        ..container::Style::default()
    }
}

fn primary_button_style(_t: &Theme, status: button::Status) -> button::Style {
    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        snap: false,
        background: Some(Background::Color(if hot {
            t::ACCENT_HI
        } else {
            t::ACCENT
        })),
        text_color: t::PF_BG_100,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        ..button::Style::default()
    }
}

fn secondary_button_style(_t: &Theme, status: button::Status) -> button::Style {
    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        snap: false,
        background: Some(Background::Color(if hot {
            t::PF_BG_400
        } else {
            t::PF_BG_300
        })),
        text_color: t::FG,
        border: Border {
            color: t::PF_BORDER,
            width: 1.0,
            radius: 2.0.into(),
        },
        ..button::Style::default()
    }
}

fn row_button_style(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_t, status| {
        let hot = selected || matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            snap: false,
            background: hot.then_some(Background::Color(if selected {
                t::ACTIVE_RUST_BG
            } else {
                t::ROW_HOVER
            })),
            text_color: t::FG,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 2.0.into(),
            },
            ..button::Style::default()
        }
    }
}

fn entry_row<'a>(state: &Picker, i: usize, e: &Entry) -> Element<'a, Message> {
    let (icon_bytes, color) = if e.is_dir {
        (crate::icons::FOLDER, t::ACCENT)
    } else if is_image(&e.name) {
        (crate::icons::IMAGE_FILE, t::FG_DIM)
    } else {
        (crate::icons::DOC2, t::FG_DIM)
    };
    let row = Row::new()
        .spacing(8.0)
        .align_y(cosmic::iced::Alignment::Center)
        .push(glyph(icon_bytes, color))
        .push(text(e.name.clone()).size(ROW_PX).width(Length::Fill));
    button(row)
        .on_press(Message::ClickEntry(i))
        .width(Length::Fill)
        .padding(cosmic::iced::Padding {
            top: 3.0,
            right: 8.0,
            bottom: 3.0,
            left: 6.0,
        })
        .sty(row_button_style(state.selected == Some(i)))
        .into()
}

fn is_image(name: &str) -> bool {
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif"
    )
}

fn view(state: &Picker) -> Element<'_, Message> {
    // Look in: ancestor dropdown + Up button.
    let look_in = Row::new()
        .spacing(8.0)
        .align_y(cosmic::iced::Alignment::Center)
        .push(text("Look in:").size(ROW_PX).colr(t::FG_DIM))
        .push(
            pick_list(
                ancestors(&state.current),
                Some(PathChoice(state.current.clone())),
                Message::LookIn,
            )
            .text_size(ROW_PX)
            .width(Length::Fill),
        )
        .push(
            button(glyph(crate::icons::ARROW_LEFT, t::FG))
                .on_press(Message::Up)
                .padding(cosmic::iced::Padding {
                    top: 4.0,
                    right: 6.0,
                    bottom: 4.0,
                    left: 6.0,
                })
                .sty(secondary_button_style),
        );

    // Quick-access places column.
    let mut places_col = Column::new()
        .spacing(2.0)
        .padding(6.0)
        .width(Length::Fixed(116.0));
    for (i, (label, icon, _)) in places().into_iter().enumerate() {
        places_col = places_col.push(
            button(
                Row::new()
                    .spacing(8.0)
                    .align_y(cosmic::iced::Alignment::Center)
                    .push(glyph(icon, t::FG_DIM))
                    .push(text(label.to_string()).size(ROW_PX)),
            )
            .on_press(Message::Place(i))
            .width(Length::Fill)
            .padding(cosmic::iced::Padding {
                top: 4.0,
                right: 6.0,
                bottom: 4.0,
                left: 6.0,
            })
            .sty(row_button_style(false)),
        );
    }
    let places_pane = container(places_col)
        .sty(surface_style(t::WINDOW_SIDE, t::PF_BORDER))
        .height(Length::Fill);

    // File list well.
    let mut list = Column::new().spacing(0.0);
    if state.entries.is_empty() {
        // Route the chooser's empty directory through the same shared empty-state
        // renderer the manager views use, so a zero-entry folder reads as a
        // deliberate state, not a stray "(empty)" label.
        list = list.push(crate::widgets::empty_state(
            mde_theme::EmptyState::info(
                "This folder is empty",
                "No files or folders to show here.",
            )
            .with_icon(mde_theme::Icon::Folder),
            None::<Message>,
        ));
    }
    for (i, e) in state.entries.iter().enumerate() {
        list = list.push(entry_row(state, i, e));
    }
    let well = container(scrollable(list).height(Length::Fill))
        .sty(surface_style(t::WINDOW, t::PF_BORDER))
        .padding(2.0)
        .width(Length::Fill)
        .height(Length::Fill);

    let middle = Row::new().spacing(8.0).push(places_pane).push(well);

    // File name row + Open/Save button.
    let accept_label = if state.save { "Save" } else { "Open" };
    let name_row = Row::new()
        .spacing(8.0)
        .align_y(cosmic::iced::Alignment::Center)
        .push(
            text("File name:")
                .size(ROW_PX)
                .colr(t::FG_DIM)
                .width(Length::Fixed(72.0)),
        )
        .push(
            text_input("", &state.filename)
                .on_input(Message::FilenameChanged)
                .on_submit(Message::Accept)
                .size(ROW_PX)
                .width(Length::Fill),
        )
        .push(
            button(text(accept_label).size(ROW_PX))
                .on_press(Message::Accept)
                .padding(cosmic::iced::Padding {
                    top: 5.0,
                    right: 14.0,
                    bottom: 5.0,
                    left: 14.0,
                })
                .sty(primary_button_style),
        );

    // Files of type row + Cancel button.
    let cur_filter = FilterChoice {
        idx: state.filter_idx,
        label: state.filters[state.filter_idx].label.clone(),
    };
    let filter_opts: Vec<FilterChoice> = state
        .filters
        .iter()
        .enumerate()
        .map(|(i, f)| FilterChoice {
            idx: i,
            label: f.label.clone(),
        })
        .collect();
    let type_row = Row::new()
        .spacing(8.0)
        .align_y(cosmic::iced::Alignment::Center)
        .push(
            text("Files of type:")
                .size(ROW_PX)
                .colr(t::FG_DIM)
                .width(Length::Fixed(72.0)),
        )
        .push(
            pick_list(filter_opts, Some(cur_filter), Message::SetFilter)
                .text_size(ROW_PX)
                .width(Length::Fill),
        )
        .push(
            button(text("Cancel").size(ROW_PX))
                .on_press(Message::Cancel)
                .padding(cosmic::iced::Padding {
                    top: 5.0,
                    right: 14.0,
                    bottom: 5.0,
                    left: 14.0,
                })
                .sty(secondary_button_style),
        );

    let body = Column::new()
        .spacing(8.0)
        .padding(12.0)
        .push(look_in)
        .push(container(middle).height(Length::Fill))
        .push(name_row)
        .push(type_row);

    container(body)
        .sty(|_t| container::Style {
            snap: false,
            background: Some(Background::Color(t::BG)),
            ..container::Style::default()
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_default_to_all_files_when_empty() {
        let f = parse_filters("");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].label, "All Files (*.*)");
        assert!(f[0].accepts("anything.xyz"));
    }

    #[test]
    fn filters_parse_groups_and_extensions() {
        let f = parse_filters("Images:png,jpg;All Files:*");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].label, "Images (*.png;*.jpg)");
        assert!(f[0].accepts("photo.PNG")); // case-insensitive
        assert!(!f[0].accepts("notes.txt"));
        assert!(f[1].accepts("notes.txt")); // All Files
    }

    #[test]
    fn read_entries_lists_folders_first_then_filtered_files() {
        let dir = std::env::temp_dir().join(format!("mde-files-picker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("keep.png"), b"x").unwrap();
        std::fs::write(dir.join("skip.txt"), b"x").unwrap();
        std::fs::write(dir.join(".hidden"), b"x").unwrap();

        let filter = Filter {
            label: String::new(),
            exts: vec!["png".to_string()],
        };
        let entries = read_entries(&dir, &filter);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // Folder first (no trailing slash), then the .png; .txt filtered out,
        // dotfile hidden.
        assert_eq!(names, vec!["sub", "keep.png"]);
        assert!(entries[0].is_dir);
        assert!(!entries[1].is_dir);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_target_prefers_typed_then_falls_back_to_selection() {
        let cur = Path::new("/home/u");
        // Absolute typed path wins.
        assert_eq!(
            resolve_target("/etc/hosts", cur, None),
            Some(PathBuf::from("/etc/hosts"))
        );
        // Relative name joins the current dir.
        assert_eq!(
            resolve_target("note.txt", cur, None),
            Some(cur.join("note.txt"))
        );
        // Empty box falls back to the highlighted entry.
        let sel = Path::new("/home/u/pic.png");
        assert_eq!(
            resolve_target("   ", cur, Some(sel)),
            Some(sel.to_path_buf())
        );
        // Empty box, nothing selected -> nothing to commit.
        assert_eq!(resolve_target("", cur, None), None);
    }
}
