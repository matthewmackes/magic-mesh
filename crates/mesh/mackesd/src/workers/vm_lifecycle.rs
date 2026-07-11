//! MV-3 — `vm_lifecycle`: the libvirt/KVM VM-lifecycle worker.
//!
//! The Fedora + KVM successor to the xcp-ng toolstack's lifecycle half
//! (xapi + xenopsd + sm + xcp-networkd): the backend the Datacenter UI drives to
//! **create / start / stop / destroy / list** VMs on a node's libvirt. Where
//! MV-1 ([`crate::kvm`]) is the service *catalog* and MV-2
//! ([`super::kvm_health`]) folds host *health*, MV-3 is the *actuator* — it turns
//! an operator's `action/vm/lifecycle` request into `virsh`/`qemu-img` calls and
//! publishes the resulting instance roster to `event/vm/instances`.
//!
//! It runs on **every** mesh node (like `kvm_health`): the KVM stack is universal,
//! so any node can host datacenter VMs. Because the action topic is flat (shared
//! by the whole mesh), each [`LifecycleAction`] carries a `host` — the worker acts
//! only on requests addressed to *its* node id ([`LifecycleAction::targets`]) and
//! advances past the rest, so one create request doesn't fan out to every node.
//!
//! ## Shape (mirrors `mackes-xcp::Hypervisor` + `kvm_health`)
//!
//! - An **injectable [`LibvirtBackend`] trait** (`create`/`start`/`stop`/
//!   `destroy`/`list`/`info` + the default-storage-pool + default-network
//!   ensure helpers) is the sole seam to the outside. Production wires
//!   [`VirshCli`] (shells `virsh`/`qemu-img` through the bounded-proc path,
//!   [`crate::workers::proc`]); tests wire a `FakeLibvirt`.
//! - The **pure core** is fully unit-tested with no live virsh: the
//!   [`VmSpec`] → `virsh define` argv (via [`build_domain_xml`] + the argv
//!   builders), the [`parse_virsh_list`] / [`parse_virsh_dominfo`] parsers, and
//!   the [`plan_transition`] lifecycle state machine. [`apply_action`] composes
//!   them over an injected backend, so the request→action wiring is testable
//!   against `FakeLibvirt` without KVM.
//! - Publishing mirrors `kvm_health` exactly: the `event/vm/instances` body is
//!   written in-process through [`crate::bus_publish`] (perf-10 / arch-6 — no
//!   fork+exec of the `mde-bus` CLI per roster), and every virsh shell-out is
//!   bounded by the EFF-20 timeout so a wedged libvirt can't pin a runtime
//!   thread.
//!
//! ## First slice vs deferred
//!
//! Implemented (this slice): **create-from-image** (a per-VM qcow2 overlay backed
//! by a golden image, or a blank disk of `disk_gb`), **start**, **stop**
//! (graceful `virsh shutdown` or `force` `virsh destroy`), **pause/resume**
//! (`virsh suspend`/`virsh resume` — the CHOOSER-7 card power controls drive
//! these to suspend/wake a running console), **destroy** (force-off then
//! `virsh undefine`, optional `--remove-all-storage`), **list**, **info**, the
//! default dir storage-pool + default NAT-network ensure helpers, **local
//! PipeWire audio** (E12-9: every domain gets `<sound model='virtio'/>` +
//! `<audio type='pipewire'>`, see [`build_domain_xml`]), **static USB
//! passthrough** (E12-10: [`LifecycleAction::AttachUsb`] /
//! [`DetachUsb`](LifecycleAction::DetachUsb), `virsh
//! attach-device`/`detach-device --live` against a standalone `<hostdev
//! type='usb'>` fragment), and **VFIO/PCI passthrough XML construction**
//! (E12-10: [`VmSpec::pci_passthrough`] / [`build_pci_hostdev_xml`] — pure XML
//! only; see that function's doc comment for why the live IOMMU/hardware
//! runtime path isn't validated here).
//!
//! Deferred (intentionally NOT stubbed with `todo!()` — each rides an existing or
//! future worker, or is explicitly hardware/live-gated per
//! `docs/design/e12-9-10-libvirt-rescope.md`): live/cold **migration** (VIRT-8
//! `compute_migrate`), **snapshots** (DATACENTER-12 `dc_snap_scheduler`), vCPU/RAM
//! hotplug, cloud-init **identity seeding** (VIRT-6 `compute_provision`, the
//! desktop/mesh-peer create path), console/VNC brokering (E12 egui VDI), UEFI
//! nvram cleanup on undefine, richer per-VM telemetry (VIRT-1
//! `compute_registry` already publishes cpu/ram/disk to `compute/inventory`),
//! **dynamic SPICE `usbredir`** (the click-to-redirect UX — needs upstream
//! `spice-client` protocol work the pinned crate doesn't implement),
//! the **VFIO live/IOMMU-hardware runtime demo** (this project's farm
//! build-VM slots run nested atop Xen dom0s, and no inventoried physical seat
//! has a confirmed second GPU / IOMMU state — mirrors the pre-existing
//! DATACENTER-22 finding on the old Xen stack), and **remote audio** (RDP
//! RDPSND is WON'T-DO per `docs/NEEDS-OPERATOR.md`; a SPICE playback channel
//! is a separate future slice, same `spice-client` wall as usbredir).

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_bus::persist::Persist;
use thiserror::Error;

use crate::workers::proc::{output_with_timeout, status_with_timeout, DEFAULT_CMD_TIMEOUT};

use super::{ShutdownToken, Worker};

/// Bus topic the worker drains for lifecycle requests (flat; per-node targeting
/// is via the request's `host` field, not the topic).
pub const ACTION_TOPIC: &str = "action/vm/lifecycle";

/// Bus topic the worker publishes this node's VM instance roster to.
pub const INSTANCES_TOPIC: &str = "event/vm/instances";

/// Canonical dir storage pool VM disks live in (the sm/SR equivalent). Matches
/// `compute_provision`'s `mde-vms` pool so the two create paths share storage.
pub const DEFAULT_POOL_NAME: &str = "mde-vms";

/// Filesystem directory backing [`DEFAULT_POOL_NAME`].
pub const DEFAULT_POOL_DIR: &str = "/var/lib/mde-vms";

/// The default libvirt NAT network (virbr0) guests attach to.
pub const DEFAULT_NETWORK: &str = "default";

/// Action-drain cadence. The bus read is a cheap local log scan; VM lifecycle is
/// a slow, operator-visible event, so a 2 s poll is plenty responsive without
/// spinning virsh.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Slow heartbeat for the `event/vm/instances` publish. Between heartbeats the
/// roster is published only right after a handled action (state changed); once
/// this elapses it republishes unconditionally so a freshly-pruned topic / late
/// subscriber still finds a recent roster. Keeps the `virsh list` snapshot off
/// the hot path (queried on-action or every 30 s, never every 2 s tick).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

// ───────────────────────────── data model ─────────────────────────────

/// A libvirt VM spec — the operator-facing description [`LibvirtBackend::create`]
/// turns into a `virsh define` (via [`build_domain_xml`]) plus its backing disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VmSpec {
    /// libvirt domain name (also the disk basename + the lifecycle key).
    pub name: String,
    /// Virtual CPUs.
    pub vcpus: u32,
    /// Guest RAM in MiB.
    pub ram_mb: u64,
    /// Backing disk size in GiB (the overlay's virtual size, or a blank disk's
    /// size when no `image_path` is given).
    pub disk_gb: u64,
    /// **Create-from-image** base: a golden qcow2 the per-VM disk is a
    /// copy-on-write overlay of. `None` ⇒ a blank qcow2 of `disk_gb`.
    #[serde(default)]
    pub image_path: Option<String>,
    /// libvirt network the guest NIC attaches to. `None` ⇒ [`DEFAULT_NETWORK`].
    #[serde(default)]
    pub network: Option<String>,
    /// VFIO/GPU PCI passthrough devices (E12-10) — empty by default, so an
    /// unconfigured VM's domain XML is unchanged. **Pure domain-XML
    /// construction only** — see [`build_pci_hostdev_xml`]'s doc comment for
    /// why the live IOMMU/hardware runtime path isn't validated by this
    /// project's environment today
    /// (`docs/design/e12-9-10-libvirt-rescope.md`).
    #[serde(default)]
    pub pci_passthrough: Vec<PciAddress>,
}

impl VmSpec {
    /// The network this spec attaches to (its override, or [`DEFAULT_NETWORK`]).
    #[must_use]
    pub fn network_or_default(&self) -> &str {
        self.network.as_deref().unwrap_or(DEFAULT_NETWORK)
    }
}

/// A PCI host device address for [`VmSpec::pci_passthrough`] (E12-10 VFIO).
/// Each field is the raw, `0x`-prefixed hex string libvirt's `<address>`
/// element expects — e.g. `lspci -D`'s `0000:01:00.0` splits into
/// `domain="0x0000"`, `bus="0x01"`, `slot="0x00"`, `function="0x0"`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PciAddress {
    /// PCI domain, e.g. `"0x0000"`.
    pub domain: String,
    /// PCI bus, e.g. `"0x01"`.
    pub bus: String,
    /// PCI slot (device), e.g. `"0x00"`.
    pub slot: String,
    /// PCI function, e.g. `"0x0"`.
    pub function: String,
}

/// One lifecycle command drained off [`ACTION_TOPIC`]. Internally tagged by `op`
/// so the JSON a Datacenter UI publishes is self-describing, e.g.
/// `{"op":"create","host":"node-a","spec":{…}}` /
/// `{"op":"stop","host":"node-a","name":"web1","force":true}`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LifecycleAction {
    /// Define a new VM from `spec` (creates the disk + `virsh define`; leaves it
    /// shut off — a subsequent `start` boots it).
    Create {
        /// Target node id (must match this node to act).
        host: String,
        /// The VM to define.
        spec: VmSpec,
    },
    /// Start a defined VM.
    Start {
        /// Target node id.
        host: String,
        /// Domain name.
        name: String,
    },
    /// Stop a running VM — graceful `virsh shutdown`, or a hard `virsh destroy`
    /// when `force`.
    Stop {
        /// Target node id.
        host: String,
        /// Domain name.
        name: String,
        /// Pull the plug instead of a graceful ACPI shutdown.
        #[serde(default)]
        force: bool,
    },
    /// Pause a running VM — `virsh suspend` (the CPU is frozen, RAM retained). The
    /// console stays attachable; [`Resume`](Self::Resume) wakes it.
    Pause {
        /// Target node id.
        host: String,
        /// Domain name.
        name: String,
    },
    /// Resume a paused VM — `virsh resume` (un-freeze a suspended domain).
    Resume {
        /// Target node id.
        host: String,
        /// Domain name.
        name: String,
    },
    /// Destroy a VM: force-off (if running) then `virsh undefine`, optionally
    /// wiping its storage.
    Destroy {
        /// Target node id.
        host: String,
        /// Domain name.
        name: String,
        /// Also `--remove-all-storage` (delete the backing disk).
        #[serde(default)]
        remove_storage: bool,
    },
    /// Hot-attach a USB host device into a running VM (E12-10 static
    /// passthrough) — `virsh attach-device --live` against a standalone
    /// `<hostdev type='usb'>` fragment addressed by vendor/product id.
    /// Requires the VM to be running (mirrors `Pause`'s own precondition
    /// shape — there's no live QEMU process to hotplug into otherwise).
    AttachUsb {
        /// Target node id.
        host: String,
        /// Domain name.
        name: String,
        /// USB vendor id, e.g. `"0x0781"`.
        vendor: String,
        /// USB product id, e.g. `"0x5567"`.
        product: String,
    },
    /// Hot-detach a previously attached USB host device — `virsh
    /// detach-device --live` against the same `<hostdev>` shape
    /// [`AttachUsb`](Self::AttachUsb) used (libvirt matches the device to
    /// remove by its description, so vendor/product must match the original
    /// attach).
    DetachUsb {
        /// Target node id.
        host: String,
        /// Domain name.
        name: String,
        /// USB vendor id, e.g. `"0x0781"`.
        vendor: String,
        /// USB product id, e.g. `"0x5567"`.
        product: String,
    },
    /// No-op lifecycle change — just asks the target to re-publish its roster
    /// (the operator's "refresh" button). The `list` verb of the first slice.
    Refresh {
        /// Target node id.
        host: String,
    },
}

