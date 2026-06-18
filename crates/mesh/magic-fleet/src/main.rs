//! `magic-fleet` — the MCNF Automation Mesh node CLI (E11.7).
//!
//! EFF-19 (2026-06-12): ported to clap (real `--help`, typed flags,
//! parity with `meshctl`) and grew `--json` on the converge-shaped
//! verbs (`apply` / `heal` / `converge` / `elect` / `reconcile`) so
//! scripts and the Workbench can consume structured outcomes instead
//! of scraping the human lines.
//!
//! Exit 0 only when the node converged / the heal did not fail.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use magic_fleet::{ApplyReport, AuditRecord, BaselineSpec, DriftStatus};

#[derive(Parser)]
#[command(
    name = "magic-fleet",
    version,
    about = "MCNF Automation Mesh node engine — apply/heal/converge desired-state baselines, elect replicated revisions, reconcile to the fleet log"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Apply a desired-state playbook locally.
    Apply {
        /// Path to the playbook YAML.
        playbook: PathBuf,
        /// Emit the outcome as one JSON object on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Re-apply a playbook + report drift (InSync/Healed/Failed).
    Heal {
        /// Path to the playbook YAML.
        playbook: PathBuf,
        /// Emit the outcome as one JSON object on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Render a desired-state baseline → apply + report drift.
    Converge {
        /// Path to the baseline YAML.
        baseline: PathBuf,
        /// Node-local exceptions file (Q124) filtered out of the
        /// baseline before it applies.
        #[arg(long, value_name = "PATH")]
        except: Option<PathBuf>,
        /// Emit the outcome as one JSON object on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Drift-watch daemon: converge on a timer, persist an audit log.
    Watch {
        /// Path to the baseline YAML.
        baseline: PathBuf,
        /// Tick cadence in seconds.
        #[arg(long, default_value_t = 900)]
        interval: u64,
        /// Audit-log path (default
        /// `$HOME/.local/state/magic-fleet/drift-audit.jsonl`).
        #[arg(long, value_name = "PATH")]
        audit: Option<String>,
        /// Node-local exceptions file (Q124).
        #[arg(long, value_name = "PATH")]
        except: Option<PathBuf>,
        /// Run a single tick and exit (how a timer unit drives it).
        #[arg(long)]
        once: bool,
    },
    /// Pick the newest-wins revision from the given files and
    /// converge to it.
    Elect {
        /// Revision YAML files this node holds.
        #[arg(required = true)]
        revisions: Vec<PathBuf>,
        /// Node-local exceptions file (Q124).
        #[arg(long, value_name = "PATH")]
        except: Option<PathBuf>,
        /// Emit the outcome as one JSON object on stdout.
        #[arg(long)]
        json: bool,
    },
    /// FPG-8: elect the head of the LizardFS revision log, converge to
    /// it host-local, write the apply-ack.
    Reconcile {
        /// Replicated workgroup root (defaults to $MDE_WORKGROUP_ROOT
        /// or $QNM_SHARED_ROOT).
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Override this node's hostname in the apply-ack.
        #[arg(long, value_name = "NAME")]
        hostname: Option<String>,
        /// Node-local exceptions file (Q124).
        #[arg(long, value_name = "PATH")]
        except: Option<PathBuf>,
        /// Emit the outcome as one JSON object on stdout.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().cmd {
        Cmd::Apply { playbook, json } => apply(&playbook, json),
        Cmd::Heal { playbook, json } => {
            let Some(yaml) = read(&playbook) else {
                return ExitCode::FAILURE;
            };
            let root = work_root("heal");
            drift_exit("heal", magic_fleet::heal_to_baseline(&yaml, &root), json)
        }
        Cmd::Converge {
            baseline,
            except,
            json,
        } => {
            let Some(yaml) = read(&baseline) else {
                return ExitCode::FAILURE;
            };
            let spec = match magic_fleet::BaselineSpec::from_yaml(&yaml) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("magic-fleet: invalid baseline: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let spec = match with_exceptions(spec, except.as_deref()) {
                Ok(s) => s,
                Err(code) => return code,
            };
            let root = work_root("converge");
            drift_exit("converge", magic_fleet::converge(&spec, &root), json)
        }
        Cmd::Watch {
            baseline,
            interval,
            audit,
            except,
            once,
        } => watch(&baseline, interval, audit, except.as_deref(), once),
        Cmd::Elect {
            revisions,
            except,
            json,
        } => elect(&revisions, except.as_deref(), json),
        Cmd::Reconcile {
            root,
            hostname,
            except,
            json,
        } => reconcile(root, hostname, except.as_deref(), json),
    }
}

fn apply(playbook: &Path, json: bool) -> ExitCode {
    let Some(yaml) = read(playbook) else {
        return ExitCode::FAILURE;
    };
    let root = work_root("apply");
    match magic_fleet::apply(&yaml, &root) {
        Ok(r) => {
            if json {
                println!("{}", outcome_json("apply", None, &r));
            } else {
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
            }
            exit_for(r.converged())
        }
        Err(e) => fail("apply", &e, json),
    }
}

/// Read a YAML input file or print the canonical error.
fn read(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("magic-fleet: cannot read {}: {e}", path.display());
            None
        }
    }
}

