//! Typed Browser VM presentation adapter.
//!
//! Sunshine remains unavailable until a seat-side Moonlight decoder exists.
//! This module implements the explicitly selected RDP alternate without gaining
//! VM lifecycle authority: it consumes an expiring, generation-bound Workloads
//! attachment lease and hands the existing IronRDP transport an authenticated
//! mesh endpoint.

use mackes_mesh_types::workloads::{
    WorkloadAttachmentProtocol, WorkloadBackend, WorkloadOperationPhase, WorkloadOperationStatus,
    WorkloadPowerState, WorkloadReadiness,
};

use super::{
    BrokerSessionLifecycle, ConnectRequest, DesktopEndpoint, DisplayMode, MonitorSpan,
    RequestedTarget, VdiProtocol,
};
use crate::auth::DesktopAuth;

const BROWSER_WORKLOAD: &str = "browser-vm";
const BROWSER_IMAGE: &str = "browser-vm-chromium";
const BROWSER_VCPU: u16 = 3;
const BROWSER_MEMORY_MB: u32 = 8_192;
const BROWSER_DISK_GB: u32 = 64;
const RDP_PORT: u16 = 3389;

/// Exact authority retained while an RDP Browser presentation is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BrowserTransportAuthority {
    node: String,
    client_node: String,
    request_id: String,
    generation: u64,
    lease_id: String,
    expires_at_ms: u64,
}

impl BrowserTransportAuthority {
    /// Admit only a ready, unexpired RDP lease from the exact Browser workload
    /// projection. The lease nonce is deliberately not converted into an RDP
    /// password; mesh identity remains the transport authentication boundary.
    pub(super) fn admit(
        target: &crate::web::BrowserVmTarget,
        local_node: &str,
        status: &WorkloadOperationStatus,
        now_ms: u64,
    ) -> Result<Self, &'static str> {
        if target.workload != BROWSER_WORKLOAD
            || !target.reachable
            || !matches!(
                target.status.trim().to_ascii_lowercase().as_str(),
                "active" | "running"
            )
            || status.workload_id.as_str() != BROWSER_WORKLOAD
            || status.backend != WorkloadBackend::LibvirtVirtqemud
            || status.phase != WorkloadOperationPhase::Completed
            || status.power != WorkloadPowerState::Running
            || status.readiness != WorkloadReadiness::Ready
        {
            return Err("Browser workload is not ready for an RDP attachment");
        }
        if status.image_ref.as_deref() != Some(BROWSER_IMAGE)
            || status.resources.vcpu != BROWSER_VCPU
            || status.resources.memory_mb != BROWSER_MEMORY_MB
            || status.resources.disk_gb != BROWSER_DISK_GB
        {
            return Err("Browser workload does not match the governed guest profile");
        }
        let lease = status
            .attachment
            .as_ref()
            .ok_or("Browser workload has no attachment lease")?;
        lease
            .validate(now_ms)
            .map_err(|_| "Browser attachment lease is expired or malformed")?;
        if lease.protocol != WorkloadAttachmentProtocol::Rdp
            || lease.workload_id != status.workload_id
            || lease.generation != status.generation
            || !safe_mesh_node(&target.serving_peer)
            || !safe_mesh_node(local_node)
        {
            return Err("Browser attachment identity does not match Workloads authority");
        }
        Ok(Self {
            node: target.serving_peer.clone(),
            client_node: local_node.to_owned(),
            request_id: status.request_id.clone(),
            generation: status.generation,
            lease_id: lease.lease_id.clone(),
            expires_at_ms: lease.expires_at_ms,
        })
    }

    pub(super) fn connect_request(&self) -> ConnectRequest {
        let endpoint = DesktopEndpoint::new(self.node.clone(), RDP_PORT)
            .expect("an admitted mesh node always forms a bounded RDP endpoint");
        ConnectRequest::new(
            RequestedTarget::new(self.node.clone(), BROWSER_WORKLOAD).with_endpoint(Some(endpoint)),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity(self.client_node.clone()),
        )
        .with_broker_session(BrokerSessionLifecycle::new(
            format!(
                "browser-rdp:{}:{}:{}",
                self.request_id, self.generation, self.lease_id
            ),
            None,
        ))
    }

    pub(super) fn still_authorized(&self, status: &WorkloadOperationStatus, now_ms: u64) -> bool {
        let target = crate::web::BrowserVmTarget {
            serving_peer: self.node.clone(),
            workload: BROWSER_WORKLOAD.to_owned(),
            status: "running".to_owned(),
            reachable: true,
        };
        Self::admit(&target, &self.client_node, status, now_ms)
            .is_ok_and(|current| current == *self)
    }

    pub(super) fn node(&self) -> &str {
        &self.node
    }
}

fn safe_mesh_node(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
