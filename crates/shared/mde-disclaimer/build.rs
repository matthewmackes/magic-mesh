//! PKG-6 (E8 pre-flight gate) — refuse to BUILD without a non-empty
//! `DISCLAIMER.md`. `include_str!` already fails if the file is
//! absent; this additionally fails the build when it exists but is
//! empty/whitespace, so a stripped disclaimer can never ship in the
//! RPM (the install-time accept screen, PKG-6's other half, then
//! shows this same text). Reruns whenever the file changes.

use std::path::Path;

fn main() {
    let path = Path::new("../../../DISCLAIMER.md");
    println!("cargo:rerun-if-changed={}", path.display());
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {
            // Non-empty disclaimer present — the gate passes.
        }
        Ok(_) => panic!(
            "PKG-6 gate: DISCLAIMER.md is empty — refusing to build. \
             A shipped Magic Mesh must carry its disclaimer."
        ),
        Err(e) => panic!("PKG-6 gate: cannot read DISCLAIMER.md ({e}) — refusing to build."),
    }
}
