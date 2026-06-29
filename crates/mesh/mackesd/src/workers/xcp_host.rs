//! XCP-6 (B2) — the `xcp_host` worker: on an XCP-ng **dom0**, advertise the
//! hypervisor's compute capacity into the mesh so any node can target it for a
//! VM spawn (the "full partner" / compute-provider behaviour from
//! `docs/design/xcp-ng-integration.md`).
//!
//! The worker self-gates on the dom0 marker (`/etc/xensource-inventory`): on a
//! non-dom0 node it idles immediately, so it's harmless to spawn on every Server.
//! On a dom0 it queries [`mackes_xcp`] locally (no SSH — the hypervisor *is* the
//! node) for CPU/RAM/SR-free/running-VM counts and publishes a capacity document
//! to `compute/xcp-host/<node-id>` every tick. The provisioning surfaces
//! (`action/provision/hosts` + the Workbench VM Spawner, XCP-3/4) read these to
//! list spawn targets.
//!
//! The document builder ([`xcp_host_doc`]) is pure + unit-tested; the worker is
//! the thin probe+publish shell (mirrors [`super::boot_readiness`]).

#![cfg(feature = "async-services")]

use std::path::Path;
use std::time::Duration;

use mackes_mesh_types::cap_tags::CapabilityTag;
use mackes_xcp::{HostCapacity, HostTarget, Hypervisor, XeSsh};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde_json::json;

use super::{ShutdownToken, Worker};

/// Bus topic prefix for per-host XCP capacity (one topic per dom0).
pub const TOPIC_PREFIX: &str = "compute/xcp-host/";

/// Publish cadence — capacity changes slowly; 15 s keeps the spawn-target list
/// fresh without hammering `xe`.
pub const INTERVAL: Duration = Duration::from_secs(15);

/// The canonical XCP-ng / XenServer dom0 marker. Present only on a hypervisor
/// control domain, so its existence is the cheap "am I a dom0?" gate.
const DOM0_MARKER: &str = "/etc/xensource-inventory";

/// Whether this node is an XCP-ng dom0 (so the worker should advertise capacity).
#[must_use]
pub fn is_xcp_dom0() -> bool {
    Path::new(DOM0_MARKER).exists()
}