impl LifecycleAction {
    /// The node id this action is addressed to.
    #[must_use]
    pub fn host(&self) -> &str {
        match self {
            Self::Create { host, .. }
            | Self::Start { host, .. }
            | Self::Stop { host, .. }
            | Self::Pause { host, .. }
            | Self::Resume { host, .. }
            | Self::Destroy { host, .. }
            | Self::AttachUsb { host, .. }
            | Self::DetachUsb { host, .. }
            | Self::Refresh { host } => host,
        }
    }

    /// Whether this action targets `node_id`. An empty target never matches
    /// (fail-safe: an unaddressed create must not fan out to every node).
    #[must_use]
    pub fn targets(&self, node_id: &str) -> bool {
        !self.host().is_empty() && self.host() == node_id
    }
}

/// Parse a [`LifecycleAction`] request body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `op`.
pub fn parse_action(body: &str) -> Result<LifecycleAction, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed lifecycle action: {e}"))
}

/// One row of `virsh list --all` — the lean instance record published in the
/// roster (name + state + libvirt id).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Instance {
    /// libvirt numeric id (`"-"` when not running).
    pub id: String,
    /// Domain name.
    pub name: String,
    /// Raw libvirt state string (`running`, `shut off`, `paused`, …).
    pub state: String,
}

/// Parsed `virsh dominfo <name>` fields — the richer per-domain detail the state
/// machine reads (`State`) and the trait's `info` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomainInfo {
    /// `Name`.
    pub name: String,
    /// `UUID`.
    pub uuid: String,
    /// `State` (raw libvirt string).
    pub state: String,
    /// `CPU(s)`.
    pub vcpus: u32,
    /// `Max memory`, converted KiB → MiB.
    pub max_mem_mib: u64,
}

impl DomainInfo {
    /// This domain's state as a [`VmState`] the state machine reasons over.
    #[must_use]
    pub fn state_kind(&self) -> VmState {
        vm_state_from_str(&self.state)
    }
}

/// The whole-node VM instance roster — the body published to [`INSTANCES_TOPIC`].
/// `host`-stamped like `kvm_health`'s summary so a consumer reads one node's row
/// off the flat topic.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstanceReport {
    /// Publishing node id.
    pub host: String,
    /// The node's VMs in `virsh list --all` order.
    pub instances: Vec<Instance>,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

// ─────────────────────────── state machine ───────────────────────────

/// A libvirt domain's coarse power state (the transitions the machine cares
/// about; `Other` folds the transient `idle`/`in shutdown`/`pmsuspended`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    /// `running`.
    Running,
    /// `paused`.
    Paused,
    /// `shut off`.
    ShutOff,
    /// `crashed`.
    Crashed,
    /// Any other/transient libvirt state.
    Other,
}

impl VmState {
    /// The canonical `virsh` state string for this state (round-trips
    /// [`vm_state_from_str`] for the well-known states).
    #[must_use]
    pub fn as_virsh_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::ShutOff => "shut off",
            Self::Crashed => "crashed",
            Self::Other => "unknown",
        }
    }
}

/// Map a raw `virsh` state string to a [`VmState`].
#[must_use]
pub fn vm_state_from_str(s: &str) -> VmState {
    match s.trim() {
        "running" => VmState::Running,
        "paused" => VmState::Paused,
        "shut off" => VmState::ShutOff,
        "crashed" => VmState::Crashed,
        _ => VmState::Other,
    }
}

/// The lifecycle operation a request maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleOp {
    /// Define a new domain.
    Create,
    /// Boot a defined domain.
    Start,
    /// Stop a running domain.
    Stop,
    /// Suspend a running domain (freeze).
    Pause,
    /// Un-freeze a suspended domain.
    Resume,
    /// Remove a domain.
    Destroy,
    /// Hot-attach a USB host device (E12-10 static passthrough).
    AttachUsb,
    /// Hot-detach a previously attached USB host device.
    DetachUsb,
}

/// The intended outcome of a valid transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Newly defined (shut off).
    Defined,
    /// Now running.
    Started,
    /// Now shut off.
    Stopped,
    /// Now suspended.
    Paused,
    /// Un-frozen — running again.
    Resumed,
    /// Removed.
    Removed,
    /// A USB host device was hot-attached (E12-10). The VM's coarse power
    /// state is unchanged.
    UsbAttached,
    /// A previously attached USB host device was hot-detached.
    UsbDetached,
}

/// A rejected transition — the precondition the current state failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransitionError {
    /// Create on a name that already exists.
    #[error("already exists")]
    AlreadyExists,
    /// An op on a name that doesn't exist.
    #[error("not found")]
    NotFound,
    /// Start on an already-running VM.
    #[error("already running")]
    AlreadyRunning,
    /// Stop on a VM that isn't running.
    #[error("not running")]
    NotRunning,
    /// Pause on a VM that's already suspended.
    #[error("already paused")]
    AlreadyPaused,
    /// Resume on a VM that isn't paused.
    #[error("not paused")]
    NotPaused,
    /// The op isn't valid from the current state.
    #[error("cannot {op:?} a VM in state {state:?}")]
    Invalid {
        /// The rejected op.
        op: LifecycleOp,
        /// The state it was rejected from.
        state: VmState,
    },
}

/// The pure lifecycle state machine: given the domain's current state (`None`
/// = doesn't exist) and an op, either the intended [`Transition`] or the
/// precondition [`TransitionError`]. No I/O — fully unit-testable.
///
/// # Errors
/// A [`TransitionError`] when the op is invalid from `current`.
pub fn plan_transition(
    current: Option<VmState>,
    op: LifecycleOp,
) -> Result<Transition, TransitionError> {
    match (op, current) {
        (LifecycleOp::Create, None) => Ok(Transition::Defined),
        (LifecycleOp::Create, Some(_)) => Err(TransitionError::AlreadyExists),

        (LifecycleOp::Start, None) => Err(TransitionError::NotFound),
        (LifecycleOp::Start, Some(VmState::Running)) => Err(TransitionError::AlreadyRunning),
        (LifecycleOp::Start, Some(VmState::ShutOff | VmState::Crashed | VmState::Paused)) => {
            Ok(Transition::Started)
        }
        (LifecycleOp::Start, Some(state)) => Err(TransitionError::Invalid {
            op: LifecycleOp::Start,
            state,
        }),

        (LifecycleOp::Stop, None) => Err(TransitionError::NotFound),
        (LifecycleOp::Stop, Some(VmState::Running | VmState::Paused)) => Ok(Transition::Stopped),
        (LifecycleOp::Stop, Some(VmState::ShutOff)) => Err(TransitionError::NotRunning),
        (LifecycleOp::Stop, Some(state)) => Err(TransitionError::Invalid {
            op: LifecycleOp::Stop,
            state,
        }),

        (LifecycleOp::Pause, None) => Err(TransitionError::NotFound),
        (LifecycleOp::Pause, Some(VmState::Running)) => Ok(Transition::Paused),
        (LifecycleOp::Pause, Some(VmState::Paused)) => Err(TransitionError::AlreadyPaused),
        (LifecycleOp::Pause, Some(state)) => Err(TransitionError::Invalid {
            op: LifecycleOp::Pause,
            state,
        }),

        (LifecycleOp::Resume, None) => Err(TransitionError::NotFound),
        (LifecycleOp::Resume, Some(VmState::Paused)) => Ok(Transition::Resumed),
        // Resume only wakes a paused VM; anything else (running/shut-off/crashed)
        // simply isn't paused.
        (LifecycleOp::Resume, Some(_)) => Err(TransitionError::NotPaused),

        (LifecycleOp::Destroy, None) => Err(TransitionError::NotFound),
        (LifecycleOp::Destroy, Some(_)) => Ok(Transition::Removed),

        // E12-10 static USB passthrough: hot-attach/detach only act on a
        // running domain — there's no live QEMU process to hotplug into
        // otherwise. A shut-off VM reads as the same "not running" refusal
        // Stop already uses; any other transient state is generically invalid.
        (LifecycleOp::AttachUsb, None) => Err(TransitionError::NotFound),
        (LifecycleOp::AttachUsb, Some(VmState::Running)) => Ok(Transition::UsbAttached),
        (LifecycleOp::AttachUsb, Some(VmState::ShutOff)) => Err(TransitionError::NotRunning),
        (LifecycleOp::AttachUsb, Some(state)) => Err(TransitionError::Invalid {
            op: LifecycleOp::AttachUsb,
            state,
        }),

        (LifecycleOp::DetachUsb, None) => Err(TransitionError::NotFound),
        (LifecycleOp::DetachUsb, Some(VmState::Running)) => Ok(Transition::UsbDetached),
        (LifecycleOp::DetachUsb, Some(VmState::ShutOff)) => Err(TransitionError::NotRunning),
        (LifecycleOp::DetachUsb, Some(state)) => Err(TransitionError::Invalid {
            op: LifecycleOp::DetachUsb,
            state,
        }),
    }
}

// ─────────────────────── pure: virsh argv builders ───────────────────────
// Each returns the argv WITHOUT the leading program. Kept pure + tested so the
// command surface can't silently drift (the `mackes-xcp` doctrine).

/// `virsh list --all` — the roster.
#[must_use]
pub fn build_list_argv() -> Vec<String> {
    vec!["list".into(), "--all".into()]
}

/// `virsh dominfo <name>`.
#[must_use]
pub fn build_dominfo_argv(name: &str) -> Vec<String> {
    vec!["dominfo".into(), name.into()]
}

/// `virsh define <xml_path>` — define a persistent domain from its XML.
#[must_use]
pub fn build_define_argv(xml_path: &str) -> Vec<String> {
    vec!["define".into(), xml_path.into()]
}

/// `virsh start <name>`.
#[must_use]
pub fn build_start_argv(name: &str) -> Vec<String> {
    vec!["start".into(), name.into()]
}

/// `virsh suspend <name>` — freeze a running domain (the pause verb).
#[must_use]
pub fn build_suspend_argv(name: &str) -> Vec<String> {
    vec!["suspend".into(), name.into()]
}

/// `virsh resume <name>` — un-freeze a suspended domain.
#[must_use]
pub fn build_resume_argv(name: &str) -> Vec<String> {
    vec!["resume".into(), name.into()]
}

/// `virsh shutdown <name>` (graceful) or `virsh destroy <name>` (`force`). NOTE
/// libvirt's `destroy` is a **force-off**, not a delete — the delete verb is
/// [`build_undefine_argv`].
#[must_use]
pub fn build_stop_argv(name: &str, force: bool) -> Vec<String> {
    if force {
        vec!["destroy".into(), name.into()]
    } else {
        vec!["shutdown".into(), name.into()]
    }
}

/// `virsh destroy <name>` — force-off (the best-effort first half of destroy).
#[must_use]
pub fn build_force_off_argv(name: &str) -> Vec<String> {
    vec!["destroy".into(), name.into()]
}

/// `virsh undefine <name> [--remove-all-storage]` — remove the domain (and its
/// disks when `remove_storage`).
#[must_use]
pub fn build_undefine_argv(name: &str, remove_storage: bool) -> Vec<String> {
    let mut a = vec!["undefine".into(), name.into()];
    if remove_storage {
        a.push("--remove-all-storage".into());
    }
    a
}

