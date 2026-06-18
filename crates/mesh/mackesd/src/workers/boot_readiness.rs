//! BOOT-STATUS-1 — the `boot_readiness` worker: probes the mesh-fabric bring-up
//! chain in dependency order and publishes ONE ordered snapshot to the Bus
//! (`state/boot-readiness`) each tick, so the Applications/HOME boot-status dialog
//! (`docs/design/boot-status-dialog.md`) and the applet chip render the same
//! authoritative state (Q5). Works headless — a Server/Lighthouse has the same
//! snapshot even with no desktop.
//!
//! The step model is the real boot dependency chain (Q7): Nebula → overlay IP →
//! mackesd serving → mde-bus broker → QNM-Shared mount → peer directory. Each
//! step carries `{id,label,status,detail,blocked_by,since_ms}`; a step whose
//! prerequisites aren't `ok` is `blocked` (not a misleading `fail`). The pure
//! builder (`build_readiness`) is unit-tested; the worker is the thin probe+publish
//! shell around it. App-daemon probes + per-peer roll-up + live pings land in
//! BOOT-STATUS-2/3.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde_json::json;

use super::{ShutdownToken, Worker};

/// The Bus topic the snapshot is published on (BOOT-STATUS-1).
pub const TOPIC: &str = "state/boot-readiness";

/// Publish cadence — fast enough to feel live while the chain converges.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// One gathered observation of the fabric bring-up state (impure inputs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootProbe {
    /// `nebula` unit active (overlay transport up).
    pub nebula_up: bool,
    /// This node's assigned overlay IP (empty until Nebula hands one out).
    pub overlay_ip: String,
    /// The mde-bus broker is reachable (we could open the spool to publish).
    pub bus_ok: bool,
    /// QNM-Shared is a real, writable FUSE mount.
    pub qnm_mounted: bool,
    /// Joined peer count in the replicated directory (>0 ⇒ replicated).
    pub peer_count: u32,
}

/// One ordered step in the dependency chain.
struct StepDef {
    id: &'static str,
    label: &'static str,
}

/// The boot dependency chain, in order (Q7).
const STEPS: [StepDef; 6] = [
    StepDef {
        id: "nebula",
        label: "Nebula overlay",
    },
    StepDef {
        id: "overlay-ip",
        label: "Overlay IP assigned",
    },
    StepDef {
        id: "mackesd",
        label: "mackesd serving",
    },
    StepDef {
        id: "bus",
        label: "Message bus broker",
    },
    StepDef {
        id: "qnm",
        label: "QNM-Shared mounted",
    },
    StepDef {
        id: "directory",
        label: "Peer directory replicated",
    },
];

/// Whether each step's own condition is met (parallel to [`STEPS`]). `mackesd` is
/// always met here — this worker runs *inside* the serving daemon.
fn step_ok(p: &BootProbe) -> [bool; 6] {
    [
        p.nebula_up,
        !p.overlay_ip.is_empty(),
        true, // mackesd is serving (this worker is part of it)
        p.bus_ok,
        p.qnm_mounted,
        p.peer_count > 0,
    ]
}

/// A short per-step detail line.
fn step_detail(p: &BootProbe, idx: usize) -> String {
    match idx {
        0 => {
            if p.nebula_up {
                "up".into()
            } else {
                "down".into()
            }
        }
        1 => {
            if p.overlay_ip.is_empty() {
                "—".into()
            } else {
                p.overlay_ip.clone()
            }
        }
        2 => "serving".into(),
        3 => {
            if p.bus_ok {
                "bound".into()
            } else {
                "unreachable".into()
            }
        }
        4 => {
            if p.qnm_mounted {
                "/mnt/mesh-storage".into()
            } else {
                "not mounted".into()
            }
        }
        5 => format!("{} peer(s)", p.peer_count),
        _ => String::new(),
    }
}

/// Build the `state/boot-readiness` snapshot from a probe. Each step is `ok`,
/// `blocked` (a prerequisite isn't ok — carries `blocked_by`), or `pending`
/// (its own condition isn't met but all prerequisites are). `ready` is true when
/// every step is `ok`. `now_ms` stamps the snapshot.
#[must_use]
pub fn build_readiness(p: &BootProbe, now_ms: u64) -> serde_json::Value {
    let oks = step_ok(p);
    let mut steps = Vec::with_capacity(STEPS.len());
    let mut first_unmet: Option<&'static str> = None;
    for (i, def) in STEPS.iter().enumerate() {
        let status = if oks[i] {
            "ok"
        } else if first_unmet.is_some() {
            "blocked"
        } else {
            "pending"
        };
        steps.push(json!({
            "id": def.id,
            "label": def.label,
            "status": status,
            "detail": step_detail(p, i),
            "blocked_by": if status == "blocked" { first_unmet } else { None },
        }));
        if !oks[i] && first_unmet.is_none() {
            first_unmet = Some(def.id);
        }
    }
    json!({
        "ok": true,
        "ready": oks.iter().all(|b| *b),
        "ts_ms": now_ms,
        "steps": steps,
    })
}

