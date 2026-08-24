//! `Enroll` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `enroll` subcommand.
#[allow(unreachable_code)]
pub fn run(
    passcode: Option<String>,
    passcode_stdin: bool,
    token: Option<String>,
    token_stdin: bool,
    name: Option<String>,
    workgroup_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    {
        // EFF-21 — stdin intake keeps the secret out of
        // /proc/<pid>/cmdline + shell history. clap's conflict
        // rules guarantee at most one source is set.
        let passcode = if passcode_stdin {
            Some(read_secret_line("enroll --passcode-stdin")?)
        } else {
            passcode
        };
        let token = if token_stdin {
            Some(read_secret_line("enroll --token-stdin")?)
        } else {
            token
        };
        let display = name.clone().unwrap_or_else(|| {
            std::env::var("HOSTNAME").unwrap_or_else(|_| {
                {
                    let mut hostname = std::process::Command::new("hostname");
                    mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut hostname);
                    hostname.output()
                }
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map_or_else(|| "unknown".to_owned(), |s| s.trim().to_owned())
            })
        });
        match (passcode, token) {
            (Some(_), Some(_)) => {
                // `conflicts_with` should catch this at parse
                // time, but belt-and-braces.
                eprintln!(
                    "mackesd enroll: --passcode and --token are mutually \
                         exclusive; pass exactly one."
                );
                std::process::exit(2);
            }
            (None, None) => {
                eprintln!(
                    "mackesd enroll: pass either --passcode (v1.x flow) or \
                         --token (v2.5 Nebula flow)."
                );
                std::process::exit(2);
            }
            (Some(pc), None) => {
                // Phase 12.3.1 — v1.x build identity + signed request.
                let identity = mackesd_core::enrollment::build_identity();
                match mackesd_core::enrollment::build_request(&identity, &pc, &display) {
                    Some(req) => {
                        println!("{}", serde_json::to_string_pretty(&req)?);
                        eprintln!(
                            "enrollment request emitted — drop into the leader's \
                                 pending inbox (Phase 12.8.2)."
                        );
                    }
                    None => {
                        eprintln!(
                            "mackesd enroll: passcode failed validation (must be \
                                 16 URL-safe characters)."
                        );
                        std::process::exit(2);
                    }
                }
            }
            (None, Some(tok)) => {
                let _ = display;
                // Leftover-3: installed `enroll --token-stdin` must take the
                // same fingerprint-pinned join path. The CSR publish path is
                // retired and cannot authenticate cert or trust-pin delivery.
                if mackesd_core::nebula_enroll::token_uses_join_path(&tok) {
                    eprintln!("mackesd enroll: join-token carries a TLS pin — using join");
                    return crate::cli::join::run(Some(tok), None, name, workgroup_root);
                }
                eprintln!(
                    "mackesd enroll: replicated-file enrollment is retired; \
                     use a join token with a TLS fingerprint \
                     (mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>)"
                );
                std::process::exit(2);
            }
        }
    }
    Ok(())
}