/// `virsh attach-device <name> <xml_path> --live` — hot-attach a device
/// described by the XML at `xml_path` into a running domain (E12-10 static
/// USB passthrough).
#[must_use]
pub fn build_attach_device_argv(name: &str, xml_path: &str) -> Vec<String> {
    vec![
        "attach-device".into(),
        name.into(),
        xml_path.into(),
        "--live".into(),
    ]
}

/// `virsh detach-device <name> <xml_path> --live` — hot-detach a previously
/// attached device described by the XML at `xml_path`.
#[must_use]
pub fn build_detach_device_argv(name: &str, xml_path: &str) -> Vec<String> {
    vec![
        "detach-device".into(),
        name.into(),
        xml_path.into(),
        "--live".into(),
    ]
}

/// `virsh pool-list --all --name` (one pool name per line).
#[must_use]
pub fn build_pool_list_argv() -> Vec<String> {
    vec!["pool-list".into(), "--all".into(), "--name".into()]
}

/// `virsh pool-define-as <name> dir - - - - <dir>` — the positional dir-pool form
/// (mirrors `compute_provision`'s VIRT-3 pool define).
#[must_use]
pub fn build_pool_define_argv(name: &str, dir: &str) -> Vec<String> {
    vec![
        "pool-define-as".into(),
        name.into(),
        "dir".into(),
        "-".into(),
        "-".into(),
        "-".into(),
        "-".into(),
        dir.into(),
    ]
}

/// `virsh pool-start <name>`.
#[must_use]
pub fn build_pool_start_argv(name: &str) -> Vec<String> {
    vec!["pool-start".into(), name.into()]
}

/// `virsh pool-autostart <name>`.
#[must_use]
pub fn build_pool_autostart_argv(name: &str) -> Vec<String> {
    vec!["pool-autostart".into(), name.into()]
}

/// `virsh net-list --all`.
#[must_use]
pub fn build_net_list_argv() -> Vec<String> {
    vec!["net-list".into(), "--all".into()]
}

/// `virsh net-start <name>`.
#[must_use]
pub fn build_net_start_argv(name: &str) -> Vec<String> {
    vec!["net-start".into(), name.into()]
}

/// `virsh net-autostart <name>`.
#[must_use]
pub fn build_net_autostart_argv(name: &str) -> Vec<String> {
    vec!["net-autostart".into(), name.into()]
}

/// `qemu-img create` argv for a VM's backing disk. With `image` it is a
/// copy-on-write **overlay** (`-b <image> -F qcow2`) — the create-from-image
/// golden pattern, so the golden is never written; without it, a blank qcow2 of
/// `disk_gb`. A `disk_gb` of 0 omits the size (an overlay inherits the base's).
#[must_use]
pub fn build_qemu_img_argv(image: Option<&str>, dest: &str, disk_gb: u64) -> Vec<String> {
    let mut a = vec!["create".into(), "-f".into(), "qcow2".into()];
    if let Some(base) = image {
        a.push("-b".into());
        a.push(base.into());
        a.push("-F".into());
        a.push("qcow2".into());
    }
    a.push(dest.into());
    if disk_gb > 0 {
        a.push(format!("{disk_gb}G"));
    }
    a
}

/// Minimal XML-escape for the handful of values interpolated into the domain XML
/// (name + paths + network). `&` first so it doesn't double-escape the entities.
#[must_use]
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build a standalone `<hostdev>` USB-passthrough device-XML fragment,
/// addressed by USB vendor/product id — the payload `virsh
/// attach-device`/`detach-device --live` consumes for E12-10's static USB
/// passthrough (admin-selected device, hot-attached). The dynamic SPICE
/// `usbredir` click-to-redirect UX is a separate, deferred mechanism — see
/// `docs/design/e12-9-10-libvirt-rescope.md`.
#[must_use]
pub fn build_usb_hostdev_xml(vendor: &str, product: &str) -> String {
    format!(
        "<hostdev mode='subsystem' type='usb'>\n\
         \x20 <source>\n\
         \x20   <vendor id='{vendor}'/>\n\
         \x20   <product id='{product}'/>\n\
         \x20 </source>\n\
         </hostdev>\n",
        vendor = xml_escape(vendor),
        product = xml_escape(product),
    )
}

/// Build a standalone `<hostdev>` PCI-passthrough (VFIO) device-XML fragment
/// for one [`PciAddress`] — `managed='yes'` so libvirt handles the
/// detach-from-host/attach-to-guest/reset cycle around the guest's lifetime.
///
/// E12-10 VFIO: **pure, unit-tested XML construction only.** This project's
/// farm build-VM slots run nested atop Xen dom0s (no realistic IOMMU
/// passthrough there), and no inventoried physical seat has a confirmed
/// second GPU or checked IOMMU/VT-d BIOS state — so the live runtime path is
/// explicitly NOT validated by this function or its tests. This mirrors the
/// pre-existing DATACENTER-22 finding on the old Xen/`xen-pciback` stack
/// (`install-helpers/setup-workstation-passthrough.sh`, stays `[!]`
/// hardware-gated) — same wall, new hypervisor. See
/// `docs/design/e12-9-10-libvirt-rescope.md`'s VFIO §Open question.
#[must_use]
pub fn build_pci_hostdev_xml(addr: &PciAddress) -> String {
    format!(
        "<hostdev mode='subsystem' type='pci' managed='yes'>\n\
         \x20 <source>\n\
         \x20   <address domain='{domain}' bus='{bus}' slot='{slot}' function='{function}'/>\n\
         \x20 </source>\n\
         </hostdev>\n",
        domain = xml_escape(&addr.domain),
        bus = xml_escape(&addr.bus),
        slot = xml_escape(&addr.slot),
        function = xml_escape(&addr.function),
    )
}

/// Build the libvirt domain XML `virsh define` consumes for `spec`, with
/// `disk_path` as the backing qcow2. A coherent, minimal q35/KVM baseline:
/// host-passthrough CPU, a virtio disk + NIC, a serial console, the qemu
/// guest-agent channel, spice graphics (localhost-listen), virtio
/// video/balloon, local PipeWire audio (E12-9: `<sound>` + `<audio
/// type='pipewire'>` — every VM gets this, see the module docs), and any
/// VFIO/PCI passthrough devices from `spec.pci_passthrough` (E12-10 — pure
/// XML only, see [`build_pci_hostdev_xml`]). Richer knobs (UEFI/OVMF
/// firmware, virtiofs, TPM) remain follow-ons; static USB passthrough is a
/// separate hot-attach op ([`LifecycleAction::AttachUsb`]), not part of the
/// initial device set.
///
/// **Video 3D acceleration (QC-23 Tier 0, unconditional):** the virtio video
/// model always carries `<acceleration accel3d='yes'/>`
/// (`docs/design/qc23-virtio-gpu-zerocopy-rescope.md` §5) — a real guest-3D
/// (virgl/venus) performance win, compatible with the existing
/// `<graphics type='spice'>` stanza unchanged (§3.2: 3D acceleration is
/// independent of the delivery mechanism). This does **not** make display
/// delivery zero-copy — the SPICE CPU-copy path (§1.1) is unaffected — and it
/// does not by itself confirm the host's QEMU build was compiled with
/// `--enable-virglrenderer` (§3.2's flagged-open, unverified item; cheap to
/// check on a real box, not assumed here).
///
/// **On the mixer tag (E12-9, deliberately NOT wired here):** the `<output>`'s
/// `streamName='vm-{name}'` is the doc-recommended connecting tissue toward
/// `mde-seat/src/mixer.rs`'s `mde.vm.name`-keyed `classify_origin` (the
/// already-shipped E12-16 mixer's VM-origin classifier). It is intentionally
/// **not** wired end-to-end in this slice: which `pw-dump` JSON property
/// libvirt's `streamName` actually lands under is unverified (needs a real
/// running VM this environment can't spin up), and `mackesd` has no existing
/// PipeWire seam/dependency to add a post-hoc stamping step blind (that would
/// be new, cross-cutting, unverified integration work, not a small delta —
/// see `docs/design/e12-9-10-libvirt-rescope.md`'s "one open verification
/// step"). Guessing either the prop mapping or the stamping mechanism here
/// would be exactly the kind of unverified claim this project's honest-gating
/// convention avoids.
#[must_use]
pub fn build_domain_xml(spec: &VmSpec, disk_path: &str) -> String {
    // E12-10 VFIO: each configured PCI passthrough device becomes its own
    // <hostdev> stanza. Empty by default (`VmSpec::pci_passthrough` defaults
    // to `vec![]`), so an unconfigured VM's XML is byte-for-byte unchanged.
    let pci_hostdevs: String = spec
        .pci_passthrough
        .iter()
        .map(build_pci_hostdev_xml)
        .collect();

    format!(
        "<domain type='kvm'>\n\
         \x20 <name>{name}</name>\n\
         \x20 <memory unit='MiB'>{mem}</memory>\n\
         \x20 <currentMemory unit='MiB'>{mem}</currentMemory>\n\
         \x20 <vcpu placement='static'>{vcpus}</vcpu>\n\
         \x20 <os>\n\
         \x20   <type arch='x86_64' machine='q35'>hvm</type>\n\
         \x20   <boot dev='hd'/>\n\
         \x20 </os>\n\
         \x20 <features>\n\
         \x20   <acpi/>\n\
         \x20   <apic/>\n\
         \x20 </features>\n\
         \x20 <cpu mode='host-passthrough' check='none'/>\n\
         \x20 <clock offset='utc'/>\n\
         \x20 <on_poweroff>destroy</on_poweroff>\n\
         \x20 <on_reboot>restart</on_reboot>\n\
         \x20 <on_crash>destroy</on_crash>\n\
         \x20 <devices>\n\
         \x20   <disk type='file' device='disk'>\n\
         \x20     <driver name='qemu' type='qcow2'/>\n\
         \x20     <source file='{disk}'/>\n\
         \x20     <target dev='vda' bus='virtio'/>\n\
         \x20   </disk>\n\
         \x20   <interface type='network'>\n\
         \x20     <source network='{net}'/>\n\
         \x20     <model type='virtio'/>\n\
         \x20   </interface>\n\
         \x20   <console type='pty'/>\n\
         \x20   <channel type='unix'>\n\
         \x20     <target type='virtio' name='org.qemu.guest_agent.0'/>\n\
         \x20   </channel>\n\
         \x20   <graphics type='spice' autoport='yes'>\n\
         \x20     <listen type='address' address='127.0.0.1'/>\n\
         \x20   </graphics>\n\
         \x20   <video>\n\
         \x20     <model type='virtio'>\n\
         \x20       <acceleration accel3d='yes'/>\n\
         \x20     </model>\n\
         \x20   </video>\n\
         \x20   <memballoon model='virtio'/>\n\
         \x20   <sound model='virtio'/>\n\
         \x20   <audio id='1' type='pipewire'>\n\
         \x20     <output name='mde-vms' streamName='vm-{name}' latency='40'/>\n\
         \x20   </audio>\n\
         {pci_hostdevs}\
         \x20 </devices>\n\
         </domain>\n",
        name = xml_escape(&spec.name),
        mem = spec.ram_mb,
        vcpus = spec.vcpus,
        disk = xml_escape(disk_path),
        net = xml_escape(spec.network_or_default()),
        pci_hostdevs = pci_hostdevs,
    )
}

// ─────────────────────────── pure: parsers ───────────────────────────

/// Parse `virsh list --all` into [`Instance`] rows. Skips the header + the
/// dashed separator; each data row is `Id  Name  State…` (State may be
/// multi-word, e.g. `shut off`).
#[must_use]
pub fn parse_virsh_list(stdout: &str) -> Vec<Instance> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // The dashed separator under the header.
        if t.chars().all(|c| c == '-') {
            continue;
        }
        let mut it = t.split_whitespace();
        let (Some(id), Some(name)) = (it.next(), it.next()) else {
            continue;
        };
        // The header row (`Id Name State`) — skip it.
        if id.eq_ignore_ascii_case("Id") && name.eq_ignore_ascii_case("Name") {
            continue;
        }
        let state = it.collect::<Vec<_>>().join(" ");
        out.push(Instance {
            id: id.to_string(),
            name: name.to_string(),
            state,
        });
    }
    out
}