/// Per-invocation scratch root (ansible work dir).
fn work_root(verb: &str) -> PathBuf {
    std::env::temp_dir().join(format!("magic-fleet-{verb}-{}", std::process::id()))
}

/// Apply a `--except PATH` exceptions file (Q124) to `spec`. `None`
/// returns `spec` unchanged; an unreadable/invalid file errors out.
fn with_exceptions(spec: BaselineSpec, except: Option<&Path>) -> Result<BaselineSpec, ExitCode> {
    let Some(p) = except else { return Ok(spec) };
    let yaml = std::fs::read_to_string(p).map_err(|e| {
        eprintln!("magic-fleet: cannot read {}: {e}", p.display());
        ExitCode::FAILURE
    })?;
    let ex = magic_fleet::LocalExceptions::from_yaml(&yaml).map_err(|e| {
        eprintln!("magic-fleet: invalid exceptions {}: {e}", p.display());
        ExitCode::FAILURE
    })?;
    if !ex.is_empty() {
        eprintln!(
            "magic-fleet: applying {} local exception(s) from {}",
            ex.len(),
            p.display()
        );
    }
    Ok(spec.without_exceptions(&ex))
}

/// `elect` — given the revision files a node currently holds, pick the
/// winner (newest-wins, no fixed center) and converge the node to it.
fn elect(paths: &[PathBuf], except: Option<&Path>, json: bool) -> ExitCode {
    let mut revisions = Vec::with_capacity(paths.len());
    for p in paths {
        let Some(yaml) = read(p) else {
            return ExitCode::FAILURE;
        };
        match magic_fleet::Revision::from_yaml(&yaml) {
            Ok(r) => revisions.push(r),
            Err(e) => {
                eprintln!("magic-fleet: invalid revision {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        }
    }
    // elect_revision returns None only on an empty slice — clap's
    // `required = true` rules that out.
    let Some(winner) = magic_fleet::elect_revision(&revisions) else {
        eprintln!("magic-fleet: no revisions to elect");
        return ExitCode::FAILURE;
    };
    if !json {
        println!(
            "magic-fleet: elected revision v{} (author={}, at={}) from {} candidate(s)",
            winner.version,
            winner.author,
            winner.at,
            revisions.len()
        );
    }
    let spec = match with_exceptions(winner.spec.clone(), except) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let root = work_root("elect");
    drift_exit("elect", magic_fleet::converge(&spec, &root), json)
}

/// FPG-8 — the unified-baseline reconcile: elect the head of the
/// replicated revision log (FPG-2), converge to it host-local (Q10 —
/// no push-SSH; this node applies itself), then write this node's
/// apply-ack into `<root>/fleet/acks/<version>/` (FPG-5 / Q14) so the
/// author's FSM can advance to Verified. Revision authenticity rests
/// on the Nebula transport carrying the replicated volume; `author`
/// is advisory (Q17). The `settings` domain is skipped here — mackesd
/// applies settings natively (FPG-1/Q9).
fn reconcile(
    root: Option<PathBuf>,
    hostname: Option<String>,
    except: Option<&Path>,
    json: bool,
) -> ExitCode {
    let root = root
        .or_else(|| std::env::var("MDE_WORKGROUP_ROOT").ok().map(PathBuf::from))
        .or_else(|| std::env::var("QNM_SHARED_ROOT").ok().map(PathBuf::from));
    let Some(root) = root else {
        eprintln!(
            "magic-fleet: reconcile needs --root DIR or $MDE_WORKGROUP_ROOT (the replicated workgroup root)"
        );
        return ExitCode::FAILURE;
    };
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
        if json {
            println!(r#"{{"verb":"reconcile","status":"empty_log","converged":true}}"#);
        } else {
            println!("magic-fleet: revision log empty — nothing to reconcile");
        }
        return ExitCode::SUCCESS;
    };
    if !json {
        println!(
            "magic-fleet: reconciling to elected head v{} (author={})",
            head.version, head.author
        );
    }
    let spec = match with_exceptions(head.spec.clone(), except) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let work = work_root("reconcile");
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
    drift_exit("reconcile", outcome, json)
}

/// The drift-watch daemon: converge-on-a-timer, persisting one audit-
/// log line per tick. `--once` runs a single tick (how a scheduler /
/// timer unit drives it).
fn watch(
    baseline: &Path,
    interval_secs: u64,
    audit: Option<String>,
    except: Option<&Path>,
    once: bool,
) -> ExitCode {
    if interval_secs == 0 {
        eprintln!("magic-fleet: --interval must be a positive integer");
        return ExitCode::FAILURE;
    }
    let Some(yaml) = read(baseline) else {
        return ExitCode::FAILURE;
    };
    let spec = match magic_fleet::BaselineSpec::from_yaml(&yaml) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("magic-fleet: invalid baseline: {e}");
            return ExitCode::FAILURE;
        }
    };
    let spec = match with_exceptions(spec, except) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let audit_log = audit.unwrap_or_else(default_audit_path);
    let root = work_root("watch");
    println!(
        "magic-fleet: drift-watch on {}, audit -> {audit_log}{}",
        baseline.display(),
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

/// EFF-19 — one structured outcome object for `--json` consumers.
/// Stable keys; `drift` is `null` for plain `apply` (no drift verdict).
fn outcome_json(verb: &str, status: Option<DriftStatus>, r: &ApplyReport) -> String {
    serde_json::json!({
        "verb": verb,
        "drift": status.map(|s| format!("{s:?}")),
        "ok": r.ok,
        "changed": r.changed,
        "failures": r.failures,
        "unreachable": r.unreachable,
        "converged": r.converged(),
    })
    .to_string()
}

/// Print a drift outcome (human or `--json`) and map it to an exit
/// code (failure only on a failed heal/converge).
fn drift_exit(verb: &str, res: io::Result<(DriftStatus, ApplyReport)>, json: bool) -> ExitCode {
    match res {
        Ok((status, r)) => {
            if json {
                println!("{}", outcome_json(verb, Some(status), &r));
            } else {
                println!(
                    "magic-fleet: drift={status:?} ok={} changed={} failures={} unreachable={}",
                    r.ok, r.changed, r.failures, r.unreachable
                );
            }
            exit_for(status != DriftStatus::Failed)
        }
        Err(e) => fail(verb, &e, json),
    }
}

fn fail(verb: &str, e: &io::Error, json: bool) -> ExitCode {
    if json {
        println!(
            "{}",
            serde_json::json!({ "verb": verb, "error": e.to_string(), "converged": false })
        );
    } else {
        eprintln!("magic-fleet: {verb} failed: {e}");
    }
    ExitCode::FAILURE
}

const fn exit_for(ok: bool) -> ExitCode {
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
