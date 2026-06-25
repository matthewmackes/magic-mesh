//! Boot-readiness reader — the live remnant of the old Workbench landing page.
//!
//! FRONTDOOR-1 replaced the Workbench's old launcher/dashboard: `app.rs`'s
//! `panel_body` now routes the Dashboard group root and the `"home"` panel to
//! the [`crate::panels::front_door`] surface, NOT this module. FRONTDOOR-16
//! removed the dead launcher rendering (the 3000-line capability-list / hero
//! dashboard widget tree that nothing called any more).
//!
//! What remains is the self-contained **boot-readiness** reader (BOOT-STATUS-*):
//! the decoder for the `state/boot-readiness` snapshot that several live callers
//! still depend on —
//!
//!   * `main.rs` — [`boot_popup_should_suppress`] + [`read_boot_readiness`]
//!     decide whether the `--boot-popup` autostart opens a window at login;
//!   * the Front Door's System tile reads [`read_boot_readiness`] for its live
//!     value (the GUI-side boot glance, via `front_door::project::system`);
//!   * `peers.rs` reads [`BootReadiness::fabric_converging`] to distinguish a
//!     settling mesh from a genuinely-empty one.
//!
//! Pure decode + a single Bus read; no widget tree, no subscription, no state.

/// One bring-up step, decoded from the `state/boot-readiness` snapshot
/// (BOOT-STATUS-1). `status` is `ok` / `pending` / `blocked`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootStep {
    /// Stable step id (`nebula` / `overlay-ip` / `mackesd` / `bus` / `qnm` /
    /// `directory`) — robust to render across panels (BOOT-PEERS-1 keys on it).
    pub id: String,
    /// Display label (e.g. "QNM-Shared mounted").
    pub label: String,
    /// `ok` | `pending` | `blocked`.
    pub status: String,
    /// Short per-step detail (overlay IP, peer count, …).
    pub detail: String,
}

/// BOOT-STATUS-2 — one app-daemon row decoded from the snapshot `services`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootService {
    /// Stable id (`musicd` / `netdata` / `kdc`) — BOOT-STATUS-6 maps it to the
    /// systemd unit + scope for the inline Restart action.
    pub id: String,
    /// Display label (e.g. "Music daemon").
    pub label: String,
    /// `ok` | `down`.
    pub status: String,
}

/// BOOT-STATUS-2/3 — one per-peer ping row decoded from the snapshot `pings`
/// (the roll-up rows: who's reachable + RTT).
#[derive(Debug, Clone, PartialEq)]
pub struct BootPing {
    /// Peer hostname.
    pub peer: String,
    /// Peer role (`lighthouse` / `peer`).
    pub role: String,
    /// Round-trip ms, or `None` when unreachable.
    pub rtt_ms: Option<f64>,
}

/// The decoded `state/boot-readiness` snapshot (BOOT-STATUS-2/4). A
/// missing/garbage body → an all-empty value (the section shows "unknown").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BootReadiness {
    /// Whether every fabric chain step is `ok`.
    pub ready: bool,
    /// The bring-up dependency chain.
    pub steps: Vec<BootStep>,
    /// BOOT-STATUS-2 — supplementary app daemons.
    pub services: Vec<BootService>,
    /// BOOT-STATUS-2/3 — per-peer ping roll-up.
    pub pings: Vec<BootPing>,
}

/// BOOT-STATUS-5 — should the boot-status auto-popup suppress itself? True only
/// when launched as the autostart popup (`--boot-popup`) AND the mesh is already
/// all-green: then the persistent applet chip / HOME glance suffices and we don't
/// pop the window. During the cold-boot warm-up (`ready == false`) the popup opens
/// so boot status is front-and-centre at login.
#[must_use]
pub fn boot_popup_should_suppress(boot_popup: bool, ready: bool) -> bool {
    boot_popup && ready
}

impl BootReadiness {
    /// BOOT-PEERS-1 — is the mesh fabric still coming up? True when a snapshot
    /// exists and any *fabric* step (everything but the final peer `directory`
    /// step) isn't `ok` yet — i.e. Nebula / overlay-IP / bus / QNM haven't all
    /// converged. An empty roster during this window is "settling", not "empty
    /// mesh". A lone healthy node (fabric up, just no peers) returns `false`, so
    /// the genuine empty state still shows.
    #[must_use]
    pub fn fabric_converging(&self) -> bool {
        !self.steps.is_empty()
            && self
                .steps
                .iter()
                .any(|s| s.id != "directory" && s.status != "ok")
    }
}