/// Parse `virsh dominfo <name>`. `None` only when the required Name + State
/// fields are absent (e.g. an error payload).
#[must_use]
pub fn parse_virsh_dominfo(stdout: &str) -> Option<DomainInfo> {
    let mut d = DomainInfo::default();
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Name" => d.name = value.to_string(),
            "UUID" => d.uuid = value.to_string(),
            "State" => d.state = value.to_string(),
            "CPU(s)" => {
                if let Ok(n) = value.parse::<u32>() {
                    d.vcpus = n;
                }
            }
            "Max memory" => {
                // `2097152 KiB` — first token is the KiB count.
                if let Some(kib) = value.split_whitespace().next() {
                    if let Ok(kib) = kib.parse::<u64>() {
                        d.max_mem_mib = kib / 1024;
                    }
                }
            }
            _ => {}
        }
    }
    if d.name.is_empty() || d.state.is_empty() {
        return None;
    }
    Some(d)
}

/// `true` when `pool` appears in a `virsh pool-list --all --name` payload.
#[must_use]
pub fn pool_exists(pool_list_stdout: &str, pool: &str) -> bool {
    pool_list_stdout.lines().map(str::trim).any(|l| l == pool)
}

/// `true` when `net` is listed **active** in a `virsh net-list --all` payload
/// (row `Name  State  Autostart  Persistent`, State == `active`).
#[must_use]
pub fn net_is_active(net_list_stdout: &str, net: &str) -> bool {
    for line in net_list_stdout.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(state)) = (it.next(), it.next()) else {
            continue;
        };
        if name == net {
            return state.eq_ignore_ascii_case("active");
        }
    }
    false
}

// ─────────────────────── backend trait + errors ───────────────────────

/// A libvirt-access failure.
#[derive(Debug, Error)]
pub enum LibvirtError {
    /// The `virsh`/`qemu-img` process couldn't be spawned or timed out.
    #[error("spawn {0}: {1}")]
    Spawn(String, #[source] std::io::Error),
    /// A command exited non-zero — carries the sub-command, exit code, and any
    /// captured stderr.
    #[error("{cmd} failed (exit {code}): {stderr}")]
    Command {
        /// The failing sub-command (first argv element).
        cmd: String,
        /// Process exit code (or -1 if killed by signal).
        code: i32,
        /// Captured stderr (empty for status-only calls whose stderr is nulled).
        stderr: String,
    },
    /// A disk-prep filesystem error (mkdir / write the domain XML).
    #[error("disk: {0}")]
    Disk(String),
}

/// The injectable libvirt-access seam (MV-3). [`VirshCli`] is the production
/// `virsh`/`qemu-img` impl; a `FakeLibvirt` drives the unit tests.
pub trait LibvirtBackend {
    /// Ensure the default dir **storage pool** exists + is started/autostarted.
    /// Idempotent. Called before a create.
    ///
    /// # Errors
    /// Spawn / non-zero virsh / disk failures.
    fn ensure_default_pool(&self) -> Result<(), LibvirtError>;

    /// Ensure the default NAT **network** is active + autostarted. Idempotent.
    /// Best-effort on the start/autostart (a pre-active network is fine).
    ///
    /// # Errors
    /// Spawn / non-zero virsh failures reading the network list.
    fn ensure_default_network(&self) -> Result<(), LibvirtError>;

    /// Create-from-image: prepare the backing disk (overlay or blank) then
    /// `virsh define` the domain. Leaves it shut off.
    ///
    /// # Errors
    /// Spawn / non-zero virsh|qemu-img / disk failures.
    fn create(&self, spec: &VmSpec) -> Result<(), LibvirtError>;

    /// Start a defined domain.
    ///
    /// # Errors
    /// Spawn / non-zero virsh failures.
    fn start(&self, name: &str) -> Result<(), LibvirtError>;

    /// Suspend a running domain (freeze; RAM retained).
    ///
    /// # Errors
    /// Spawn / non-zero virsh failures.
    fn pause(&self, name: &str) -> Result<(), LibvirtError>;

    /// Resume a suspended domain.
    ///
    /// # Errors
    /// Spawn / non-zero virsh failures.
    fn resume(&self, name: &str) -> Result<(), LibvirtError>;

    /// Stop a domain — graceful, or a hard force-off when `force`.
    ///
    /// # Errors
    /// Spawn / non-zero virsh failures.
    fn stop(&self, name: &str, force: bool) -> Result<(), LibvirtError>;

    /// Force-off (if running) then undefine a domain, optionally removing its
    /// storage.
    ///
    /// # Errors
    /// Spawn / non-zero virsh failure on the operative undefine (a force-off
    /// error on an already-off VM is tolerated).
    fn destroy(&self, name: &str, remove_storage: bool) -> Result<(), LibvirtError>;

    /// Hot-attach a USB host device (by vendor/product id) into a running
    /// domain (E12-10 static passthrough; `virsh attach-device --live`).
    ///
    /// # Errors
    /// Spawn / non-zero virsh / disk failures (writing the device XML).
    fn attach_usb(&self, name: &str, vendor: &str, product: &str) -> Result<(), LibvirtError>;

    /// Hot-detach a previously attached USB host device (`virsh
    /// detach-device --live`).
    ///
    /// # Errors
    /// Spawn / non-zero virsh / disk failures (writing the device XML).
    fn detach_usb(&self, name: &str, vendor: &str, product: &str) -> Result<(), LibvirtError>;

    /// The node's VM roster (`virsh list --all`).
    ///
    /// # Errors
    /// Spawn / non-zero virsh failures.
    fn list(&self) -> Result<Vec<Instance>, LibvirtError>;

    /// Detail for one domain, or `None` when it doesn't exist.
    ///
    /// # Errors
    /// Spawn / non-zero virsh failures (a not-found is `Ok(None)`, not an error).
    fn info(&self, name: &str) -> Result<Option<DomainInfo>, LibvirtError>;
}

/// Production [`LibvirtBackend`]: shells `virsh`/`qemu-img` through the bounded
/// [`crate::workers::proc`] path so a wedged libvirt can't pin a runtime thread.
/// Holds only the pool + default-network identity; every call is stateless.
#[derive(Debug, Clone)]
pub struct VirshCli {
    /// libvirt pool name.
    pool_name: String,
    /// Filesystem dir backing the pool (also where per-VM disks land).
    pool_dir: PathBuf,
    /// Default network name.
    network: String,
}

impl Default for VirshCli {
    fn default() -> Self {
        Self::new()
    }
}

impl VirshCli {
    /// Production defaults: the `mde-vms` dir pool at [`DEFAULT_POOL_DIR`] + the
    /// `default` NAT network.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pool_name: DEFAULT_POOL_NAME.to_string(),
            pool_dir: PathBuf::from(DEFAULT_POOL_DIR),
            network: DEFAULT_NETWORK.to_string(),
        }
    }

    /// Run a program to completion (status only; stdout/stderr nulled), bounded.
    fn run_status(program: &str, args: &[String]) -> Result<(), LibvirtError> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        let st = status_with_timeout(cmd, DEFAULT_CMD_TIMEOUT)
            .map_err(|e| LibvirtError::Spawn(program.to_string(), e))?;
        if st.success() {
            Ok(())
        } else {
            Err(LibvirtError::Command {
                cmd: args.first().cloned().unwrap_or_else(|| program.to_string()),
                code: st.code().unwrap_or(-1),
                stderr: String::new(),
            })
        }
    }

    /// Run `virsh <args>` capturing stdout+stderr, bounded.
    fn virsh_output(&self, args: &[String]) -> Result<(bool, String, String), LibvirtError> {
        let _ = self;
        let mut cmd = Command::new("virsh");
        cmd.args(args);
        let out = output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT)
            .map_err(|e| LibvirtError::Spawn("virsh".to_string(), e))?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// `virsh <args>` status-only.
    fn virsh_status(&self, args: &[String]) -> Result<(), LibvirtError> {
        let _ = self;
        Self::run_status("virsh", args)
    }

    /// Write `device_xml` to a temp file and run `virsh attach-device`
    /// (`attach`) or `detach-device` (`!attach`) `--live` against `name`,
    /// cleaning up the temp file regardless of outcome. Shared by the E12-10
    /// USB hostdev attach/detach calls — mirrors [`Self::create`]'s
    /// XML-to-tempfile pattern for `virsh define`.
    fn device_xml_action(
        &self,
        name: &str,
        device_xml: &str,
        attach: bool,
    ) -> Result<(), LibvirtError> {
        let xml_path = std::env::temp_dir().join(format!("mde-vm-{name}-dev.xml"));
        std::fs::write(&xml_path, device_xml.as_bytes())
            .map_err(|e| LibvirtError::Disk(format!("write device xml: {e}")))?;
        let argv = if attach {
            build_attach_device_argv(name, &xml_path.to_string_lossy())
        } else {
            build_detach_device_argv(name, &xml_path.to_string_lossy())
        };
        let res = self.virsh_status(&argv);
        let _ = std::fs::remove_file(&xml_path);
        res
    }
}

