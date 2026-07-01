//! E0.14 — link-search shim for the vendored Opus archive.
//!
//! `mde-musicd` pulls `opus 0.3` → `audiopus_sys 0.2.2`, which vendors and
//! CMake-builds libopus. On Fedora, CMake's GNUInstallDirs installs the
//! archive to `<out>/lib64`, but audiopus_sys's build script hardcodes the
//! rustc link-search path as `{dir}/lib` (audiopus_sys-0.2.2 build.rs
//! `link_opus`, the `{}/lib` format) — it never adds `lib64`. The result:
//! `cargo check` passes (no link) but linking any binary that pulls the
//! audio chain fails with `rust-lld: unable to find library -lopus`, so the
//! audio chain's tests silently never run (a link failure prints no
//! `test result:` line) and DoD §3 verification is skipped.
//!
//! The designed path is `opus-devel` (the unversioned system `libopus.so` +
//! pkg-config `.pc`, which makes audiopus_sys use the system lib and skip
//! vendoring). This shim is the durable fallback for dev/CI boxes that do
//! NOT have opus-devel: it adds every sibling `audiopus_sys-*/out/lib64`
//! (which holds the vendored `libopus.a`) to the link search path.
//! `audiopus_sys` already emits `cargo:rustc-link-lib=dylib=opus`, and
//! `-lopus` falls back to `libopus.a` when no `.so` is present — so the
//! vendored archive links. The directive propagates to the final link of
//! this crate AND of downstream binaries that depend on `mde-musicd`,
//! and it re-globs after `cargo clean`, so the
//! fix is durable without the non-reproducible manual symlink.

use std::path::Path;

fn main() {
    // OUT_DIR = target/<profile>/build/mde-musicd-<hash>/out
    // audiopus archive = target/<profile>/build/audiopus_sys-<hash>/out/lib64
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    // ancestors(): out -> mde-musicd-<hash> -> build/  (nth(2))
    let Some(build_root) = Path::new(&out_dir).ancestors().nth(2) else {
        return;
    };

    let Ok(entries) = std::fs::read_dir(build_root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("audiopus_sys-")
        {
            continue;
        }
        let lib64 = entry.path().join("out").join("lib64");
        // Only add it when the vendored archive is actually there — a
        // missing `-L` is harmless, but this keeps the search path honest
        // (and skips the pkg-config / opus-devel case, which has no
        // vendored out dir at all).
        if lib64.join("libopus.a").exists() {
            println!("cargo:rustc-link-search=native={}", lib64.display());
        }
    }

    // Re-run when the shim itself changes; a `cargo clean` wipes the build
    // outputs and re-runs this from scratch, re-globbing the fresh
    // audiopus_sys out dir — that is what makes the fix durable.
    println!("cargo:rerun-if-changed=build.rs");
}