/// Parse the `state/boot-readiness` snapshot body. A missing/garbage body →
/// [`BootReadiness::default`] (the section then shows "unknown").
#[must_use]
pub fn parse_boot_readiness(reply: &str) -> BootReadiness {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(reply) else {
        return BootReadiness::default();
    };
    let ready = v
        .get("ready")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let steps = v
        .get("steps")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(BootStep {
                        id: s
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        label: s.get("label")?.as_str()?.to_string(),
                        status: s
                            .get("status")
                            .and_then(|x| x.as_str())
                            .unwrap_or("pending")
                            .to_string(),
                        detail: s
                            .get("detail")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let services = v
        .get("services")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(BootService {
                        id: s
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        label: s.get("label")?.as_str()?.to_string(),
                        status: s
                            .get("status")
                            .and_then(|x| x.as_str())
                            .unwrap_or("down")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let pings = v
        .get("pings")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(BootPing {
                        peer: s.get("peer")?.as_str()?.to_string(),
                        role: s
                            .get("role")
                            .and_then(|x| x.as_str())
                            .unwrap_or("peer")
                            .to_string(),
                        rtt_ms: s.get("rtt_ms").and_then(serde_json::Value::as_f64),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    BootReadiness {
        ready,
        steps,
        services,
        pings,
    }
}

/// Read the latest `state/boot-readiness` snapshot off the bus.
/// [`BootReadiness::default`] when the bus/topic isn't available yet (mid-boot).
#[must_use]
pub fn read_boot_readiness() -> BootReadiness {
    let Some(dir) = mde_bus::default_data_dir() else {
        return BootReadiness::default();
    };
    let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
        return BootReadiness::default();
    };
    let topic = "state/boot-readiness";
    let Ok(Some(latest)) = persist.latest_ulid(topic) else {
        return BootReadiness::default();
    };
    persist
        .list_since(topic, None)
        .ok()
        .and_then(|msgs| msgs.into_iter().rev().find(|m| m.ulid == latest))
        .and_then(|m| m.body)
        .map(|b| parse_boot_readiness(&b))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_boot_readiness_reads_ready_and_steps() {
        let snap = r#"{"ok":true,"ready":false,"ts_ms":9,"steps":[
            {"id":"nebula","label":"Nebula overlay","status":"ok","detail":"up"},
            {"id":"qnm","label":"QNM-Shared mounted","status":"pending","detail":"not mounted"},
            {"id":"directory","label":"Peer directory replicated","status":"blocked","detail":"0 peer(s)","blocked_by":"qnm"}
        ],
        "services":[
            {"id":"musicd","label":"Music daemon","active":true,"reachable":null,"status":"ok"},
            {"id":"netdata","label":"Live metrics","active":false,"reachable":false,"status":"down"}
        ],
        "pings":[
            {"peer":"lh-01","overlay_ip":"10.42.0.1","role":"lighthouse","rtt_ms":3.2,"reachable":true},
            {"peer":"anvil","overlay_ip":"","role":"peer","rtt_ms":null,"reachable":false}
        ]}"#;
        let r = parse_boot_readiness(snap);
        assert!(!r.ready);
        assert_eq!(r.steps.len(), 3);
        assert_eq!(r.steps[0].label, "Nebula overlay");
        assert_eq!(r.steps[0].status, "ok");
        assert_eq!(r.steps[1].status, "pending");
        assert_eq!(r.steps[2].detail, "0 peer(s)");
        // BOOT-STATUS-2 — services + pings decode too.
        assert_eq!(r.services.len(), 2);
        assert_eq!(r.services[0].id, "musicd");
        assert_eq!(r.services[0].status, "ok");
        assert_eq!(r.services[1].status, "down");
        assert_eq!(r.pings.len(), 2);
        assert_eq!(r.pings[0].role, "lighthouse");
        assert_eq!(r.pings[0].rtt_ms, Some(3.2));
        assert_eq!(r.pings[1].rtt_ms, None);
        // BOOT-PEERS-1 — fabric still converging (a non-directory step pending).
        assert!(r.fabric_converging());
        // ready snapshot + garbage.
        assert!(parse_boot_readiness(r#"{"ready":true,"steps":[]}"#).ready);
        assert_eq!(parse_boot_readiness("nope"), BootReadiness::default());
    }

    #[test]
    fn boot_popup_suppresses_only_when_ready() {
        // BOOT-STATUS-5 — suppress the auto-popup iff it's the boot-popup launch
        // AND the mesh is already all-green.
        assert!(boot_popup_should_suppress(true, true)); // ready → no window
        assert!(!boot_popup_should_suppress(true, false)); // converging → open
        assert!(!boot_popup_should_suppress(false, true)); // normal launch → open
        assert!(!boot_popup_should_suppress(false, false));
    }

    #[test]
    fn fabric_converging_distinguishes_settling_from_lone_node() {
        // BOOT-PEERS-1 — fabric up but no peers (lone healthy node) is NOT
        // converging (the genuine empty state should show).
        let lone = r#"{"ready":false,"steps":[
            {"id":"nebula","label":"Nebula overlay","status":"ok"},
            {"id":"overlay-ip","label":"Overlay IP assigned","status":"ok"},
            {"id":"mackesd","label":"mackesd serving","status":"ok"},
            {"id":"bus","label":"Message bus broker","status":"ok"},
            {"id":"qnm","label":"QNM-Shared mounted","status":"ok"},
            {"id":"directory","label":"Peer directory replicated","status":"pending"}
        ]}"#;
        assert!(!parse_boot_readiness(lone).fabric_converging());
        // No snapshot at all → not converging (we have no evidence of mid-boot).
        assert!(!BootReadiness::default().fabric_converging());
    }
}