/// Build the `compute/xcp-host/<node>` capacity document from a probe. Pure so
/// the published shape is testable without a live host. `now_ms` stamps it.
///
/// The doc self-asserts the `hypervisor` capability-tag token (DATACENTER-17):
/// a dom0 publishing capacity *is* a Hypervisor, so the spawn surfaces and the
/// Node-roles roster read the same first-class role off the live advert that
/// the tag model carries. The token is taken from the typed
/// [`CapabilityTag::Hypervisor`] so it can never drift from the writer's
/// vocabulary.
///
/// `address` is the dom0's reachable IPv4 (XCP-6): the provisioning-side consumer
/// ([`super::xcp_provision::select_published_capacity`]) matches this advert to an
/// `MCNF_XEN_DOM0S` allow-list entry by `address` (or `hostname`), so it can
/// prefer this published capacity over a fresh SSH probe of the dom0. Empty when
/// the dom0's primary IPv4 couldn't be determined (the consumer then matches by
/// hostname / falls back to a direct probe).
#[must_use]
pub fn xcp_host_doc(
    node_id: &str,
    hostname: &str,
    address: &str,
    cap: &HostCapacity,
    now_ms: u64,
) -> serde_json::Value {
    json!({
        "ok": true,
        "kind": "xcp-host",
        "role": CapabilityTag::Hypervisor.as_str(),
        "node_id": node_id,
        "hostname": hostname,
        "address": address,
        "ts_ms": now_ms,
        "capacity": {
            "cpu_count": cap.cpu_count,
            "mem_total_kib": cap.mem_total_kib,
            "mem_free_kib": cap.mem_free_kib,
            "sr_free_bytes": cap.sr_free_bytes,
            "running_vms": cap.running_vms,
        },
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn read_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Best-effort primary outbound IPv4 of this dom0 (XCP-6) — the address a
/// provisioner's `MCNF_XEN_DOM0S` allow-list entry uses, published into the
/// advert so the consumer can match it to the right dom0. Uses the connected-UDP
/// trick: connecting a datagram socket sends **no** packet, it just makes the
/// kernel pick (via the routing table) the egress source address, which it
/// exposes as the socket's local address. Empty on failure (no route / no IPv4),
/// in which case the consumer matches by hostname or falls back to a direct probe.
fn primary_ipv4() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.0.2.1:9")?; // TEST-NET-1 — no packet sent, route lookup only
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_default()
}

/// The `xcp_host` worker (XCP-6).
pub struct XcpHostWorker {
    node_id: String,
}

impl XcpHostWorker {
    /// New worker. `node_id` keys the per-host capacity topic.
    #[must_use]
    pub fn new(node_id: String) -> Self {
        Self { node_id }
    }
}

#[async_trait::async_trait]
impl Worker for XcpHostWorker {
    fn name(&self) -> &'static str {
        "xcp_host"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Self-gate: only a dom0 advertises XCP capacity. On every other node the
        // worker idles (returns) so spawning it fleet-wide is a no-op.
        if !is_xcp_dom0() {
            tracing::debug!("xcp_host: not an XCP-ng dom0 ({DOM0_MARKER} absent) — worker idle");
            return Ok(());
        }
        let Some(bus_root) = mde_bus::default_data_dir() else {
            tracing::debug!("xcp_host: no bus data dir; worker idle");
            return Ok(());
        };
        let topic = format!("{TOPIC_PREFIX}{}", self.node_id);
        let hv = XeSsh::new(HostTarget::Local);
        tracing::info!(topic = %topic, "xcp_host: dom0 detected — advertising capacity");
        loop {
            // The xe probe + bus publish are sync (xe shells out; Persist isn't
            // Send) — run on a blocking thread so the async runtime isn't stalled.
            let hv = hv.clone();
            let bus_root = bus_root.clone();
            let topic = topic.clone();
            let node_id = self.node_id.clone();
            let _ = tokio::task::spawn_blocking(move || match hv.host_capacity() {
                Ok(cap) => {
                    let doc =
                        xcp_host_doc(&node_id, &read_hostname(), &primary_ipv4(), &cap, now_ms());
                    if let Ok(persist) = Persist::open(bus_root) {
                        let _ =
                            persist.write(&topic, Priority::Default, None, Some(&doc.to_string()));
                    }
                }
                Err(e) => tracing::warn!(error = %e, "xcp_host: capacity probe failed"),
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

    #[test]
    fn doc_carries_capacity_and_identity() {
        let cap = HostCapacity {
            cpu_count: 8,
            mem_total_kib: 16 * 1024 * 1024,
            mem_free_kib: 9 * 1024 * 1024,
            sr_free_bytes: 500_000_000_000,
            running_vms: 3,
        };
        let v = xcp_host_doc("node-7", "xcp-1", "172.20.0.9", &cap, 42);
        assert_eq!(v["kind"], "xcp-host");
        // DATACENTER-17 — the dom0 self-asserts the hypervisor role, taken
        // from the typed cap-tag so it stays in lock-step with the vocabulary.
        assert_eq!(v["role"], "hypervisor");
        assert_eq!(v["role"], CapabilityTag::Hypervisor.as_str());
        assert_eq!(v["node_id"], "node-7");
        assert_eq!(v["hostname"], "xcp-1");
        // XCP-6 — the advert carries the dom0's reachable address so the
        // provisioner can match it to an MCNF_XEN_DOM0S allow-list entry.
        assert_eq!(v["address"], "172.20.0.9");
        assert_eq!(v["ts_ms"], 42);
        assert_eq!(v["capacity"]["cpu_count"], 8);
        assert_eq!(v["capacity"]["running_vms"], 3);
        assert_eq!(v["capacity"]["sr_free_bytes"], 500_000_000_000_u64);
    }
}