impl LibvirtBackend for VirshCli {
    fn ensure_default_pool(&self) -> Result<(), LibvirtError> {
        let (ok, stdout, stderr) = self.virsh_output(&build_pool_list_argv())?;
        if !ok {
            return Err(LibvirtError::Command {
                cmd: "pool-list".into(),
                code: -1,
                stderr: stderr.trim().to_string(),
            });
        }
        if pool_exists(&stdout, &self.pool_name) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.pool_dir)
            .map_err(|e| LibvirtError::Disk(format!("mkdir {}: {e}", self.pool_dir.display())))?;
        self.virsh_status(&build_pool_define_argv(
            &self.pool_name,
            &self.pool_dir.to_string_lossy(),
        ))?;
        self.virsh_status(&build_pool_start_argv(&self.pool_name))?;
        // Autostart is a nicety — don't fail the create if it can't be set.
        let _ = self.virsh_status(&build_pool_autostart_argv(&self.pool_name));
        Ok(())
    }

    fn ensure_default_network(&self) -> Result<(), LibvirtError> {
        let (ok, stdout, stderr) = self.virsh_output(&build_net_list_argv())?;
        if !ok {
            return Err(LibvirtError::Command {
                cmd: "net-list".into(),
                code: -1,
                stderr: stderr.trim().to_string(),
            });
        }
        if net_is_active(&stdout, &self.network) {
            return Ok(());
        }
        // Best-effort: a network that's defined-but-inactive gets started; one
        // that isn't defined at all is a host-provisioning concern (node-virt.yml
        // ships the default network), so we don't try to define it here.
        let _ = self.virsh_status(&build_net_start_argv(&self.network));
        let _ = self.virsh_status(&build_net_autostart_argv(&self.network));
        Ok(())
    }

    fn create(&self, spec: &VmSpec) -> Result<(), LibvirtError> {
        // 1. Backing disk into the pool dir.
        std::fs::create_dir_all(&self.pool_dir)
            .map_err(|e| LibvirtError::Disk(format!("mkdir {}: {e}", self.pool_dir.display())))?;
        let disk = self.pool_dir.join(format!("{}.qcow2", spec.name));
        let disk_str = disk.to_string_lossy().into_owned();
        Self::run_status(
            "qemu-img",
            &build_qemu_img_argv(spec.image_path.as_deref(), &disk_str, spec.disk_gb),
        )?;
        // 2. Domain XML → temp file → `virsh define` → clean up.
        let xml = build_domain_xml(spec, &disk_str);
        let xml_path = std::env::temp_dir().join(format!("mde-vm-{}.xml", spec.name));
        std::fs::write(&xml_path, xml.as_bytes())
            .map_err(|e| LibvirtError::Disk(format!("write domain xml: {e}")))?;
        let res = self.virsh_status(&build_define_argv(&xml_path.to_string_lossy()));
        let _ = std::fs::remove_file(&xml_path);
        res
    }

    fn start(&self, name: &str) -> Result<(), LibvirtError> {
        self.virsh_status(&build_start_argv(name))
    }

    fn pause(&self, name: &str) -> Result<(), LibvirtError> {
        self.virsh_status(&build_suspend_argv(name))
    }

    fn resume(&self, name: &str) -> Result<(), LibvirtError> {
        self.virsh_status(&build_resume_argv(name))
    }

    fn stop(&self, name: &str, force: bool) -> Result<(), LibvirtError> {
        self.virsh_status(&build_stop_argv(name, force))
    }

    fn destroy(&self, name: &str, remove_storage: bool) -> Result<(), LibvirtError> {
        // Best-effort force-off (a halted VM errors here — tolerated), then the
        // operative undefine.
        let _ = self.virsh_status(&build_force_off_argv(name));
        self.virsh_status(&build_undefine_argv(name, remove_storage))
    }

    fn attach_usb(&self, name: &str, vendor: &str, product: &str) -> Result<(), LibvirtError> {
        self.device_xml_action(name, &build_usb_hostdev_xml(vendor, product), true)
    }

    fn detach_usb(&self, name: &str, vendor: &str, product: &str) -> Result<(), LibvirtError> {
        self.device_xml_action(name, &build_usb_hostdev_xml(vendor, product), false)
    }

    fn list(&self) -> Result<Vec<Instance>, LibvirtError> {
        let (ok, stdout, stderr) = self.virsh_output(&build_list_argv())?;
        if !ok {
            return Err(LibvirtError::Command {
                cmd: "list".into(),
                code: -1,
                stderr: stderr.trim().to_string(),
            });
        }
        Ok(parse_virsh_list(&stdout))
    }

    fn info(&self, name: &str) -> Result<Option<DomainInfo>, LibvirtError> {
        let (ok, stdout, stderr) = self.virsh_output(&build_dominfo_argv(name))?;
        if !ok {
            // A not-found domain is `Ok(None)`, not an error — that's how the
            // create precondition (name must not exist) reads a fresh name.
            let s = stderr.to_ascii_lowercase();
            if s.contains("not found") || s.contains("failed to get domain") {
                return Ok(None);
            }
            return Err(LibvirtError::Command {
                cmd: "dominfo".into(),
                code: -1,
                stderr: stderr.trim().to_string(),
            });
        }
        Ok(parse_virsh_dominfo(&stdout))
    }
}

/// Apply one lifecycle action over an injected [`LibvirtBackend`]: read the
/// domain's current state, validate the transition ([`plan_transition`]), then
/// call the backend. `Refresh` is a no-op (it only nudges a roster publish).
/// Pure over the backend seam — driven by `FakeLibvirt` in tests.
///
/// # Errors
/// A human-readable message when the transition is invalid or a backend call
/// fails (the worker logs it on the alert lane and moves on).
pub fn apply_action(backend: &dyn LibvirtBackend, action: &LifecycleAction) -> Result<(), String> {
    let (name, op) = match action {
        LifecycleAction::Refresh { .. } => return Ok(()),
        LifecycleAction::Create { spec, .. } => (spec.name.as_str(), LifecycleOp::Create),
        LifecycleAction::Start { name, .. } => (name.as_str(), LifecycleOp::Start),
        LifecycleAction::Stop { name, .. } => (name.as_str(), LifecycleOp::Stop),
        LifecycleAction::Pause { name, .. } => (name.as_str(), LifecycleOp::Pause),
        LifecycleAction::Resume { name, .. } => (name.as_str(), LifecycleOp::Resume),
        LifecycleAction::Destroy { name, .. } => (name.as_str(), LifecycleOp::Destroy),
        LifecycleAction::AttachUsb { name, .. } => (name.as_str(), LifecycleOp::AttachUsb),
        LifecycleAction::DetachUsb { name, .. } => (name.as_str(), LifecycleOp::DetachUsb),
    };
    let current = backend
        .info(name)
        .map_err(|e| e.to_string())?
        .map(|d| d.state_kind());
    plan_transition(current, op).map_err(|e| format!("vm '{name}': {e}"))?;
    match action {
        LifecycleAction::Create { spec, .. } => {
            backend.ensure_default_pool().map_err(|e| e.to_string())?;
            backend
                .ensure_default_network()
                .map_err(|e| e.to_string())?;
            backend.create(spec).map_err(|e| e.to_string())
        }
        LifecycleAction::Start { name, .. } => backend.start(name).map_err(|e| e.to_string()),
        LifecycleAction::Pause { name, .. } => backend.pause(name).map_err(|e| e.to_string()),
        LifecycleAction::Resume { name, .. } => backend.resume(name).map_err(|e| e.to_string()),
        LifecycleAction::Stop { name, force, .. } => {
            backend.stop(name, *force).map_err(|e| e.to_string())
        }
        LifecycleAction::Destroy {
            name,
            remove_storage,
            ..
        } => backend
            .destroy(name, *remove_storage)
            .map_err(|e| e.to_string()),
        LifecycleAction::AttachUsb {
            name,
            vendor,
            product,
            ..
        } => backend
            .attach_usb(name, vendor, product)
            .map_err(|e| e.to_string()),
        LifecycleAction::DetachUsb {
            name,
            vendor,
            product,
            ..
        } => backend
            .detach_usb(name, vendor, product)
            .map_err(|e| e.to_string()),
        LifecycleAction::Refresh { .. } => Ok(()),
    }
}

// ─────────────────────────── bus + worker ───────────────────────────

/// Publish an [`InstanceReport`] to [`INSTANCES_TOPIC`] in-process (perf-10 /
/// arch-6) — no fork+exec of the `mde-bus` CLI (a whole process + a fresh SQLite
/// open + a [`crate::proc_reap`] reaper thread) per roster. Byte-identical stored
/// row to the old `mde-bus publish <topic> --body-flag <json>` (the compact
/// `serde_json` of the report). `bus_root` is [`VmLifecycleWorker::publish_bus_root`]
/// — the MDE_BUS_ROOT-honouring root the fork+exec'd CLI resolved via the
/// inherited env (NOT the worker's `dirs::data_dir()`-based ACTION-read root,
/// which the live daemon's `MDE_BUS_ROOT=/run/mde-bus` diverges from).
/// Best-effort: an absent root / failed open / write error is swallowed.
fn publish_instances(bus_root: Option<&Path>, report: &InstanceReport) {
    if let Some(mut persist) = crate::bus_publish::open_bus(bus_root.map(Path::to_path_buf)) {
        crate::bus_publish::publish_json(&mut persist, INSTANCES_TOPIC, report);
    }
}

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `compute_provision`.
fn read_new_actions(bus_root: &Path, cursor: &mut Option<String>) -> Vec<LifecycleAction> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_action(body) {
            Ok(a) => out.push(a),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "vm_lifecycle: bad lifecycle action")
            }
        }
    }
    out
}

/// Seed the cursor to the newest existing message so a (re)start doesn't
/// re-execute the whole backlog of lifecycle commands — a queued `destroy`
/// shouldn't fire on the next daemon restart. `None` when the topic is empty.
fn prime_cursor(bus_root: &Path) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(ACTION_TOPIC, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The MV-3 vm-lifecycle worker.
pub struct VmLifecycleWorker {
    /// This node's id — the create/event `host` stamp AND the action target
    /// this worker matches ([`LifecycleAction::targets`]).
    host: String,
    /// The injectable libvirt seam (production: [`VirshCli`]). `Arc` so each
    /// action runs on a `spawn_blocking` thread without borrowing `self`.
    backend: Arc<dyn LibvirtBackend + Send + Sync>,
    /// Action-drain cadence.
    poll: Duration,
    /// Roster-publish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl VmLifecycleWorker {
    /// Construct with production defaults: the live [`VirshCli`] backend, the
    /// default cadences, and the auto-resolved bus root. `host` is this node's
    /// id (the action target + event stamp).
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            backend: Arc::new(VirshCli::new()),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
        }
    }

    /// Inject a backend (tests). Production uses the [`VirshCli`] default.
    #[must_use]
    pub fn with_backend(mut self, backend: Arc<dyn LibvirtBackend + Send + Sync>) -> Self {
        self.backend = backend;
        self
    }

    /// Override the action-drain cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
    }

    /// The root the in-process roster publish targets (perf-10). A test's
    /// `with_bus_root` override wins (so a driven `run` publishes into the temp
    /// store, never the real one); production falls back to
    /// [`crate::bus_publish::default_bus_root`] — the MDE_BUS_ROOT-honouring root
    /// the fork+exec'd `mde-bus` used, NOT the `dirs`-based [`default_bus_root`]
    /// this worker READS actions from.
    fn publish_bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override
            .clone()
            .or_else(crate::bus_publish::default_bus_root)
    }

    /// Drain + apply new actions addressed to this node. Returns `true` when any
    /// action ran (so the caller force-publishes the fresh roster).
    async fn drain_and_apply(&self, bus_root: &Path, cursor: &mut Option<String>) -> bool {
        let actions = read_new_actions(bus_root, cursor);
        let mut acted = false;
        for action in actions {
            if !action.targets(&self.host) {
                continue;
            }
            // Refresh is a pure publish nudge — no backend work.
            if matches!(action, LifecycleAction::Refresh { .. }) {
                acted = true;
                continue;
            }
            let backend = Arc::clone(&self.backend);
            match tokio::task::spawn_blocking(move || apply_action(&*backend, &action)).await {
                Ok(Ok(())) => acted = true,
                Ok(Err(e)) => {
                    // Alert lane (mirrors kvm_health) — a failed operator action
                    // is operator-visible.
                    tracing::warn!(
                        target: "mackesd::alert",
                        "ALERT (warn): vm_lifecycle action failed — {e}",
                    );
                }
                Err(e) => tracing::warn!(error = %e, "vm_lifecycle: action task join failed"),
            }
        }
        acted
    }

    /// Snapshot the roster (`virsh list`) + publish it, gated so the virsh call
    /// stays off the hot path: query only when `force` (an action just changed
    /// state) or the heartbeat has elapsed since `last_at`.
    async fn publish_snapshot(
        &self,
        publish_root: Option<&Path>,
        last_at: &mut Option<Instant>,
        force: bool,
    ) {
        let now = Instant::now();
        let due = force
            || match last_at {
                None => true,
                Some(at) => now.duration_since(*at) >= self.heartbeat,
            };
        if !due {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let instances = match tokio::task::spawn_blocking(move || backend.list()).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "vm_lifecycle: list failed; skipping publish");
                return;
            }
            Err(_) => return,
        };
        let report = InstanceReport {
            host: self.host.clone(),
            instances,
            published_at_ms: now_ms(),
        };
        publish_instances(publish_root, &report);
        *last_at = Some(now);
    }
}

#[async_trait::async_trait]
impl Worker for VmLifecycleWorker {
    fn name(&self) -> &'static str {
        "vm_lifecycle"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = self.bus_root();
        let publish_root = self.publish_bus_root();
        // Publish an immediate roster on start so a panel doesn't wait a full
        // heartbeat for the first row.
        let mut last_pub: Option<Instant> = None;
        self.publish_snapshot(publish_root.as_deref(), &mut last_pub, true)
            .await;
        // Skip any backlog so a restart doesn't re-run stale lifecycle commands.
        let mut cursor = bus_root.as_deref().and_then(prime_cursor);
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let acted = if let Some(root) = &bus_root {
                        self.drain_and_apply(root, &mut cursor).await
                    } else {
                        false
                    };
                    self.publish_snapshot(publish_root.as_deref(), &mut last_pub, acted).await;
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // ── request parsing ──

