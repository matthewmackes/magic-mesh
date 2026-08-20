//! App VM entrypoint for the guest-only Wayland workspace supervisor.

use std::path::Path;

use mde_wayland_workspace::{run, run_session, WorkspacePaths};

fn main() {
    let paths = WorkspacePaths::default();
    let result = match std::env::args().nth(1).as_deref() {
        Some("--session-child") => run_session(&paths),
        None => std::env::current_exe()
            .map_err(|source| mde_wayland_workspace::WorkspaceError::Io {
                operation: "resolve workspace executable",
                source,
            })
            .and_then(|executable| run(&paths, Path::new(&executable))),
        Some(_) => {
            eprintln!("mde-wayland-workspace: unexpected argument");
            std::process::exit(2);
        }
    };
    match result {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("mde-wayland-workspace: {error}");
            std::process::exit(1);
        }
    }
}
