//! `mde-files` binary entry.
//!
//! Launches the Iced application that renders the Artifact Manager UI, or — when
//! invoked with `--pick` — the Open/Save file chooser (E10.3), the single file
//! engine the shell's `filedialog` subcommand now drives.
//!
//! Native file-ops parity (E11.6) is also reachable headlessly for scripting +
//! the shell's delete path: `--trash <path>…`, `--list-trash`, `--restore
//! <trash-name>…`, `--empty-trash` drive the freedesktop home trash directly,
//! `--properties <path>…` prints native file metadata, `--mounts` lists the
//! This-PC volumes parsed from `/proc/mounts`, `--search <root> <query>`
//! recursively finds matching entries, `--list-archive`/`--extract-archive`
//! read + unpack tar/tar.gz archives, `--copy`/`--move`/`--mkdir` are the
//! native local file operations, `--thumbnail` generates a freedesktop
//! thumbnail, and `--open-with <file>` resolves the default handler app.

use std::process::ExitCode;

use mde_files::trash::TrashDir;
use mde_files::MdeFiles;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Native trash ops short-circuit the GUI (the --pick pattern).
    match args.first().map(String::as_str) {
        Some("--trash") => return trash_paths(&args[1..]),
        Some("--list-trash") => return list_trash(),
        Some("--restore") => return restore_names(&args[1..]),
        Some("--empty-trash") => return empty_trash(),
        Some("--properties") => return properties(&args[1..]),
        Some("--mounts") => return mounts(),
        Some("--search") => return search(&args[1..]),
        Some("--list-archive") => return list_archive(&args[1..]),
        Some("--extract-archive") => return extract_archive(&args[1..]),
        Some("--copy") => return copy_op(&args[1..]),
        Some("--move") => return move_op(&args[1..]),
        Some("--mkdir") => return mkdir_op(&args[1..]),
        Some("--thumbnail") => return thumbnail_op(&args[1..]),
        Some("--open-with") => return open_with_op(&args[1..]),
        Some("--bookmarks") => return bookmarks_op(),
        _ => {}
    }
    if args.iter().any(|a| a == "--pick") {
        // The chooser prints the chosen path to stdout + exits 0 (non-zero on
        // Cancel) — the contract `mde filedialog` execs against.
        return match mde_files::picker::run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    match MdeFiles::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mde-files: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Open the home trash, reporting a failure as a non-zero exit.
fn home_trash() -> Result<TrashDir, ExitCode> {
    TrashDir::home().map_err(|e| {
        eprintln!("mde-files: cannot open trash: {e}");
        ExitCode::FAILURE
    })
}

/// `--trash <path>…` — move each path to the trash.
fn trash_paths(paths: &[String]) -> ExitCode {
    if paths.is_empty() {
        eprintln!("usage: mde-files --trash <path> [path ...]");
        return ExitCode::FAILURE;
    }
    let trash = match home_trash() {
        Ok(t) => t,
        Err(code) => return code,
    };
    let mut failed = false;
    for p in paths {
        match trash.trash(std::path::Path::new(p)) {
            Ok(item) => println!("trashed {p} -> {}", item.trash_name),
            Err(e) => {
                eprintln!("mde-files: cannot trash {p}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `--list-trash` — print the recoverable items, one per line.
fn list_trash() -> ExitCode {
    let trash = match home_trash() {
        Ok(t) => t,
        Err(code) => return code,
    };
    match trash.list() {
        Ok(items) => {
            for item in items {
                println!(
                    "{}\t{}\t{}",
                    item.trash_name,
                    item.deletion_date,
                    item.original_path.display()
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot read trash: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--restore <trash-name>…` — restore each named item to its original path.
fn restore_names(names: &[String]) -> ExitCode {
    if names.is_empty() {
        eprintln!("usage: mde-files --restore <trash-name> [trash-name ...]");
        return ExitCode::FAILURE;
    }
    let trash = match home_trash() {
        Ok(t) => t,
        Err(code) => return code,
    };
    let items = match trash.list() {
        Ok(items) => items,
        Err(e) => {
            eprintln!("mde-files: cannot read trash: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for name in names {
        match items.iter().find(|i| &i.trash_name == name) {
            Some(item) => match trash.restore(item) {
                Ok(()) => println!("restored {name} -> {}", item.original_path.display()),
                Err(e) => {
                    eprintln!("mde-files: cannot restore {name}: {e}");
                    failed = true;
                }
            },
            None => {
                eprintln!("mde-files: no trashed item named {name}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `--properties <path>…` — print each path's native file properties.
fn properties(paths: &[String]) -> ExitCode {
    if paths.is_empty() {
        eprintln!("usage: mde-files --properties <path> [path ...]");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            println!();
        }
        match mde_files::properties::FileProperties::of(std::path::Path::new(p)) {
            Ok(props) => print!("{}", mde_files::properties::report(&props)),
            Err(e) => {
                eprintln!("mde-files: cannot stat {p}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `--search <root> <query>` — recursively find entries whose name matches
/// `query` under `root`, printing `path` (with a trailing `/` for directories).
fn search(args: &[String]) -> ExitCode {
    let (Some(root), Some(query)) = (args.first(), args.get(1)) else {
        eprintln!("usage: mde-files --search <root> <query>");
        return ExitCode::FAILURE;
    };
    let opts = mde_files::search::SearchOptions::default();
    let hits = mde_files::search::search_tree(std::path::Path::new(root), query, &opts);
    for hit in &hits {
        println!(
            "{}{}",
            hit.path.display(),
            if hit.is_dir { "/" } else { "" }
        );
    }
    ExitCode::SUCCESS
}

/// `--bookmarks` — print the user's GTK sidebar bookmarks (saved places).
fn bookmarks_op() -> ExitCode {
    for bm in mde_files::bookmarks::user_bookmarks() {
        println!(
            "{}\t{}{}",
            bm.label,
            bm.uri,
            bm.path
                .map(|p| format!("\t{}", p.display()))
                .unwrap_or_default()
        );
    }
    ExitCode::SUCCESS
}

/// `--open-with <file>` — resolve the file's MIME + its default handler app,
/// printing the MIME, the `.desktop` id, and the launch command (does not launch).
fn open_with_op(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("usage: mde-files --open-with <file>");
        return ExitCode::FAILURE;
    };
    use mde_files::desktop;
    let p = std::path::Path::new(path);
    let Some(mime) = mde_files::mime::detect(p) else {
        eprintln!("mde-files: unknown file type for {path}");
        return ExitCode::FAILURE;
    };
    println!("mime: {mime}");
    match desktop::default_entry(mime, &desktop::config_dirs(), &desktop::data_dirs()) {
        Some(entry) => {
            println!("app: {} ({})", entry.name, entry.id);
            println!("exec: {}", entry.command(&[path]).join(" "));
            ExitCode::SUCCESS
        }
        None => {
            println!("app: (no default handler registered for {mime})");
            ExitCode::SUCCESS
        }
    }
}

/// `--thumbnail <image> [--large]` — generate a freedesktop thumbnail, printing
/// the cache path it wrote.
fn thumbnail_op(args: &[String]) -> ExitCode {
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: mde-files --thumbnail <image> [--large]");
        return ExitCode::FAILURE;
    };
    let size = if args.iter().any(|a| a == "--large") {
        mde_files::thumbnails::ThumbSize::Large
    } else {
        mde_files::thumbnails::ThumbSize::Normal
    };
    match mde_files::thumbnails::generate(std::path::Path::new(path), size) {
        Ok(dest) => {
            println!("{}", dest.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot thumbnail {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--copy <src> <dst>` — copy a file or directory tree.
fn copy_op(args: &[String]) -> ExitCode {
    let (Some(src), Some(dst)) = (args.first(), args.get(1)) else {
        eprintln!("usage: mde-files --copy <src> <dst>");
        return ExitCode::FAILURE;
    };
    match mde_files::fileops::copy(std::path::Path::new(src), std::path::Path::new(dst)) {
        Ok(()) => {
            println!("copied {src} -> {dst}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot copy {src}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--move <src> <dst>` — move/rename a file or directory.
fn move_op(args: &[String]) -> ExitCode {
    let (Some(src), Some(dst)) = (args.first(), args.get(1)) else {
        eprintln!("usage: mde-files --move <src> <dst>");
        return ExitCode::FAILURE;
    };
    match mde_files::fileops::move_path(std::path::Path::new(src), std::path::Path::new(dst)) {
        Ok(()) => {
            println!("moved {src} -> {dst}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot move {src}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--mkdir <path>` — create a directory (and parents).
fn mkdir_op(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("usage: mde-files --mkdir <path>");
        return ExitCode::FAILURE;
    };
    match mde_files::fileops::make_dir(std::path::Path::new(path)) {
        Ok(()) => {
            println!("created {path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot mkdir {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--list-archive <file>` — print a tar/tar.gz archive's members.
fn list_archive(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("usage: mde-files --list-archive <archive.tar[.gz]>");
        return ExitCode::FAILURE;
    };
    match mde_files::archive::list(std::path::Path::new(path)) {
        Ok(entries) => {
            for e in &entries {
                println!(
                    "{}\t{}{}",
                    e.path.display(),
                    e.size,
                    if e.is_dir { "\t(dir)" } else { "" }
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot read {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--extract-archive <file> <dest>` — extract a tar/tar.gz archive into `dest`.
fn extract_archive(args: &[String]) -> ExitCode {
    let (Some(path), Some(dest)) = (args.first(), args.get(1)) else {
        eprintln!("usage: mde-files --extract-archive <archive.tar[.gz]> <dest-dir>");
        return ExitCode::FAILURE;
    };
    match mde_files::archive::extract(std::path::Path::new(path), std::path::Path::new(dest)) {
        Ok(n) => {
            println!("extracted {n} member(s) to {dest}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot extract {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--mounts` — print the user-facing volumes (This PC), one per line.
fn mounts() -> ExitCode {
    print!(
        "{}",
        mde_files::mounts::report(&mde_files::mounts::user_volumes())
    );
    ExitCode::SUCCESS
}

/// `--empty-trash` — permanently delete everything in the trash.
fn empty_trash() -> ExitCode {
    let trash = match home_trash() {
        Ok(t) => t,
        Err(code) => return code,
    };
    match trash.empty() {
        Ok(n) => {
            println!("emptied trash ({n} item(s))");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mde-files: cannot empty trash: {e}");
            ExitCode::FAILURE
        }
    }
}
