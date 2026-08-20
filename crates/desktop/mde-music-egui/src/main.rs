//! `mde-music-egui` binary entry point (§0.12 runtime reachability): stands up the
//! egui music surface. All behaviour lives in the library so the view-model stays
//! unit-testable; `main` is the thin Wayland-client launcher.
//!
//! The standalone review client is deliberately unprivileged. Music provider
//! credentials and playback authority belong to `mde-musicd`; allowing a root or
//! set-id desktop process to render the same surface would silently widen the UI
//! into a second authority. Linux exposes the kernel-owned launch identity in
//! `/proc/self/status`, which is validated before any egui/provider construction.

use std::io::{self, Read};

const STATUS_READ_LIMIT: u64 = 16 * 1024;

fn admitted_unprivileged_identity(status: &str) -> bool {
    let Some(uid_line) = status.lines().find(|line| line.starts_with("Uid:")) else {
        return false;
    };
    let mut ids = uid_line
        .split_ascii_whitespace()
        .skip(1)
        .map(str::parse::<u32>);
    let Some(Ok(real)) = ids.next() else {
        return false;
    };
    let Some(Ok(effective)) = ids.next() else {
        return false;
    };
    let Some(Ok(saved)) = ids.next() else {
        return false;
    };
    let Some(Ok(filesystem)) = ids.next() else {
        return false;
    };
    ids.next().is_none() && real != 0 && real == effective && real == saved && real == filesystem
}

fn validate_launch_authority() -> io::Result<()> {
    let file = std::fs::File::open("/proc/self/status")?;
    let mut status = String::new();
    file.take(STATUS_READ_LIMIT).read_to_string(&mut status)?;
    if admitted_unprivileged_identity(&status) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Music UI requires one unchanged, non-root seat identity; playback authority remains in mde-musicd",
        ))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    validate_launch_authority()?;
    mde_music_egui::run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::admitted_unprivileged_identity;

    #[test]
    fn hostile_setid_or_root_launch_cannot_inherit_music_ui_authority() {
        assert!(admitted_unprivileged_identity(
            "Name:\tmde-music-egui\nUid:\t1000\t1000\t1000\t1000\n"
        ));
        for status in [
            "Name:\tmde-music-egui\nUid:\t1000\t0\t0\t0\n",
            "Name:\tmde-music-egui\nUid:\t0\t0\t0\t0\n",
            "Name:\tmde-music-egui\nUid:\t1000\t1000\t0\t1000\n",
            "Name:\tmde-music-egui\nUid:\t1000\t1000\t1000\t0\n",
            "Name:\tmde-music-egui\nUid:\t1000\t1000\t1000\n",
            "Name:\tmde-music-egui\n",
        ] {
            assert!(
                !admitted_unprivileged_identity(status),
                "admitted {status:?}"
            );
        }
    }
}
