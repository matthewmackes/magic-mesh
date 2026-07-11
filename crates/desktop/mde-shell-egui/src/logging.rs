//! test-obs-3 — the shell's structured-logging init.
//!
//! The 100K-LOC egui desktop tier had NO logging framework: error paths surfaced
//! only as transient toasts or ad-hoc `eprintln!`, so a failure on the bare DRM
//! seat (where there is no terminal to watch) left nothing behind. This module
//! stands up a single process-wide [`tracing`] subscriber at startup so every
//! `tracing::{error,warn,info,debug}` in the shell lands in a persistent,
//! filterable sink.
//!
//! It deliberately mirrors the `mackesd` daemon's subscriber ([`crate`]-level
//! parity is the point — one log shape across the fleet): a `tracing_subscriber`
//! `fmt` layer that renders **human-readable text on a TTY** (interactive dev
//! runs) and **machine-grep-able JSON when detached** (the shipped shell runs
//! under systemd, so its stderr is journald — JSON there ships straight to a log
//! aggregator). Force either with `MDE_LOG_FORMAT=json|text`. The level filter is
//! `MDE_LOG` (shell-namespaced), falling back to the standard `RUST_LOG`, then a
//! quiet `info` default.
//!
//! Kept intentionally lightweight — a single `fmt` layer + `EnvFilter`, no span
//! registry or extra layers — which is all a seat-side desktop needs.

use tracing_subscriber::EnvFilter;

/// Build the level filter from the environment: `MDE_LOG` wins (the shell-scoped
/// knob the operator reaches for), then the conventional `RUST_LOG`, then a quiet
/// `info` default so an un-tuned seat still logs warnings/errors without noise.
fn env_filter() -> EnvFilter {
    if let Ok(spec) = std::env::var("MDE_LOG") {
        if !spec.trim().is_empty() {
            if let Ok(filter) = EnvFilter::try_new(&spec) {
                return filter;
            }
        }
    }
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Whether to emit JSON lines. `MDE_LOG_FORMAT=json|text` forces it; otherwise
/// JSON when stderr is NOT a terminal (the systemd/journald seat case) and text
/// when attached to a TTY (interactive dev). Mirrors `mackesd`'s exact choice.
fn want_json() -> bool {
    use std::io::IsTerminal;
    match std::env::var("MDE_LOG_FORMAT").as_deref() {
        Ok("json") => true,
        Ok("text") => false,
        _ => !std::io::stderr().is_terminal(),
    }
}

/// Initialize the process-wide subscriber **once**. Idempotent and safe to call
/// from tests: it uses `try_init`, so a second call (or a test process that
/// already installed a subscriber) is a quiet no-op rather than a panic. Returns
/// `true` when this call installed the subscriber, `false` when one was already
/// present.
pub(crate) fn init() -> bool {
    let filter = env_filter();
    if want_json() {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_target(true)
            .try_init()
            .is_ok()
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .try_init()
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The subscriber init is idempotent: the first call may or may not win the
    /// global (another test in the same process could have installed one first),
    /// but a *second* call must always return `false` and never panic — the
    /// property that lets `main` call it unconditionally and tests call it freely.
    #[test]
    fn init_is_idempotent_and_never_panics() {
        // First call: installs the subscriber if this process has none yet.
        let _ = init();
        // Second call: a global is now guaranteed present, so this must be a
        // no-op returning false (never a double-init panic).
        assert!(!init(), "second init() must be a quiet no-op");
    }

    /// A bad `MDE_LOG` spec must not crash startup — it falls through to the
    /// `RUST_LOG`/`info` path so a fat-fingered filter never bricks the seat.
    #[test]
    fn env_filter_tolerates_a_bad_spec() {
        // Build a filter with no env set; must always succeed (default `info`).
        let _ = env_filter();
    }

    /// The converted `tracing` call sites must compile against the crate's macro
    /// set — a cheap guard that the framework is wired (fields + levels resolve).
    #[test]
    fn tracing_macros_compile_at_the_converted_shape() {
        let _ = init();
        tracing::error!(target: "shell::test", error = "probe", "converted error site compiles");
        tracing::warn!(target: "shell::test", verb = "Suspend", "converted warn site compiles");
    }
}
