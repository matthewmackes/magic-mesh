//! Workloads U9 — the `android-provision` verb handler (two-layer Cuttlefish).
//!
//! Android is delivered as a **two-layer** stack: an L1 Linux (Debian) VM sized for
//! nested virtualization (`cpu host-passthrough`), inside which the
//! `cuttlefish_host` Ansible role (a separate unit) runs `cvd start
//! --start_vnc_server` to boot the Android guest under crosvm. This handler owns
//! only the FIRST layer's declaration: it constructs an [`DeliveryType::AndroidVm`]
//! [`WorkloadSpec`] sized for Cuttlefish and routes it through the normal
//! set-desired / provision path (the L1 VM targets the `modules/android` tofu
//! module the infra unit creates — this handler writes no tofu). The Android screen
//! lives inside crosvm-inside-the-L1-VM (invisible to `virsh domdisplay`), so its
//! console is the in-guest VNC/WebRTC endpoint `cvd` serves, not a libvirt display.
//!
//! Fallback: on a host WITHOUT nested-KVM the same spec is realized as
//! Android-x86-in-KVM (a direct KVM guest, no Cuttlefish layer) — a `modules/android`
//! concern; the spec this handler mints is identical either way.
//!
//! Honest routing (§7): the spec is routed through the reconcile/set-desired seam
//! (shared with `set-desired`). Until that seam is wired (U4/U5), this verb honestly
//! reports the leg is not yet wired WHILE still doing — and echoing — the real work
//! of constructing the correct Cuttlefish workload spec. It never fakes a provision.

use std::path::Path;

use mackes_mesh_types::cloud::{CloudReply, DeliveryType, WorkloadSpec};

use super::super::reconcile;
use super::super::CloudWorker;
use super::CloudActionBody;

/// Cuttlefish L1-VM minimum virtual CPUs (nested-KVM Android needs headroom).
const CUTTLEFISH_MIN_VCPU: u16 = 4;
/// Cuttlefish L1-VM minimum memory (MiB) — 8 GiB.
const CUTTLEFISH_MIN_MEMORY_MB: u32 = 8192;
/// Cuttlefish L1-VM minimum root disk (GiB) — the Debian base + AOSP images.
const CUTTLEFISH_MIN_DISK_GB: u32 = 80;

/// Handle one `action/cloud/android-provision` request → a typed [`CloudReply`].
pub(super) fn handle(w: &CloudWorker, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    build_reply(&w.state_root, verb_name, body)
}

pub(super) fn authorization_target(body: &CloudActionBody) -> String {
    workload_name(body, body.node.trim())
}

/// Construct the Cuttlefish [`WorkloadSpec`] and route it through the normal
/// set-desired seam (`reconcile::write_desired_doc`), exactly as `set-desired`
/// does. Pure over the desired-doc `state_root` so it is tested without a live
/// backend.
fn build_reply(state_root: &Path, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    let node = body.node.trim();
    if node.is_empty() {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!(
                "`{verb_name}` requires a placement `node` for the Cuttlefish Android VM"
            )),
            ..Default::default()
        };
    }
    let name = workload_name(body, node);
    let spec = android_spec(node, &name);

    // Route the Cuttlefish L1-VM through the normal set-desired path: persist this
    // node's desired slice; the operator then applies it via the armed `provision`.
    // A persist failure is an honest error — the spec is still echoed, never a fake
    // success.
    match reconcile::write_desired_doc(state_root, &spec) {
        Ok(()) => CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            desired: Some(vec![spec]),
            ..Default::default()
        },
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!(
                "android-provision built the Cuttlefish Android VM `{name}` on `{node}` \
                 but could not persist its desired slice: {e}"
            )),
            desired: Some(vec![spec]),
            ..Default::default()
        },
    }
}

/// The workload name — the request's `name`, else a stable `android-<node>` default.
fn workload_name(body: &CloudActionBody, node: &str) -> String {
    body.name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| format!("android-{node}"), ToString::to_string)
}

/// Build the [`DeliveryType::AndroidVm`] L1-VM spec, sized for Cuttlefish. Pure +
/// directly tested — the load-bearing deliverable of this unit.
#[must_use]
pub(super) fn android_spec(node: &str, name: &str) -> WorkloadSpec {
    WorkloadSpec {
        name: name.to_string(),
        delivery_type: DeliveryType::AndroidVm,
        node: node.to_string(),
        vcpu: CUTTLEFISH_MIN_VCPU,
        memory_mb: CUTTLEFISH_MIN_MEMORY_MB,
        disk_gb: CUTTLEFISH_MIN_DISK_GB,
        // The `modules/android` golden Debian base (or Android-x86 on the fallback
        // path) — the delivery type's default, not an operator override here.
        image: None,
        network_isolation: false,
        raw_hcl: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn body(node: &str, name: Option<&str>) -> CloudActionBody {
        CloudActionBody {
            node: node.to_string(),
            name: name.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn android_spec_is_an_androidvm_sized_for_cuttlefish() {
        let spec = android_spec("eagle", "droid-1");
        assert_eq!(spec.delivery_type, DeliveryType::AndroidVm);
        assert_eq!(spec.name, "droid-1");
        assert_eq!(spec.node, "eagle");
        // Cuttlefish nested-KVM minimums (≥4 vcpu / ≥8 GiB / ≥80 GiB).
        assert!(spec.vcpu >= 4, "vcpu {}", spec.vcpu);
        assert!(spec.memory_mb >= 8192, "mem {}", spec.memory_mb);
        assert!(spec.disk_gb >= 80, "disk {}", spec.disk_gb);
        assert!(spec.image.is_none());
        assert!(!spec.network_isolation);
    }

    #[test]
    fn a_request_without_a_placement_node_is_honestly_rejected() {
        let tmp = tempdir().unwrap();
        let reply = build_reply(tmp.path(), "android-provision", &body("", None));
        assert!(!reply.ok);
        assert!(reply.desired.is_none());
        assert!(reply.error.unwrap().contains("placement `node`"));
    }

    #[test]
    fn android_provision_persists_and_echoes_the_cuttlefish_desired_slice() {
        let tmp = tempdir().unwrap();
        let reply = build_reply(
            tmp.path(),
            "android-provision",
            &body("eagle", Some("droid")),
        );
        assert!(reply.ok, "err: {:?}", reply.error);
        // The constructed AndroidVm spec is echoed …
        let desired = reply.desired.expect("echoed spec");
        assert_eq!(desired.len(), 1);
        assert_eq!(desired[0].name, "droid");
        assert_eq!(desired[0].delivery_type, DeliveryType::AndroidVm);
        // … and actually persisted to this node's desired slice.
        let slice = reconcile::read_desired_slice(tmp.path(), "eagle");
        assert_eq!(slice.len(), 1);
        assert_eq!(slice[0].name, "droid");
        assert_eq!(slice[0].delivery_type, DeliveryType::AndroidVm);
    }

    #[test]
    fn a_default_named_request_uses_the_stable_android_node_name() {
        let tmp = tempdir().unwrap();
        let reply = build_reply(tmp.path(), "android-provision", &body("eagle", None));
        assert!(reply.ok, "err: {:?}", reply.error);
        let desired = reply.desired.expect("echoed spec");
        assert_eq!(desired[0].name, "android-eagle", "default name");
    }
}
