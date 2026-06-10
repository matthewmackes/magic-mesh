//! `magic-fleet` — the Magic Mesh Automation Mesh node CLI (E11.7).
//!
//!   magic-fleet apply    <playbook.yml>   apply a desired-state playbook locally
//!   magic-fleet heal     <playbook.yml>   re-apply + report drift (InSync/Healed/Failed)
//!   magic-fleet converge <baseline.yml>   render a desired-state baseline → apply + report drift
//!   magic-fleet watch    <baseline.yml> [--interval=SECS] [--audit=PATH] [--once]
//!                                         drift-watch daemon: converge on a timer, persist an audit log
//!   magic-fleet elect    <revision.yml>...  pick the newest-wins revision and converge to it
//!   magic-fleet reconcile [--root=DIR] [--hostname=NAME] [--except=PATH]
//!                                         FPG-8: elect the head of the LizardFS revision log,
//!                                         converge to it host-local, write the apply-ack
//!
//! `converge`/`watch`/`elect` accept `--except=PATH` — a node's locally-declared
//! exceptions (Q124), filtered out of the baseline before it applies.
//!
//! Exit 0 only when the node converged / the heal did not fail.

use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use magic_fleet::{ApplyReport, AuditRecord, BaselineSpec, DriftStatus};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("watch") {
        return watch(&args[2..]);
    }
    if args.get(1).map(String::as_str) == Some("elect") {
        return elect(&args[2..]);
    }
    if args.get(1).map(String::as_str) == Some("reconcile") {
        return reconcile(&args[2..]);
    }
    let (Some(verb @ ("apply" | "heal" | "converge")), Some(path)) =
        (args.get(1).map(String::as_str), args.get(2))
    else {
        eprintln!(
            "usage: magic-fleet <apply|heal> <playbook.yml> | converge <baseline.yml>\n       magic-fleet watch <baseline.yml> [--interval=SECS] [--audit=PATH] [--once]\n       magic-fleet elect <revision.yml> [revision.yml ...]"
        );
        return ExitCode::FAILURE;
    };
    let yaml = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("magic-fleet: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let root = std::env::temp_dir().join(format!("magic-fleet-{}", std::process::id()));
    match verb {
        "apply" => match magic_fleet::apply(&yaml, &root) {
            Ok(r) => {
                println!(
                    "magic-fleet: ok={} changed={} failures={} unreachable={} -> {}",
                    r.ok,
                    r.changed,
                    r.failures,
                    r.unreachable,
                    if r.converged() {
                        "CONVERGED"
                    } else {
                        "NOT CONVERGED"
                    }
                );
                exit_for(r.converged())
            }
            Err(e) => fail("apply", &e),
        },
        "heal" => drift_exit(verb, magic_fleet::heal_to_baseline(&yaml, &root)),
        // "converge" — the only remaining verb the let-else admits.
        _ => match magic_fleet::BaselineSpec::from_yaml(&yaml) {
            Ok(spec) => match with_exceptions(spec, &args[3..]) {
                Ok(spec) => drift_exit(verb, magic_fleet::converge(&spec, &root)),
                Err(code) => code,
            },
            Err(e) => {
                eprintln!("magic-fleet: invalid baseline: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Apply a `--except=PATH` flag (when present in `flags`) to `spec`, returning the
/// baseline filtered through the node's locally-declared exceptions (Q124). No
/// `--except` flag returns `spec` unchanged; an unreadable/invalid file errors out
/// with a failure exit code.
fn with_exceptions(spec: BaselineSpec, flags: &[String]) -> Result<BaselineSpec, ExitCode> {
    for a in flags {
        let Some(p) = a.strip_prefix("--except=") else {
            continue;
        };
        let yaml = std::fs::read_to_string(p).map_err(|e| {
            eprintln!("magic-fleet: cannot read {p}: {e}");
            ExitCode::FAILURE
        })?;
        let ex = magic_fleet::LocalExceptions::from_yaml(&yaml).map_err(|e| {
            eprintln!("magic-fleet: invalid exceptions {p}: {e}");
            ExitCode::FAILURE
        })?;
        if !ex.is_empty() {
            eprintln!(
                "magic-fleet: applying {} local exception(s) from {p}",
                ex.len()
            );
        }
        return Ok(spec.without_exceptions(&ex));
    }
    Ok(spec)
}

/// `elect` — given the revision files a node currently holds, pick the winner
/// (newest-wins, no fixed center) and converge the node to its baseline. This is
/// the version-aware apply path: peers gossip revisions, the node elects one.
fn elect(args: &[String]) -> ExitCode {
    // Revision paths are the bare args; `--except=PATH` (the node's local
    // exceptions) is a flag applied to the elected winner before converging.
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if paths.is_empty() {
        eprintln!("usage: magic-fleet elect <revision.yml> [revision.yml ...] [--except=PATH]");
        return ExitCode::FAILURE;
    }
    let mut revisions = Vec::with_capacity(paths.len());
    for p in paths {
        let yaml = match std::fs::read_to_string(p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("magic-fleet: cannot read {p}: {e}");
                return ExitCode::FAILURE;
            }
        };
        match magic_fleet::Revision::from_yaml(&yaml) {
            Ok(r) => revisions.push(r),
            Err(e) => {
                eprintln!("magic-fleet: invalid revision {p}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // elect_revision returns None only on an empty slice, ruled out above.
    let Some(winner) = magic_fleet::elect_revision(&revisions) else {
        eprintln!("magic-fleet: no revisions to elect");
        return ExitCode::FAILURE;
    };
    println!(
        "magic-fleet: elected revision v{} (author={}, at={}) from {} candidate(s)",
        winner.version,
        winner.author,
        winner.at,
        revisions.len()
    );
    let spec = match with_exceptions(winner.spec.clone(), args) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let root = std::env::temp_dir().join(format!("magic-fleet-elect-{}", std::process::id()));
    drift_exit("elect", magic_fleet::converge(&spec, &root))
}

/// FPG-8 — the unified-baseline reconcile: elect the head of the
/// replicated revision log (FPG-2), converge to it host-local (Q10 —
/// no push-SSH; this node applies itself), then write this node's
/// apply-ack into `<root>/fleet/acks/<version>/` (FPG-5 / Q14) so the
/// author's FSM can advance to Verified. Revision authenticity rests
/// on the Nebula transport carrying the replicated volume; `author`
/// is advisory (Q17). The `settings` domain is skipped here — mackesd
/// applies settings natively (FPG-1/Q9).
fn reconcile(args: &[String]) -> ExitCode {
    let mut root: Option<String> = None;
    let mut hostname: Option<String> = None;
    for a in args {
        if let Some(v) = a.strip_prefix("--root=") {
            root = Some(v.to_string());
        } else if let Some(v) = a.strip_prefix("--hostname=") {
            hostname = Some(v.to_string());
        }
    }
    let root = root
        .or_else(|| std::env::var("MDE_WORKGROUP_ROOT").ok())
        .or_else(|| std::env::var("QNM_SHARED_ROOT").ok());
    let Some(root) = root else {
        eprintln!(
            "magic-fleet: reconcile needs --root=DIR or $MDE_WORKGROUP_ROOT (the replicated workgroup root)"
        );
        return ExitCode::FAILURE;
    };
    let root = std::path::PathBuf::from(root);
    let hostname = hostname.unwrap_or_else(|| {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    });
    let log_dir = magic_fleet::store::revisions_dir(&root);
    let Some(head) = magic_fleet::store::elect_head(&log_dir) else {
        println!("magic-fleet: revision log empty — nothing to reconcile");
        return ExitCode::SUCCESS;
    };
    println!(
        "magic-fleet: reconciling to elected head v{} (author={})",
        head.version, head.author
    );
    let spec = match with_exceptions(head.spec.clone(), args) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let work = std::env::temp_dir().join(format!("magic-fleet-reconcile-{}", std::process::id()));
    let outcome = magic_fleet::converge(&spec, &work);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let ack = match &outcome {
        Ok((status, _)) if *status != DriftStatus::Failed => magic_fleet::store::ApplyAck {
            peer: hostname,
            status: "applied".into(),
            at: now,
            detail: String::new(),
        },
        Ok((_, r)) => magic_fleet::store::ApplyAck {
            peer: hostname,
            status: "failed".into(),
            at: now,
            detail: format!("failures={} unreachable={}", r.failures, r.unreachable),
        },
        Err(e) => magic_fleet::store::ApplyAck {
            peer: hostname,
            status: "failed".into(),
            at: now,
            detail: e.to_string(),
        },
    };
    if let Err(e) = magic_fleet::store::write_ack(&root, head.version, &ack) {
        eprintln!("magic-fleet: ack write failed: {e}");
    }
    drift_exit("reconcile", outcome)
}

/// The drift-watch daemon: parse `watch` flags, then converge-on-a-timer,
/// persisting one audit-log line per tick. `--once` runs a single tick (the
/// daemon's single-shot mode, and how a scheduler/timer unit drives it).
fn watch(args: &[String]) -> ExitCode {
    let mut baseline: Option<&str> = None;
    let mut interval_secs: u64 = 900; // 15 min default cadence.
    let mut audit: Option<String> = None;
    let mut once = false;
    for a in args {
        if let Some(v) = a.strip_prefix("--interval=") {
            match v.parse::<u64>() {
                Ok(n) if n > 0 => interval_secs = n,
                _ => {
                    eprintln!("magic-fleet: --interval must be a positive integer (got {v:?})");
                    return ExitCode::FAILURE;
                }
            }
        } else if let Some(v) = a.strip_prefix("--audit=") {
            audit = Some(v.to_string());
        } else if a == "--once" {
            once = true;
        } else if a.starts_with("--except=") {
            // handled below via with_exceptions(spec, args)
        } else if a.starts_with("--") {
            eprintln!("magic-fleet: unknown watch flag {a:?}");
            return ExitCode::FAILURE;
        } else if baseline.is_none() {
            baseline = Some(a);
        } else {
            eprintln!("magic-fleet: unexpected extra argument {a:?}");
            return ExitCode::FAILURE;
        }
    }
    let Some(path) = baseline else {
        eprintln!(
            "usage: magic-fleet watch <baseline.yml> [--interval=SECS] [--audit=PATH] [--except=PATH] [--once]"
        );
        return ExitCode::FAILURE;
    };
    let yaml = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("magic-fleet: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let spec = match magic_fleet::BaselineSpec::from_yaml(&yaml) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("magic-fleet: invalid baseline: {e}");
            return ExitCode::FAILURE;
        }
    };
    let spec = match with_exceptions(spec, args) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let audit_log = audit.unwrap_or_else(default_audit_path);
    let root = std::env::temp_dir().join(format!("magic-fleet-watch-{}", std::process::id()));
    println!(
        "magic-fleet: drift-watch on {path}, audit -> {audit_log}{}",
        if once {
            " (single tick)".to_string()
        } else {
            format!(", every {interval_secs}s")
        }
    );
    loop {
        let last = match magic_fleet::drift_watch_tick(&spec, &root, Path::new(&audit_log)) {
            Ok(rec) => {
                report_tick(&rec);
                rec.status
            }
            Err(e) => {
                eprintln!("magic-fleet: drift-watch tick failed: {e}");
                if once {
                    return ExitCode::FAILURE;
                }
                // A transient tick failure (e.g. ansible-runner blip) must not kill
                // the daemon — log, wait the interval, and retry.
                std::thread::sleep(Duration::from_secs(interval_secs));
                continue;
            }
        };
        if once {
            return exit_for(last != DriftStatus::Failed);
        }
        std::thread::sleep(Duration::from_secs(interval_secs));
    }
}

/// Print one drift-watch tick's persisted record.
fn report_tick(rec: &AuditRecord) {
    println!(
        "magic-fleet: at={} drift={:?} ok={} changed={} failures={} unreachable={}",
        rec.at, rec.status, rec.ok, rec.changed, rec.failures, rec.unreachable
    );
}

/// The default audit-log path: `$HOME/.local/state/magic-fleet/drift-audit.jsonl`,
/// falling back to a temp-dir path when `HOME` is unset.
fn default_audit_path() -> String {
    std::env::var("HOME").map_or_else(
        |_| {
            std::env::temp_dir()
                .join("magic-fleet-drift-audit.jsonl")
                .display()
                .to_string()
        },
        |home| format!("{home}/.local/state/magic-fleet/drift-audit.jsonl"),
    )
}

/// Print a drift outcome and map it to an exit code (failure only on a failed heal).
fn drift_exit(verb: &str, res: io::Result<(DriftStatus, ApplyReport)>) -> ExitCode {
    match res {
        Ok((status, r)) => {
            println!(
                "magic-fleet: drift={status:?} ok={} changed={} failures={} unreachable={}",
                r.ok, r.changed, r.failures, r.unreachable
            );
            exit_for(status != DriftStatus::Failed)
        }
        Err(e) => fail(verb, &e),
    }
}

fn fail(verb: &str, e: &io::Error) -> ExitCode {
    eprintln!("magic-fleet: {verb} failed: {e}");
    ExitCode::FAILURE
}

const fn exit_for(ok: bool) -> ExitCode {
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