    #[test]
    fn parse_create_action_round_trips() {
        let body = r#"{"op":"create","host":"node-a","spec":{"name":"web1","vcpus":2,"ram_mb":2048,"disk_gb":20,"image_path":"/img/fedora.qcow2","network":"default"}}"#;
        let a = parse_action(body).expect("parse");
        match &a {
            LifecycleAction::Create { host, spec } => {
                assert_eq!(host, "node-a");
                assert_eq!(spec.name, "web1");
                assert_eq!(spec.vcpus, 2);
                assert_eq!(spec.ram_mb, 2048);
                assert_eq!(spec.disk_gb, 20);
                assert_eq!(spec.image_path.as_deref(), Some("/img/fedora.qcow2"));
                assert_eq!(spec.network_or_default(), "default");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(a.targets("node-a"));
        assert!(!a.targets("node-b"));
    }

    #[test]
    fn parse_optional_fields_default() {
        let body =
            r#"{"op":"create","host":"n","spec":{"name":"d","vcpus":1,"ram_mb":512,"disk_gb":10}}"#;
        let a = parse_action(body).expect("parse");
        let LifecycleAction::Create { spec, .. } = a else {
            panic!("expected create");
        };
        assert!(spec.image_path.is_none());
        assert_eq!(spec.network_or_default(), "default"); // absent ⇒ default
    }

    #[test]
    fn parse_stop_and_destroy_flags() {
        let stop = parse_action(r#"{"op":"stop","host":"n","name":"web1","force":true}"#).unwrap();
        assert_eq!(
            stop,
            LifecycleAction::Stop {
                host: "n".into(),
                name: "web1".into(),
                force: true
            }
        );
        // force defaults to false.
        let graceful = parse_action(r#"{"op":"stop","host":"n","name":"web1"}"#).unwrap();
        assert_eq!(
            graceful,
            LifecycleAction::Stop {
                host: "n".into(),
                name: "web1".into(),
                force: false
            }
        );
        let destroy =
            parse_action(r#"{"op":"destroy","host":"n","name":"web1","remove_storage":true}"#)
                .unwrap();
        assert_eq!(
            destroy,
            LifecycleAction::Destroy {
                host: "n".into(),
                name: "web1".into(),
                remove_storage: true
            }
        );
    }

    #[test]
    fn parse_pause_and_resume_actions() {
        assert_eq!(
            parse_action(r#"{"op":"pause","host":"n","name":"web1"}"#).unwrap(),
            LifecycleAction::Pause {
                host: "n".into(),
                name: "web1".into(),
            }
        );
        assert_eq!(
            parse_action(r#"{"op":"resume","host":"n","name":"web1"}"#).unwrap(),
            LifecycleAction::Resume {
                host: "n".into(),
                name: "web1".into(),
            }
        );
    }

    #[test]
    fn parse_attach_and_detach_usb_actions() {
        let attach = parse_action(
            r#"{"op":"attach_usb","host":"n","name":"web1","vendor":"0x0781","product":"0x5567"}"#,
        )
        .unwrap();
        assert_eq!(
            attach,
            LifecycleAction::AttachUsb {
                host: "n".into(),
                name: "web1".into(),
                vendor: "0x0781".into(),
                product: "0x5567".into(),
            }
        );
        assert!(attach.targets("n"));

        let detach = parse_action(
            r#"{"op":"detach_usb","host":"n","name":"web1","vendor":"0x0781","product":"0x5567"}"#,
        )
        .unwrap();
        assert_eq!(
            detach,
            LifecycleAction::DetachUsb {
                host: "n".into(),
                name: "web1".into(),
                vendor: "0x0781".into(),
                product: "0x5567".into(),
            }
        );
    }

    #[test]
    fn parse_refresh_and_reject_malformed() {
        assert_eq!(
            parse_action(r#"{"op":"refresh","host":"n"}"#).unwrap(),
            LifecycleAction::Refresh { host: "n".into() }
        );
        assert!(parse_action("nope").is_err());
        assert!(parse_action(r#"{"op":"teleport","host":"n"}"#).is_err());
    }

    #[test]
    fn empty_host_never_targets() {
        // Fail-safe: an unaddressed create must not fan out to every node.
        let a = LifecycleAction::Refresh {
            host: String::new(),
        };
        assert!(!a.targets("node-a"));
        assert!(!a.targets(""));
    }

    // ── argv builders ──

    #[test]
    fn virsh_argv_shapes() {
        assert_eq!(build_list_argv(), vec!["list", "--all"]);
        assert_eq!(build_dominfo_argv("web1"), vec!["dominfo", "web1"]);
        assert_eq!(build_define_argv("/t/d.xml"), vec!["define", "/t/d.xml"]);
        assert_eq!(build_start_argv("web1"), vec!["start", "web1"]);
        assert_eq!(build_suspend_argv("web1"), vec!["suspend", "web1"]);
        assert_eq!(build_resume_argv("web1"), vec!["resume", "web1"]);
        // libvirt `destroy` is a FORCE-OFF, not a delete.
        assert_eq!(build_stop_argv("web1", true), vec!["destroy", "web1"]);
        assert_eq!(build_stop_argv("web1", false), vec!["shutdown", "web1"]);
        assert_eq!(build_force_off_argv("web1"), vec!["destroy", "web1"]);
        assert_eq!(build_undefine_argv("web1", false), vec!["undefine", "web1"]);
        assert_eq!(
            build_undefine_argv("web1", true),
            vec!["undefine", "web1", "--remove-all-storage"]
        );
    }

    #[test]
    fn pool_and_net_argv_shapes() {
        assert_eq!(build_pool_list_argv(), vec!["pool-list", "--all", "--name"]);
        assert_eq!(
            build_pool_define_argv("mde-vms", "/var/lib/mde-vms"),
            vec![
                "pool-define-as",
                "mde-vms",
                "dir",
                "-",
                "-",
                "-",
                "-",
                "/var/lib/mde-vms",
            ]
        );
        assert_eq!(
            build_pool_start_argv("mde-vms"),
            vec!["pool-start", "mde-vms"]
        );
        assert_eq!(
            build_pool_autostart_argv("mde-vms"),
            vec!["pool-autostart", "mde-vms"]
        );
        assert_eq!(build_net_list_argv(), vec!["net-list", "--all"]);
        assert_eq!(
            build_net_start_argv("default"),
            vec!["net-start", "default"]
        );
        assert_eq!(
            build_net_autostart_argv("default"),
            vec!["net-autostart", "default"]
        );
    }

    #[test]
    fn qemu_img_overlay_vs_blank() {
        // Create-from-image ⇒ a COW overlay backed by the golden.
        assert_eq!(
            build_qemu_img_argv(Some("/img/base.qcow2"), "/pool/web1.qcow2", 40),
            vec![
                "create",
                "-f",
                "qcow2",
                "-b",
                "/img/base.qcow2",
                "-F",
                "qcow2",
                "/pool/web1.qcow2",
                "40G",
            ]
        );
        // No image ⇒ a blank qcow2 of disk_gb.
        assert_eq!(
            build_qemu_img_argv(None, "/pool/web1.qcow2", 20),
            vec!["create", "-f", "qcow2", "/pool/web1.qcow2", "20G"]
        );
        // disk_gb 0 omits the size (an overlay inherits the base's).
        assert_eq!(
            build_qemu_img_argv(Some("/b.qcow2"), "/p/d.qcow2", 0),
            vec![
                "create",
                "-f",
                "qcow2",
                "-b",
                "/b.qcow2",
                "-F",
                "qcow2",
                "/p/d.qcow2"
            ]
        );
    }

    // ── domain XML ──

    #[test]
    fn domain_xml_carries_the_spec() {
        let spec = VmSpec {
            name: "web1".into(),
            vcpus: 4,
            ram_mb: 4096,
            disk_gb: 20,
            image_path: None,
            network: Some("mesh-net".into()),
            pci_passthrough: Vec::new(),
        };
        let xml = build_domain_xml(&spec, "/var/lib/mde-vms/web1.qcow2");
        assert!(xml.starts_with("<domain type='kvm'>"));
        assert!(xml.contains("<name>web1</name>"));
        assert!(xml.contains("<memory unit='MiB'>4096</memory>"));
        assert!(xml.contains("<vcpu placement='static'>4</vcpu>"));
        assert!(xml.contains("machine='q35'"));
        assert!(xml.contains("<source file='/var/lib/mde-vms/web1.qcow2'/>"));
        assert!(xml.contains("type='qcow2'"));
        assert!(xml.contains("<source network='mesh-net'/>"));
        assert!(xml.contains("org.qemu.guest_agent.0"));
        assert!(xml.trim_end().ends_with("</domain>"));
    }

    #[test]
    fn domain_xml_includes_virtio_gpu_accel3d() {
        // QC-23 Tier 0 (docs/design/qc23-virtio-gpu-zerocopy-rescope.md §5):
        // every VM's virtio video model carries 3D acceleration, unconditionally
        // — no opt-in flag, same posture as the E12-9 audio device above.
        let spec = VmSpec {
            name: "web1".into(),
            vcpus: 2,
            ram_mb: 2048,
            disk_gb: 20,
            image_path: None,
            network: None,
            pci_passthrough: Vec::new(),
        };
        let xml = build_domain_xml(&spec, "/var/lib/mde-vms/web1.qcow2");
        assert!(xml.contains("<model type='virtio'>"));
        assert!(xml.contains("<acceleration accel3d='yes'/>"));
        // The accelerated model is nested inside <video>...</video>, which itself
        // lands inside <devices>...</devices> (not floating loose or duplicated
        // outside the device list).
        let video_start = xml.find("<video>").expect("video open");
        let video_end = xml.find("</video>").expect("video close");
        let accel_pos = xml.find("<acceleration").expect("acceleration present");
        assert!(accel_pos > video_start && accel_pos < video_end);
        assert_eq!(xml.matches("<acceleration").count(), 1);
        // The existing SPICE graphics stanza is unchanged alongside it (§3.2:
        // accel3d is compatible with SPICE, not a replacement delivery mechanism).
        assert!(xml.contains("<graphics type='spice' autoport='yes'>"));
    }

    #[test]
    fn domain_xml_includes_pipewire_audio_device() {
        // E12-9 Option A (docs/design/e12-9-10-libvirt-rescope.md): every VM
        // gets a virtio sound card + QEMU's native PipeWire audiodev, so the
        // domain XML always carries this — no opt-in flag.
        let spec = VmSpec {
            name: "web1".into(),
            vcpus: 2,
            ram_mb: 2048,
            disk_gb: 20,
            image_path: None,
            network: None,
            pci_passthrough: Vec::new(),
        };
        let xml = build_domain_xml(&spec, "/var/lib/mde-vms/web1.qcow2");
        assert!(xml.contains("<sound model='virtio'/>"));
        assert!(xml.contains("<audio id='1' type='pipewire'>"));
        // The output's streamName carries the VM name (the doc-recommended
        // connecting tissue toward the E12-16 mixer's mde.vm.name classifier
        // — see build_domain_xml's doc comment for why the mixer-side match
        // isn't wired in this slice).
        assert!(xml.contains("streamName='vm-web1'"));
        assert!(xml.contains("</audio>"));
        // The audio block lands inside <devices>...</devices>.
        let devices_start = xml.find("<devices>").expect("devices open");
        let devices_end = xml.find("</devices>").expect("devices close");
        let audio_pos = xml.find("<audio").expect("audio present");
        assert!(audio_pos > devices_start && audio_pos < devices_end);
    }

    #[test]
    fn domain_xml_includes_pci_passthrough_hostdevs_when_configured() {
        // E12-10 VFIO: pure XML construction only (see build_pci_hostdev_xml's
        // doc comment) — an empty pci_passthrough emits none; a populated one
        // emits one <hostdev> per configured address.
        let spec = VmSpec {
            name: "gpu1".into(),
            vcpus: 8,
            ram_mb: 16384,
            disk_gb: 100,
            image_path: None,
            network: None,
            pci_passthrough: vec![
                PciAddress {
                    domain: "0x0000".into(),
                    bus: "0x01".into(),
                    slot: "0x00".into(),
                    function: "0x0".into(),
                },
                PciAddress {
                    domain: "0x0000".into(),
                    bus: "0x01".into(),
                    slot: "0x00".into(),
                    function: "0x1".into(),
                },
            ],
        };
        let xml = build_domain_xml(&spec, "/var/lib/mde-vms/gpu1.qcow2");
        assert_eq!(
            xml.matches("<hostdev mode='subsystem' type='pci'").count(),
            2
        );
        assert!(xml.contains("function='0x0'"));
        assert!(xml.contains("function='0x1'"));
        let devices_start = xml.find("<devices>").expect("devices open");
        let devices_end = xml.find("</devices>").expect("devices close");
        let hostdev_pos = xml.find("<hostdev").expect("hostdev present");
        assert!(hostdev_pos > devices_start && hostdev_pos < devices_end);
    }

    #[test]
    fn domain_xml_omits_hostdev_when_pci_passthrough_is_empty() {
        let spec = VmSpec {
            name: "plain1".into(),
            vcpus: 2,
            ram_mb: 2048,
            disk_gb: 20,
            image_path: None,
            network: None,
            pci_passthrough: Vec::new(),
        };
        let xml = build_domain_xml(&spec, "/var/lib/mde-vms/plain1.qcow2");
        assert!(!xml.contains("<hostdev"));
    }

    #[test]
    fn domain_xml_escapes_interpolated_values() {
        let spec = VmSpec {
            name: "a&b<c>".into(),
            vcpus: 1,
            ram_mb: 512,
            disk_gb: 10,
            image_path: None,
            network: None,
            pci_passthrough: Vec::new(),
        };
        let xml = build_domain_xml(&spec, "/p/x'y\".qcow2");
        assert!(xml.contains("<name>a&amp;b&lt;c&gt;</name>"));
        assert!(xml.contains("&apos;"));
        assert!(xml.contains("&quot;"));
        // No raw unescaped angle bracket from the name leaked into the body.
        assert!(!xml.contains("a&b<c>"));
        // Absent network ⇒ the default.
        assert!(xml.contains("<source network='default'/>"));
        // The escaped name also drives the audio streamName (E12-9).
        assert!(xml.contains("streamName='vm-a&amp;b&lt;c&gt;'"));
    }

    // ── E12-10 hostdev XML fragments (USB + PCI) ──

    #[test]
    fn usb_hostdev_xml_carries_vendor_and_product() {
        let xml = build_usb_hostdev_xml("0x0781", "0x5567");
        assert!(xml.starts_with("<hostdev mode='subsystem' type='usb'>"));
        assert!(xml.contains("<vendor id='0x0781'/>"));
        assert!(xml.contains("<product id='0x5567'/>"));
        assert!(xml.trim_end().ends_with("</hostdev>"));
    }

    #[test]
    fn usb_hostdev_xml_escapes_interpolated_values() {
        let xml = build_usb_hostdev_xml("0x0781\"", "<evil>");
        assert!(xml.contains("&quot;"));
        assert!(xml.contains("&lt;evil&gt;"));
        assert!(!xml.contains("<evil>"));
    }

    #[test]
    fn pci_hostdev_xml_carries_the_address() {
        let addr = PciAddress {
            domain: "0x0000".into(),
            bus: "0x01".into(),
            slot: "0x00".into(),
            function: "0x0".into(),
        };
        let xml = build_pci_hostdev_xml(&addr);
        assert!(xml.starts_with("<hostdev mode='subsystem' type='pci' managed='yes'>"));
        assert!(xml.contains("domain='0x0000'"));
        assert!(xml.contains("bus='0x01'"));
        assert!(xml.contains("slot='0x00'"));
        assert!(xml.contains("function='0x0'"));
        assert!(xml.trim_end().ends_with("</hostdev>"));
    }

    // ── E12-10 attach/detach-device argv ──

    #[test]
    fn attach_detach_device_argv_shapes() {
        assert_eq!(
            build_attach_device_argv("web1", "/tmp/dev.xml"),
            vec!["attach-device", "web1", "/tmp/dev.xml", "--live"]
        );
        assert_eq!(
            build_detach_device_argv("web1", "/tmp/dev.xml"),
            vec!["detach-device", "web1", "/tmp/dev.xml", "--live"]
        );
    }

    // ── parsers ──

    #[test]
    fn parse_list_multi_vm_and_multiword_state() {
        let raw = " Id   Name       State\n\
                    -----------------------------\n\
                    \x20 1    web1       running\n\
                    \x20 -    db         shut off\n\
                    \x20 -    build      shut off\n";
        let v = parse_virsh_list(raw);
        assert_eq!(v.len(), 3);
        assert_eq!(
            v[0],
            Instance {
                id: "1".into(),
                name: "web1".into(),
                state: "running".into()
            }
        );
        assert_eq!(v[1].name, "db");
        assert_eq!(v[1].id, "-");
        assert_eq!(v[1].state, "shut off"); // multi-word state preserved
        assert_eq!(v[2].name, "build");
    }

    #[test]
    fn parse_list_empty_when_no_domains() {
        // virsh prints just the header + separator for an empty host.
        let raw = " Id   Name   State\n----------------------\n";
        assert!(parse_virsh_list(raw).is_empty());
        assert!(parse_virsh_list("").is_empty());
    }

    fn dominfo_running() -> &'static str {
        "Id:             1\nName:           web1\nUUID:           abc-123\nOS Type:        hvm\nState:          running\nCPU(s):         2\nCPU time:       12.3s\nMax memory:     2097152 KiB\nUsed memory:    2097152 KiB\nPersistent:     yes\n"
    }

    #[test]
    fn parse_dominfo_fields() {
        let d = parse_virsh_dominfo(dominfo_running()).expect("parse");
        assert_eq!(d.name, "web1");
        assert_eq!(d.uuid, "abc-123");
        assert_eq!(d.state, "running");
        assert_eq!(d.vcpus, 2);
        assert_eq!(d.max_mem_mib, 2048); // 2_097_152 KiB / 1024
        assert_eq!(d.state_kind(), VmState::Running);
    }

    #[test]
    fn parse_dominfo_none_when_incomplete() {
        assert!(parse_virsh_dominfo("Id: 1\n").is_none());
        assert!(parse_virsh_dominfo("error: failed to get domain 'nope'\n").is_none());
    }

    #[test]
    fn pool_and_net_predicates() {
        assert!(pool_exists("default\nmde-vms\nimages\n", "mde-vms"));
        assert!(!pool_exists("default\nimages\n", "mde-vms"));
        let netlist = " Name      State    Autostart   Persistent\n\
                        ----------------------------------------------\n\
                        \x20default   active   yes         yes\n\
                        \x20isolated  inactive no          yes\n";
        assert!(net_is_active(netlist, "default"));
        assert!(!net_is_active(netlist, "isolated"));
        assert!(!net_is_active(netlist, "nonexistent"));
    }

    #[test]
    fn vm_state_mapping_and_roundtrip() {
        assert_eq!(vm_state_from_str("running"), VmState::Running);
        assert_eq!(vm_state_from_str("shut off"), VmState::ShutOff);
        assert_eq!(vm_state_from_str("paused"), VmState::Paused);
        assert_eq!(vm_state_from_str("crashed"), VmState::Crashed);
        assert_eq!(vm_state_from_str("in shutdown"), VmState::Other);
        // The well-known states round-trip through the canonical string.
        for s in [
            VmState::Running,
            VmState::Paused,
            VmState::ShutOff,
            VmState::Crashed,
        ] {
            assert_eq!(vm_state_from_str(s.as_virsh_str()), s);
        }
    }

    // ── state machine ──

    #[test]
    fn plan_transition_create() {
        assert_eq!(
            plan_transition(None, LifecycleOp::Create),
            Ok(Transition::Defined)
        );
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::Create),
            Err(TransitionError::AlreadyExists)
        );
    }

    #[test]
    fn plan_transition_start() {
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::Start),
            Ok(Transition::Started)
        );
        assert_eq!(
            plan_transition(Some(VmState::Crashed), LifecycleOp::Start),
            Ok(Transition::Started)
        );
        assert_eq!(
            plan_transition(Some(VmState::Running), LifecycleOp::Start),
            Err(TransitionError::AlreadyRunning)
        );
        assert_eq!(
            plan_transition(None, LifecycleOp::Start),
            Err(TransitionError::NotFound)
        );
    }

    #[test]
    fn plan_transition_stop() {
        assert_eq!(
            plan_transition(Some(VmState::Running), LifecycleOp::Stop),
            Ok(Transition::Stopped)
        );
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::Stop),
            Err(TransitionError::NotRunning)
        );
        assert_eq!(
            plan_transition(None, LifecycleOp::Stop),
            Err(TransitionError::NotFound)
        );
    }

    #[test]
    fn plan_transition_pause_resume() {
        // Pause: only a running VM freezes.
        assert_eq!(
            plan_transition(Some(VmState::Running), LifecycleOp::Pause),
            Ok(Transition::Paused)
        );
        assert_eq!(
            plan_transition(Some(VmState::Paused), LifecycleOp::Pause),
            Err(TransitionError::AlreadyPaused)
        );
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::Pause),
            Err(TransitionError::Invalid {
                op: LifecycleOp::Pause,
                state: VmState::ShutOff,
            })
        );
        assert_eq!(
            plan_transition(None, LifecycleOp::Pause),
            Err(TransitionError::NotFound)
        );

        // Resume: only a paused VM wakes.
        assert_eq!(
            plan_transition(Some(VmState::Paused), LifecycleOp::Resume),
            Ok(Transition::Resumed)
        );
        assert_eq!(
            plan_transition(Some(VmState::Running), LifecycleOp::Resume),
            Err(TransitionError::NotPaused),
            "a running VM isn't paused"
        );
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::Resume),
            Err(TransitionError::NotPaused)
        );
        assert_eq!(
            plan_transition(None, LifecycleOp::Resume),
            Err(TransitionError::NotFound)
        );
    }

    #[test]
    fn plan_transition_attach_detach_usb() {
        // Hot-attach/detach (E12-10) only act on a running domain — mirrors
        // Stop's own "not running" refusal for a shut-off VM.
        assert_eq!(
            plan_transition(Some(VmState::Running), LifecycleOp::AttachUsb),
            Ok(Transition::UsbAttached)
        );
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::AttachUsb),
            Err(TransitionError::NotRunning)
        );
        assert_eq!(
            plan_transition(None, LifecycleOp::AttachUsb),
            Err(TransitionError::NotFound)
        );
        assert_eq!(
            plan_transition(Some(VmState::Paused), LifecycleOp::AttachUsb),
            Err(TransitionError::Invalid {
                op: LifecycleOp::AttachUsb,
                state: VmState::Paused,
            })
        );

        assert_eq!(
            plan_transition(Some(VmState::Running), LifecycleOp::DetachUsb),
            Ok(Transition::UsbDetached)
        );
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::DetachUsb),
            Err(TransitionError::NotRunning)
        );
        assert_eq!(
            plan_transition(None, LifecycleOp::DetachUsb),
            Err(TransitionError::NotFound)
        );
    }

    #[test]
    fn plan_transition_destroy() {
        assert_eq!(
            plan_transition(Some(VmState::Running), LifecycleOp::Destroy),
            Ok(Transition::Removed)
        );
        assert_eq!(
            plan_transition(Some(VmState::ShutOff), LifecycleOp::Destroy),
            Ok(Transition::Removed)
        );
        assert_eq!(
            plan_transition(None, LifecycleOp::Destroy),
            Err(TransitionError::NotFound)
        );
    }

    // ── FakeLibvirt-driven apply_action (no live virsh) ──

    /// An in-memory [`LibvirtBackend`] recording calls + domain states, so
    /// [`apply_action`] is exercised end-to-end without KVM.
    struct FakeLibvirt {
        domains: Mutex<BTreeMap<String, VmState>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeLibvirt {
        fn new() -> Self {
            Self {
                domains: Mutex::new(BTreeMap::new()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn with_domain(name: &str, state: VmState) -> Self {
            let f = Self::new();
            f.domains.lock().unwrap().insert(name.to_string(), state);
            f
        }
        fn record(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_string());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn state_of(&self, name: &str) -> Option<VmState> {
            self.domains.lock().unwrap().get(name).copied()
        }
    }

    impl LibvirtBackend for FakeLibvirt {
        fn ensure_default_pool(&self) -> Result<(), LibvirtError> {
            self.record("ensure_default_pool");
            Ok(())
        }
        fn ensure_default_network(&self) -> Result<(), LibvirtError> {
            self.record("ensure_default_network");
            Ok(())
        }
        fn create(&self, spec: &VmSpec) -> Result<(), LibvirtError> {
            self.record(&format!("create:{}", spec.name));
            self.domains
                .lock()
                .unwrap()
                .insert(spec.name.clone(), VmState::ShutOff);
            Ok(())
        }
        fn start(&self, name: &str) -> Result<(), LibvirtError> {
            self.record(&format!("start:{name}"));
            self.domains
                .lock()
                .unwrap()
                .insert(name.to_string(), VmState::Running);
            Ok(())
        }
        fn pause(&self, name: &str) -> Result<(), LibvirtError> {
            self.record(&format!("pause:{name}"));
            self.domains
                .lock()
                .unwrap()
                .insert(name.to_string(), VmState::Paused);
            Ok(())
        }
        fn resume(&self, name: &str) -> Result<(), LibvirtError> {
            self.record(&format!("resume:{name}"));
            self.domains
                .lock()
                .unwrap()
                .insert(name.to_string(), VmState::Running);
            Ok(())
        }
        fn stop(&self, name: &str, force: bool) -> Result<(), LibvirtError> {
            self.record(&format!("stop:{name}:force={force}"));
            self.domains
                .lock()
                .unwrap()
                .insert(name.to_string(), VmState::ShutOff);
            Ok(())
        }
        fn destroy(&self, name: &str, remove_storage: bool) -> Result<(), LibvirtError> {
            self.record(&format!("destroy:{name}:rm={remove_storage}"));
            self.domains.lock().unwrap().remove(name);
            Ok(())
        }
        fn attach_usb(&self, name: &str, vendor: &str, product: &str) -> Result<(), LibvirtError> {
            self.record(&format!("attach_usb:{name}:{vendor}:{product}"));
            // Hot-attach doesn't change the domain's coarse power state.
            Ok(())
        }
        fn detach_usb(&self, name: &str, vendor: &str, product: &str) -> Result<(), LibvirtError> {
            self.record(&format!("detach_usb:{name}:{vendor}:{product}"));
            Ok(())
        }
        fn list(&self) -> Result<Vec<Instance>, LibvirtError> {
            Ok(self
                .domains
                .lock()
                .unwrap()
                .iter()
                .map(|(n, s)| Instance {
                    id: "-".into(),
                    name: n.clone(),
                    state: s.as_virsh_str().into(),
                })
                .collect())
        }
        fn info(&self, name: &str) -> Result<Option<DomainInfo>, LibvirtError> {
            Ok(self.state_of(name).map(|s| DomainInfo {
                name: name.to_string(),
                state: s.as_virsh_str().into(),
                ..DomainInfo::default()
            }))
        }
    }

    fn create_action(name: &str) -> LifecycleAction {
        LifecycleAction::Create {
            host: "node-a".into(),
            spec: VmSpec {
                name: name.into(),
                vcpus: 2,
                ram_mb: 2048,
                disk_gb: 20,
                image_path: Some("/img/base.qcow2".into()),
                network: None,
                pci_passthrough: Vec::new(),
            },
        }
    }

    #[test]
    fn apply_create_ensures_pool_net_then_defines() {
        let fake = FakeLibvirt::new();
        apply_action(&fake, &create_action("web1")).expect("create ok");
        // Pool + network ensured before the define, in order.
        assert_eq!(
            fake.calls(),
            vec![
                "ensure_default_pool".to_string(),
                "ensure_default_network".to_string(),
                "create:web1".to_string(),
            ]
        );
        assert_eq!(fake.state_of("web1"), Some(VmState::ShutOff));
    }

    #[test]
    fn apply_double_create_is_rejected_by_the_state_machine() {
        let fake = FakeLibvirt::with_domain("web1", VmState::ShutOff);
        let err = apply_action(&fake, &create_action("web1")).expect_err("already exists");
        assert!(err.contains("already exists"), "{err}");
        // The backend create was NOT called again (guarded before the seam).
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn apply_start_stop_destroy_lifecycle() {
        let fake = FakeLibvirt::with_domain("web1", VmState::ShutOff);
        let start = LifecycleAction::Start {
            host: "node-a".into(),
            name: "web1".into(),
        };
        apply_action(&fake, &start).expect("start ok");
        assert_eq!(fake.state_of("web1"), Some(VmState::Running));

        // Force stop passes the flag through to the backend.
        let stop = LifecycleAction::Stop {
            host: "node-a".into(),
            name: "web1".into(),
            force: true,
        };
        apply_action(&fake, &stop).expect("stop ok");
        assert_eq!(fake.state_of("web1"), Some(VmState::ShutOff));
        assert!(fake.calls().iter().any(|c| c == "stop:web1:force=true"));

        let destroy = LifecycleAction::Destroy {
            host: "node-a".into(),
            name: "web1".into(),
            remove_storage: true,
        };
        apply_action(&fake, &destroy).expect("destroy ok");
        assert_eq!(fake.state_of("web1"), None); // removed
        assert!(fake.calls().iter().any(|c| c == "destroy:web1:rm=true"));
    }

    #[test]
    fn apply_pause_resume_lifecycle() {
        // A running VM pauses → suspended; resume wakes it back to running. Each
        // step drives the injected backend and advances the recorded state.
        let fake = FakeLibvirt::with_domain("web1", VmState::Running);
        let pause = LifecycleAction::Pause {
            host: "node-a".into(),
            name: "web1".into(),
        };
        apply_action(&fake, &pause).expect("pause ok");
        assert_eq!(fake.state_of("web1"), Some(VmState::Paused));
        assert!(fake.calls().iter().any(|c| c == "pause:web1"));

        let resume = LifecycleAction::Resume {
            host: "node-a".into(),
            name: "web1".into(),
        };
        apply_action(&fake, &resume).expect("resume ok");
        assert_eq!(fake.state_of("web1"), Some(VmState::Running));
        assert!(fake.calls().iter().any(|c| c == "resume:web1"));

        // Pausing a shut-off VM is rejected by the state machine (no backend call).
        let off = FakeLibvirt::with_domain("db1", VmState::ShutOff);
        let err = apply_action(
            &off,
            &LifecycleAction::Pause {
                host: "node-a".into(),
                name: "db1".into(),
            },
        )
        .expect_err("cannot pause a shut-off VM");
        assert!(err.contains("cannot"), "{err}");
        assert!(off.calls().is_empty());

        // Resuming a VM that isn't paused is rejected honestly.
        let running = FakeLibvirt::with_domain("web2", VmState::Running);
        let err = apply_action(
            &running,
            &LifecycleAction::Resume {
                host: "node-a".into(),
                name: "web2".into(),
            },
        )
        .expect_err("cannot resume a running VM");
        assert!(err.contains("not paused"), "{err}");
    }

    #[test]
    fn apply_attach_detach_usb_lifecycle() {
        // A running VM accepts hot-attach/detach; the backend call carries
        // the vendor/product id through, and the VM's coarse state (unlike
        // pause/resume) is unchanged.
        let fake = FakeLibvirt::with_domain("web1", VmState::Running);
        let attach = LifecycleAction::AttachUsb {
            host: "node-a".into(),
            name: "web1".into(),
            vendor: "0x0781".into(),
            product: "0x5567".into(),
        };
        apply_action(&fake, &attach).expect("attach ok");
        assert_eq!(fake.state_of("web1"), Some(VmState::Running));
        assert!(fake
            .calls()
            .iter()
            .any(|c| c == "attach_usb:web1:0x0781:0x5567"));

        let detach = LifecycleAction::DetachUsb {
            host: "node-a".into(),
            name: "web1".into(),
            vendor: "0x0781".into(),
            product: "0x5567".into(),
        };
        apply_action(&fake, &detach).expect("detach ok");
        assert_eq!(fake.state_of("web1"), Some(VmState::Running));
        assert!(fake
            .calls()
            .iter()
            .any(|c| c == "detach_usb:web1:0x0781:0x5567"));

        // A shut-off VM refuses hot-attach honestly (no backend call) — mirrors
        // how Pause refuses a shut-off VM above.
        let off = FakeLibvirt::with_domain("db1", VmState::ShutOff);
        let err = apply_action(
            &off,
            &LifecycleAction::AttachUsb {
                host: "node-a".into(),
                name: "db1".into(),
                vendor: "0x0781".into(),
                product: "0x5567".into(),
            },
        )
        .expect_err("cannot hot-attach to a shut-off VM");
        assert!(err.contains("not running"), "{err}");
        assert!(off.calls().is_empty());
    }

    #[test]
    fn apply_start_on_running_is_rejected() {
        let fake = FakeLibvirt::with_domain("web1", VmState::Running);
        let start = LifecycleAction::Start {
            host: "node-a".into(),
            name: "web1".into(),
        };
        let err = apply_action(&fake, &start).expect_err("already running");
        assert!(err.contains("already running"), "{err}");
    }

    #[test]
    fn apply_start_missing_is_not_found() {
        let fake = FakeLibvirt::new();
        let start = LifecycleAction::Start {
            host: "node-a".into(),
            name: "ghost".into(),
        };
        let err = apply_action(&fake, &start).expect_err("not found");
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn apply_refresh_is_a_noop() {
        let fake = FakeLibvirt::new();
        apply_action(
            &fake,
            &LifecycleAction::Refresh {
                host: "node-a".into(),
            },
        )
        .expect("noop");
        assert!(fake.calls().is_empty());
    }

    // ── report + topics ──

    #[test]
    fn instance_report_round_trips_json() {
        let report = InstanceReport {
            host: "node-a".into(),
            instances: vec![Instance {
                id: "1".into(),
                name: "web1".into(),
                state: "running".into(),
            }],
            published_at_ms: 42,
        };
        let json = serde_json::to_string(&report).expect("serialize");
        let back: InstanceReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, report);
    }

    #[test]
    fn topics_are_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/vm/lifecycle");
        assert_eq!(INSTANCES_TOPIC, "event/vm/instances");
        assert!(ACTION_TOPIC.starts_with("action/"));
        assert!(INSTANCES_TOPIC.starts_with("event/"));
    }

    #[test]
    fn worker_name_matches_module() {
        let w = VmLifecycleWorker::new("node".to_string());
        assert_eq!(w.name(), "vm_lifecycle");
    }

    #[tokio::test]
    async fn tick_loop_exits_on_shutdown() {
        // Drives run() with a FakeLibvirt + a temp bus root (no live virsh, no
        // real bus actions) and exits promptly on shutdown.
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = VmLifecycleWorker::new("node".to_string())
            .with_backend(Arc::new(FakeLibvirt::new()))
            .with_bus_root(dir.path().to_path_buf())
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