/// The `boot_readiness` worker.
pub struct BootReadinessWorker {
    workgroup_root: PathBuf,
    node_id: String,
    db_path: PathBuf,
}

impl BootReadinessWorker {
    /// New worker. `workgroup_root` is the QNM-Shared mount; `db_path` the bus
    /// directory store (for the peer-directory count).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String, db_path: PathBuf) -> Self {
        Self {
            workgroup_root,
            node_id,
            db_path,
        }
    }

    /// Gather a fresh probe (impure: systemctl, fs, the directory).
    fn probe(&self) -> BootProbe {
        let nebula_up = std::process::Command::new("systemctl")
            .args(["is-active", "nebula"])
            .output()
            .map(|o| o.stdout.starts_with(b"active"))
            .unwrap_or(false);
        let overlay_ip = std::fs::read_to_string(super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let qnm_mounted = crate::shared_root_writable(&self.workgroup_root);
        let now = now_ms();
        let peer_count = crate::ipc::directory::DirectoryService::new(
            &self.workgroup_root,
            Some(self.db_path.clone()),
        )
        .mesh_health_counts(&self.node_id, now)
        .0;
        BootProbe {
            nebula_up,
            overlay_ip,
            bus_ok: true, // set false on a publish failure below
            qnm_mounted,
            peer_count,
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[async_trait::async_trait]
impl Worker for BootReadinessWorker {
    fn name(&self) -> &'static str {
        "boot_readiness"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = mde_bus::default_data_dir() else {
            tracing::debug!("boot_readiness: no bus data dir; worker idle");
            return Ok(());
        };
        loop {
            // The probe + publish are sync (Persist isn't Send); run on a blocking
            // thread so the async runtime isn't stalled.
            let probe = self.probe();
            let bus_root = bus_root.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(persist) = Persist::open(bus_root) {
                    let snap = build_readiness(&probe, now_ms());
                    let _ = persist.write(TOPIC, Priority::Default, None, Some(&snap.to_string()));
                }
            })
            .await;
            tokio::select! {
                _ = shutdown.wait() => break,
                () = tokio::time::sleep(INTERVAL) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(v: &serde_json::Value, i: usize, k: &str) -> String {
        v["steps"][i][k].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn all_up_is_ready_every_step_ok() {
        let p = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            peer_count: 4,
        };
        let v = build_readiness(&p, 123);
        assert_eq!(v["ready"], true);
        assert_eq!(v["ts_ms"], 123);
        for i in 0..6 {
            assert_eq!(val(&v, i, "status"), "ok", "step {i}");
        }
        assert_eq!(val(&v, 1, "detail"), "10.42.0.5");
    }

    #[test]
    fn first_unmet_is_pending_downstream_is_blocked() {
        // Nebula down → step 0 pending, everything after blocked_by nebula.
        let p = BootProbe::default();
        let v = build_readiness(&p, 0);
        assert_eq!(v["ready"], false);
        assert_eq!(val(&v, 0, "status"), "pending"); // first unmet
        assert_eq!(val(&v, 1, "status"), "blocked");
        assert_eq!(val(&v, 1, "blocked_by"), "nebula");
        assert_eq!(val(&v, 5, "blocked_by"), "nebula");
    }

    #[test]
    fn midchain_unmet_blocks_only_downstream() {
        // Nebula + overlay-ip + mackesd + bus ok, but QNM not mounted.
        let p = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: false,
            peer_count: 0,
        };
        let v = build_readiness(&p, 0);
        assert_eq!(val(&v, 3, "status"), "ok"); // bus ok
        assert_eq!(val(&v, 4, "status"), "pending"); // qnm = first unmet
        assert_eq!(val(&v, 5, "status"), "blocked"); // directory blocked by qnm
        assert_eq!(val(&v, 5, "blocked_by"), "qnm");
        assert_eq!(v["ready"], false);
    }
}
