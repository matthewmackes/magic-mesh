//! DATACENTER (action layer) — `action/dc/vm-power` + `action/dc/vm-snapshot`
//! + `action/dc/vm-clone` + `action/dc/vm-delete` → Xen VM control.
//!
//! The action side of the DATACENTER epic: the worker
//! ([`crate::workers::datacenter_orchestrator`]) PUBLISHES VM state; this
//! responder lets the Workbench plane ACT on it. Same dedicated-OS-thread,
//! `action/<domain>/<verb>` Bus-RPC shape as the route-trace responder
//! ([`crate::ipc::route`]) — the reads/exec are synchronous SSH calls.
//!
//! `vm-power` request body `{ "uuid", "op": "start"|"shutdown"|"reboot", "dom0" }`:
//!   * `op` maps to an `xe` verb (`start`→`vm-start`, …);
//!   * `uuid` is validated to be hex+`-` only (no command injection);
//!   * `dom0` MUST be in the configured allowed set
//!     ([`crate::workers::datacenter_orchestrator::xen_dom0s`]) before any SSH.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-snapshot` request body `{ "uuid", "dom0" }`:
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * snapshots the VM via `xe vm-snapshot uuid=<uuid> new-name-label=…`.
//! Reply `{"ok":true,"snapshot":"<new-snapshot-uuid>"}` on success (the new
//! snapshot uuid `xe` prints on stdout), `{"error":"<message>"}` on failure.
//!
//! `vm-clone` request body `{ "uuid", "dom0", "name"? }`:
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * an optional `name` is sanitized to `[A-Za-z0-9._-]` only; absent, the
//!     clone defaults to name-label `dc-clone-<first 8 chars of uuid>`;
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * clones the VM via `xe vm-clone uuid=<uuid> new-name-label=…`.
//! Reply `{"ok":true,"clone":"<new-vm-uuid>"}` on success (the new uuid `xe`
//! prints on stdout), `{"error":"<message>"}` on failure.
//!
//! `vm-delete` request body `{ "uuid", "dom0", "confirm": true }`:
//!   * `confirm` MUST be the boolean `true` — a destructive guard checked first;
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * deletes the VM via `xe vm-uninstall uuid=<uuid> force=true`.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-console` request body `{ "uuid", "dom0" }` (read-only):
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * reads the XAPI console object's `location` (the connection URL the noVNC
//!     viewer uses) via `xe console-list vm-uuid=<uuid> params=location --minimal`.
//! Reply `{"ok":true,"location":"<console URL>"}` on success; if the VM has no
//! console (halted / not running) the trimmed output is empty →
//! `{"error":"no console (vm not running?)"}`; `{"error":"<message>"}` on failure.
//!
//! `vm-suspend` request body `{ "uuid", "op": "suspend"|"resume", "dom0" }`:
//!   * `op` maps to an `xe` verb (`suspend`→`vm-suspend`, `resume`→`vm-resume`);
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-snapshots` request body `{ "uuid", "dom0" }` (read-only):
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * lists the VM's snapshots via `xe snapshot-list snapshot-of=<uuid> …`,
//!     emitting `<snap-uuid>|<name-label>` per line (the same `$u|$(…)` shell
//!     idiom the orchestrator's VM read uses), parsed back into pairs.
//! Reply `{"ok":true,"snapshots":[{"uuid","name"}, …]}` on success (possibly
//! empty), `{"error":"<message>"}` on failure.
//!
//! `vm-snapshot-revert` request body `{ "snapshot", "dom0" }`:
//!   * `snapshot` (a snapshot uuid) is validated to be hex+`-` only (same guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * reverts the parent VM to the snapshot via
//!     `xe snapshot-revert snapshot-uuid=<snapshot>`.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-snapshot-delete` request body `{ "snapshot", "dom0", "confirm": true }`:
//!   * `confirm` MUST be the boolean `true` — a destructive guard checked first
//!     (mirrors `vm-delete`), since a snapshot-destroy is irreversible;
//!   * `snapshot` (a snapshot uuid) is validated to be hex+`-` only (same guard);
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * destroys the snapshot via `xe snapshot-destroy uuid=<snapshot>`.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-migrate` request body `{ "uuid", "dom0", "host" }`:
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `host` (the destination host name-label or uuid) is validated to
//!     `[A-Za-z0-9._:-]` only — same shell-interpolation guard;
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * live-migrates the VM via `xe vm-migrate uuid=<uuid> host=<host> live=true`.
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-resize` request body `{ "uuid", "dom0", "vcpus", "mem_mib" }`:
//!   * `uuid` is validated to be hex+`-` only (same injection guard);
//!   * `vcpus` (1..=64) and `mem_mib` (1..=1048576) are bounds-checked integers,
//!     so the values interpolated into the `xe` string are always numeric;
//!   * `dom0` MUST be in the configured allowed set before any SSH;
//!   * sets VCPUs (max + at-startup) and the memory limits (static/dynamic) via a
//!     compound `xe` invocation — the VM must be HALTED (XAPI enforces this).
//! Reply `{"ok":true}` on success, `{"error":"<message>"}` on failure.
//!
//! `vm-create` request body `{ "name", "template"?, "vcpus", "mem_mib", "network_uuid", "dom0" }`:
//!   * a STRUCTURAL change → it does NOT touch XAPI directly; it WRITES a
//!     `xenserver_vm` golden-template clone resource into the allow-listed
//!     `infra/tofu/xen-xapi` workspace's generated `dc-vms.tf` (idempotent; a
//!     repeated `name` is rejected so a create never silently overwrites);
//!   * `name` is sanitized to `[A-Za-z0-9._-]`, `template` (default `MDE-VM-golden`)
//!     and `network_uuid` to hex/dot/dash, `vcpus`/`mem_mib` bounds-checked — every
//!     interpolated field is validated before it reaches the HCL;
//!   * `dom0` MUST be in the configured allowed set (the pool the resource targets).
//! Reply `{"ok":true,"resource":"<addr>","path":"<rel tf path>"}` on success — the
//! caller then runs `action/dc/tofu-apply` on `xen-xapi` to materialize it (so the
//! structural change goes through Tofu — no drift). `{"error":"<message>"}` on failure.
//!
//! `do-regions` request body ignored/empty (read-only):
//!   * runs `doctl compute region list --context <ctx> -o json` locally, where
//!     `<ctx>` is `MCNF_DOCTL_CONTEXT` (default `mackes`, the authed context);
//!   * parses the JSON array (each entry `{"slug","name","available"}`).
//! Reply `{"ok":true,"regions":[{"slug","name","available"}, …]}` on success,
//! `{"error":"doctl region list failed"}` if doctl is missing/failed.
//!
//! DATACENTER-19 — `lighthouse-create` request body `{"name","region","size"?,"image"?}`
//! (the guided new-lighthouse flow's Tofu-write half; mirrors `vm-create`):
//!   * `name` is sanitized to `[A-Za-z0-9._-]` (also the resource's `name` + block
//!     label) and `region` to `[a-z0-9-]` (a DO region slug); `size`/`image` default
//!     to the `zone1-do` workspace's `lighthouse_size`/`lighthouse_image` variables
//!     and are validated to slug chars when supplied;
//!   * WRITES a `digitalocean_droplet` resource into the allow-listed `zone1-do`
//!     workspace's generated `dc-lighthouses.tf` (a duplicate `name` is rejected —
//!     a create never silently overwrites an existing block).
//! Reply `{"ok":true,"resource":"<addr>","path":"<rel tf path>"}` on success — the
//! caller then runs `action/dc/tofu-apply` on `zone1-do` to PROVISION the droplet
//! live (the carried live-DO step: `tofu apply` + bootstrap `mackesd found --role
//! lighthouse` + add the DNS record). `{"error":"<message>"}` on failure.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The VM power-control responder.
///
/// Rooted at the shared workgroup root (carried for parity with the other action
/// services; the allowed-dom0 set + ssh key come from the orchestrator's
/// env-driven config).
///
/// DATACENTER-6 (op-lock half): the service also carries an in-flight op-lock —
/// a shared set of the resource keys currently being mutated (the VM `uuid` for
/// the `vm-*` mutating verbs). [`build_reply`] try-inserts the key before
/// dispatching a mutating verb and rejects a second concurrent mutation on the
/// same resource with a clear `busy` reason; a [`OpLockGuard`] removes the key
/// when the op completes (RAII). `Clone` shares the same lock (the spawn in
/// `bin/mackesd.rs` clones the service into the responder thread), so two
/// in-flight requests — even across `Clone`d handles — see one set.
#[derive(Debug, Clone)]
pub struct DatacenterService {
    // The repo root the daemon runs in — `vm-create` resolves the allow-listed
    // `infra/tofu/xen-xapi` workspace under it to write the golden-template clone
    // resource (structural changes go through Tofu — no drift). The allowed-dom0
    // set + ssh key come from the orchestrator's env config.
    workgroup_root: PathBuf,
    /// In-flight resource keys currently being mutated. `Arc<Mutex<…>>` so a
    /// `Clone` of the service (the responder-thread handle) shares ONE set, and
    /// so concurrent `build_reply` calls serialize on insert/remove.
    in_flight: Arc<Mutex<BTreeSet<String>>>,
}

impl DatacenterService {
    /// Build the service rooted at the shared workgroup root, with an empty
    /// in-flight op-lock set.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            in_flight: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// Try to claim `key` in the in-flight set. Returns a [`OpLockGuard`] (which
    /// releases the key on drop) when the key was free, or `None` when a mutation
    /// on the same resource is already in flight — the caller turns that into the
    /// `busy` reject. A poisoned lock is recovered (the set is plain data; a panic
    /// mid-mutation cannot leave it inconsistent), so the op-lock never wedges the
    /// responder.
    #[must_use]
    fn try_lock(&self, key: String) -> Option<OpLockGuard<'_>> {
        let mut set = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if set.insert(key.clone()) {
            Some(OpLockGuard {
                in_flight: &self.in_flight,
                key,
            })
        } else {
            None
        }
    }
}

/// RAII release for one claimed in-flight resource key: dropping it removes the
/// key from the service's in-flight set, so a panic or early return in
/// [`build_reply`] still frees the lock (the resource never gets stuck `busy`).
struct OpLockGuard<'a> {
    in_flight: &'a Arc<Mutex<BTreeSet<String>>>,
    key: String,
}

impl Drop for OpLockGuard<'_> {
    fn drop(&mut self) {
        let mut set = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set.remove(&self.key);
    }
}

/// Action verbs served on `action/dc/<verb>`.
///
/// DATACENTER-12: the trailing five are the storage verbs ([`crate::ipc::storage_ops`]),
/// served on THIS responder so the panel's Storage tab acts through the same Bus
/// round trip as the VM tab. `build_reply` routes them into `storage_ops`.
pub const ACTION_VERBS: [&str; 22] = [
    "vm-power",
    "vm-snapshot",
    "vm-clone",
    "vm-delete",
    "vm-console",
    "vm-suspend",
    "vm-migrate",
    "vm-resize",
    "vm-create",
    "vm-snapshots",
    "vm-snapshot-revert",
    "vm-snapshot-delete",
    "do-regions",
    // DATACENTER-19 — the guided new-lighthouse flow's Tofu-write half: writes a
    // `digitalocean_droplet` into the `zone1-do` workspace (mirrors `vm-create`).
    "lighthouse-create",
    // DATACENTER-18 — New-Mesh genesis: plan (read-only) + write (the founding
    // lighthouse droplet + its DNS A-record). Reuses DC-19's lighthouse Tofu-write;
    // the live apply + `mackesd found` stay operator-gated.
    "genesis-plan",
    "genesis-write",
    // DAR-45 (DEVOPS-AUTOMATION-REBUILD) — the genesis-wizard backoffice step's
    // read-only planner probe: shells out to `backoffice-plan.sh --tier <t>` and
    // returns the RENDERED ordered unit list + a `secrets_ready` boolean. NOT a
    // canned plan — the acceptance asserts it matches the script's output. The
    // live bring-up (`backoffice-up.sh`) stays operator-gated on the control VM.
    "backoffice-plan",
    "sr-create",
    "vdi-create",
    "vdi-attach",
    "vdi-detach",
    "sr-snapshot",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Build the remote `xe` argument string for a VM power op. PURE.
///
/// Maps `op` → the `xe` verb (`start`→`vm-start`, `shutdown`→`vm-shutdown`,
/// `reboot`→`vm-reboot`; any other `op` is an error) and validates `uuid` is
/// non-empty and contains only `[0-9a-fA-F-]` — this is the command-injection
/// guard, since the result is interpolated into a remote shell `xe …` string.
/// Returns e.g. `"vm-start uuid=<uuid>"`.
///
/// # Errors
/// Returns `Err` for an unknown `op`, an empty `uuid`, or a `uuid` containing any
/// character that is not an ASCII hex digit or `-`.
pub fn vm_power_command(uuid: &str, op: &str) -> Result<String, String> {
    let verb = match op {
        "start" => "vm-start",
        "shutdown" => "vm-shutdown",
        "reboot" => "vm-reboot",
        other => return Err(format!("unknown op: {other}")),
    };
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!("{verb} uuid={uuid}"))
}

/// Build the remote `xe` argument string for a VM snapshot. PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`], since the result is
/// interpolated into a remote shell `xe …` string. The new snapshot is given a
/// stable name-label `dc-snap-<first 8 chars of uuid>`. Returns e.g.
/// `"vm-snapshot uuid=<uuid> new-name-label=dc-snap-abcd1234"`.
///
/// # Errors
/// Returns `Err` for an empty `uuid`, or a `uuid` containing any character that
/// is not an ASCII hex digit or `-`.
pub fn vm_snapshot_command(uuid: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    let short: String = uuid.chars().take(8).collect();
    Ok(format!(
        "vm-snapshot uuid={uuid} new-name-label=dc-snap-{short}"
    ))
}

/// Build the remote `xe` argument string for a VM clone. PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`], since the result is
/// interpolated into a remote shell `xe …` string. The new clone's name-label is
/// either the caller-supplied `name` (sanitized to `[A-Za-z0-9._-]` only — any
/// other character is rejected) or, when absent, the stable default
/// `dc-clone-<first 8 chars of uuid>`. Returns e.g.
/// `"vm-clone uuid=<uuid> new-name-label=dc-clone-abcd1234"`.
///
/// # Errors
/// Returns `Err` for an empty `uuid`, a `uuid` containing any character that is
/// not an ASCII hex digit or `-`, or a supplied `name` that is empty or contains
/// any character outside `[A-Za-z0-9._-]`.
pub fn vm_clone_command(uuid: &str, name: Option<&str>) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    let label = match name {
        Some(n) => {
            if n.is_empty() {
                return Err("empty name".into());
            }
            if !n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err("name contains invalid characters".into());
            }
            n.to_string()
        }
        None => {
            let short: String = uuid.chars().take(8).collect();
            format!("dc-clone-{short}")
        }
    };
    Ok(format!("vm-clone uuid={uuid} new-name-label={label}"))
}

/// Build the remote `xe` argument string for a VM suspend/resume op. PURE.
///
/// Maps `op` → the `xe` verb (`suspend`→`vm-suspend`, `resume`→`vm-resume`; any
/// other `op` is an error) and validates `uuid` is non-empty and contains only
/// `[0-9a-fA-F-]` — the same command-injection guard as [`vm_power_command`],
/// since the result is interpolated into a remote shell `xe …` string. Returns
/// e.g. `"vm-suspend uuid=<uuid>"`.
///
/// # Errors
/// Returns `Err` for an unknown `op`, an empty `uuid`, or a `uuid` containing any
/// character that is not an ASCII hex digit or `-`.
pub fn vm_suspend_command(uuid: &str, op: &str) -> Result<String, String> {
    let verb = match op {
        "suspend" => "vm-suspend",
        "resume" => "vm-resume",
        other => return Err(format!("unknown op: {other}")),
    };
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!("{verb} uuid={uuid}"))
}

/// Build the remote `xe` argument string for a live VM migration. PURE.
///
/// Validates `uuid` is hex+`-` only (the [`vm_power_command`] injection guard) and
/// `host` (the destination host name-label or uuid) contains only
/// `[A-Za-z0-9._:-]` — both are interpolated into a remote shell `xe …` string.
/// Returns `"vm-migrate uuid=<uuid> host=<host> live=true"`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `uuid` or an empty/invalid `host`.
pub fn vm_migrate_command(uuid: &str, host: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    if host.is_empty() {
        return Err("empty host".into());
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
    {
        return Err("host contains invalid characters".into());
    }
    Ok(format!("vm-migrate uuid={uuid} host={host} live=true"))
}

/// The VCPU upper bound a `vm-resize` accepts — a generous ceiling that still keeps
/// the value sane (and bounded for the shell interpolation). PURE constant.
pub const RESIZE_MAX_VCPUS: u32 = 64;

/// The memory upper bound (MiB) a `vm-resize` accepts — 1 TiB, a generous ceiling.
/// PURE constant.
pub const RESIZE_MAX_MEM_MIB: u64 = 1_048_576;

/// Build the remote `xe` argument strings for a VM resize (VCPUs + memory). PURE.
///
/// `vcpus` must be `1..=RESIZE_MAX_VCPUS` and `mem_mib` `1..=RESIZE_MAX_MEM_MIB`,
/// so every value interpolated into the `xe` strings is a bounds-checked integer
/// (there is no string field to injection-guard — that is why the inputs are typed
/// integers, not strings). `uuid` is the [`vm_power_command`] hex+`-` guard. The
/// memory is converted MiB→bytes for XAPI's byte-valued limits, and both the
/// static and dynamic min/max are pinned to the same target so the VM gets an exact
/// allocation. The VM must be HALTED for the VCPUs-max change (XAPI enforces this;
/// a running VM yields the `xe` error, surfaced to the caller).
///
/// Returns a `Vec` of `xe`-argument strings to run in order:
///   1. `vm-param-set uuid=<uuid> VCPUs-max=<n>`
///   2. `vm-param-set uuid=<uuid> VCPUs-at-startup=<n>`
///   3. `vm-memory-limits-set uuid=<uuid> static-min=<b> static-max=<b> dynamic-min=<b> dynamic-max=<b>`
///
/// # Errors
/// Returns `Err` for an invalid `uuid`, `vcpus` out of `1..=RESIZE_MAX_VCPUS`, or
/// `mem_mib` out of `1..=RESIZE_MAX_MEM_MIB`.
pub fn vm_resize_commands(uuid: &str, vcpus: u64, mem_mib: u64) -> Result<Vec<String>, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    if !(1..=u64::from(RESIZE_MAX_VCPUS)).contains(&vcpus) {
        return Err(format!("vcpus out of range (1..={RESIZE_MAX_VCPUS})"));
    }
    if !(1..=RESIZE_MAX_MEM_MIB).contains(&mem_mib) {
        return Err(format!("mem_mib out of range (1..={RESIZE_MAX_MEM_MIB})"));
    }
    let bytes = mem_mib * 1024 * 1024;
    Ok(vec![
        format!("vm-param-set uuid={uuid} VCPUs-max={vcpus}"),
        format!("vm-param-set uuid={uuid} VCPUs-at-startup={vcpus}"),
        format!(
            "vm-memory-limits-set uuid={uuid} static-min={bytes} static-max={bytes} \
             dynamic-min={bytes} dynamic-max={bytes}"
        ),
    ])
}

/// Build the HCL for a golden-template `xenserver_vm` clone resource. PURE.
///
/// This is the `vm-create` wizard's output — a structural change recorded in Tofu
/// (not poked into XAPI directly), so an applied create never drifts. Every
/// interpolated field is validated first:
///   * `name` → `[A-Za-z0-9._-]` (also the resource's `name_label`);
///   * `template` → `[A-Za-z0-9._-]` (defaults via the caller to `MDE-VM-golden`);
///   * `network_uuid` → hex/dot/dash only;
///   * `vcpus` `1..=RESIZE_MAX_VCPUS`, `mem_mib` `1..=RESIZE_MAX_MEM_MIB`.
/// The Terraform resource address is `xenserver_vm.dc_<sanitized-name>` (dots/dashes
/// → underscores, since an HCL block label must be an identifier). Returns
/// `(resource_address, hcl_block)`. The `lifecycle.ignore_changes` mirrors the
/// adopted build-VM resources so the clone plans clean on the create-only fields.
///
/// # Errors
/// Returns `Err` for any field that fails its validation above.
pub fn vm_create_resource(
    name: &str,
    template: &str,
    vcpus: u64,
    mem_mib: u64,
    network_uuid: &str,
) -> Result<(String, String), String> {
    if name.is_empty() {
        return Err("empty name".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("name contains invalid characters".into());
    }
    if template.is_empty() {
        return Err("empty template".into());
    }
    if !template
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("template contains invalid characters".into());
    }
    if network_uuid.is_empty() {
        return Err("empty network_uuid".into());
    }
    if !network_uuid
        .chars()
        .all(|c| c.is_ascii_hexdigit() || matches!(c, '.' | '-'))
    {
        return Err("network_uuid contains invalid characters".into());
    }
    if !(1..=u64::from(RESIZE_MAX_VCPUS)).contains(&vcpus) {
        return Err(format!("vcpus out of range (1..={RESIZE_MAX_VCPUS})"));
    }
    if !(1..=RESIZE_MAX_MEM_MIB).contains(&mem_mib) {
        return Err(format!("mem_mib out of range (1..={RESIZE_MAX_MEM_MIB})"));
    }
    // An HCL block label must be a bare identifier — fold the name's `.`/`-` to `_`.
    let ident: String = name
        .chars()
        .map(|c| if matches!(c, '.' | '-') { '_' } else { c })
        .collect();
    let addr = format!("xenserver_vm.dc_{ident}");
    let bytes = mem_mib * 1024 * 1024;
    let hcl = format!(
        "resource \"xenserver_vm\" \"dc_{ident}\" {{\n  \
         name_label        = \"{name}\"\n  \
         template_name     = \"{template}\"\n  \
         static_mem_max    = {bytes}\n  \
         vcpus             = {vcpus}\n  \
         check_ip_timeout  = 0\n  \
         network_interface = [{{ device = \"0\", network_uuid = \"{network_uuid}\" }}]\n  \
         lifecycle {{\n    \
         ignore_changes = [hard_drive, template_name, boot_mode, boot_order, \
         cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, \
         name_description, cdrom]\n  \
         }}\n}}\n"
    );
    Ok((addr, hcl))
}

/// Run a remote `xe` command on a dom0 over SSH, returning the process result.
/// Mirrors the exact ssh arg style of `ssh_xe` in the orchestrator.
fn ssh_xe_status(key: &str, dom0: &str, remote: &str) -> std::io::Result<std::process::Output> {
    std::process::Command::new("ssh")
        .args([
            "-i",
            key,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &format!("root@{dom0}"),
            remote,
        ])
        .output()
}

/// The resource key a mutating `verb` op-locks, or `None` for a read-only verb
/// that needs no lock. PURE (used by [`build_reply`]'s op-lock and unit-testable
/// on its own).
///
/// DATACENTER-6 (op-lock half): a second concurrent mutation on the same resource
/// is rejected, so the lock is keyed on the resource the verb targets — every
/// mutating verb THIS responder dispatches targets a single VM, so the key is the
/// VM `uuid`, namespaced `vm:<uuid>`:
/// * `vm-power` / `vm-snapshot` / `vm-clone` / `vm-delete` / `vm-suspend` /
///   `vm-migrate` / `vm-resize` → `vm:<uuid>`;
/// * `vm-create` locks on the new VM's `name` (`vm-new:<name>`) — there is no uuid
///   yet, but two creates of the same name must not race the same `dc-vms.tf` write;
/// * the read-only verbs `do-regions` and `vm-console` return `None` — they read,
///   never mutate, so concurrent reads are allowed.
///
/// (The `host-power` host op runs on its own separate responder
/// [`crate::ipc::host_ops`] and is not dispatched here, so it is intentionally
/// not in this key space — the `vm:` namespace prefix leaves room for a future
/// `host:<dom0>` key without collision.)
///
/// A verb whose body is missing/unparseable, or whose `uuid` is empty, also
/// returns `None`: there is no resource to lock, and the per-verb handler will
/// produce the real validation error. The key is NOT validated for injection here
/// (that is the per-verb command builder's job); it is only ever used as a set
/// member, never interpolated into a shell command.
#[must_use]
pub fn lock_key(verb: &str, req_body: Option<&str>) -> Option<String> {
    // `vm-create` has no uuid yet — it locks on the new VM's name so two creates
    // of the same name can't race the same generated-`.tf` write.
    if verb == "vm-create" {
        let name = serde_json::from_str::<serde_json::Value>(req_body?)
            .ok()?
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)?;
        if name.is_empty() {
            return None;
        }
        return Some(format!("vm-new:{name}"));
    }
    // DATACENTER-19 — `lighthouse-create` has no droplet id yet; it locks on the
    // new lighthouse's name so two creates of the same name can't race the same
    // generated-`.tf` write (mirrors `vm-create`).
    if verb == "lighthouse-create" {
        let name = serde_json::from_str::<serde_json::Value>(req_body?)
            .ok()?
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)?;
        if name.is_empty() {
            return None;
        }
        return Some(format!("lighthouse-new:{name}"));
    }
    // DATACENTER-18 — `genesis-write` mutates the shared `dc-lighthouses.tf`; it
    // locks on the new mesh id so two genesis writes of the same mesh can't race
    // the same `.tf` write. (`genesis-plan` is read-only → no lock, below.)
    if verb == "genesis-write" {
        let mesh_id = serde_json::from_str::<serde_json::Value>(req_body?)
            .ok()?
            .get("mesh_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)?;
        if mesh_id.is_empty() {
            return None;
        }
        return Some(format!("mesh-new:{mesh_id}"));
    }
    // DATACENTER-11 — the snapshot-mutating verbs target a SNAPSHOT (not the VM),
    // and read it from the body's `snapshot` field, so they lock on `snap:<uuid>`
    // (a distinct namespace from `vm:` — reverting/deleting a snapshot shouldn't
    // collide with a power op on its parent VM). `vm-snapshots` is read-only → no
    // lock (it falls through to the `None` tail below).
    // DATACENTER-12 — storage verbs lock on the resource they target so two
    // mutations on the same SR/VDI/VBD don't race. Each reads a different body
    // field for its key; an absent/empty field returns `None` (no lock — the
    // per-verb builder produces the real validation error).
    let keyed = match verb {
        "vm-snapshot-revert" | "vm-snapshot-delete" => Some(("snapshot", "snap")),
        "sr-create" => Some(("name", "sr-new")),
        "vdi-create" => Some(("sr", "sr")),
        "vdi-attach" | "sr-snapshot" => Some(("vdi", "vdi")),
        "vdi-detach" => Some(("vbd", "vbd")),
        _ => None,
    };
    if let Some((field, ns)) = keyed {
        let val = serde_json::from_str::<serde_json::Value>(req_body?)
            .ok()?
            .get(field)
            .and_then(|v| v.as_str())
            .map(str::to_string)?;
        if val.is_empty() {
            return None;
        }
        return Some(format!("{ns}:{val}"));
    }
    // The mutating vm-* verbs lock on the target VM's uuid; read-only verbs hold
    // no lock.
    match verb {
        "vm-power" | "vm-snapshot" | "vm-clone" | "vm-delete" | "vm-suspend" | "vm-migrate"
        | "vm-resize" => {}
        _ => return None,
    }
    let uuid = serde_json::from_str::<serde_json::Value>(req_body?)
        .ok()?
        .get("uuid")
        .and_then(|v| v.as_str())
        .map(str::to_string)?;
    if uuid.is_empty() {
        return None;
    }
    Some(format!("vm:{uuid}"))
}

/// Build the reply for one `action/dc/<verb>` request, dispatching on `verb`.
///
/// DATACENTER-6 (op-lock half): before dispatching a *mutating* verb, the resource
/// key ([`lock_key`]) is claimed in the service's in-flight set. If a mutation on
/// the same resource is already in flight, this returns the clear `busy` reject
/// WITHOUT running the op; otherwise a [`OpLockGuard`] holds the key for the
/// duration of the (synchronous) dispatch and releases it on return (RAII).
/// Read-only verbs ([`lock_key`] → `None`) take no lock and never reject.
#[must_use]
pub fn build_reply(svc: &DatacenterService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    // Op-lock: claim the resource for the duration of a mutating dispatch. The
    // guard is dropped at the end of this function (after the reply is built),
    // releasing the key. Read-only verbs (lock_key → None) are unguarded.
    let _guard = match lock_key(verb, req_body) {
        Some(key) => match svc.try_lock(key.clone()) {
            Some(g) => Some(g),
            None => {
                return err(format!(
                    "resource {key} busy: a {verb} is already in flight"
                ));
            }
        },
        None => None,
    };
    match verb {
        "vm-power" => vm_power_reply(req_body),
        "vm-snapshot" => vm_snapshot_reply(req_body),
        "vm-clone" => vm_clone_reply(req_body),
        "vm-delete" => vm_delete_reply(req_body),
        "vm-console" => vm_console_reply(req_body),
        "vm-suspend" => vm_suspend_reply(req_body),
        "vm-migrate" => vm_migrate_reply(req_body),
        "vm-resize" => vm_resize_reply(req_body),
        "vm-create" => vm_create_reply(svc, req_body),
        "vm-snapshots" => vm_snapshots_reply(req_body),
        "vm-snapshot-revert" => vm_snapshot_revert_reply(req_body),
        "vm-snapshot-delete" => vm_snapshot_delete_reply(req_body),
        "do-regions" => do_regions_reply(),
        "lighthouse-create" => lighthouse_create_reply(svc, req_body),
        "genesis-plan" => genesis_plan_reply(req_body),
        "genesis-write" => genesis_write_reply(svc, req_body),
        // DAR-45 — read-only backoffice planner probe (no lock; mutates nothing).
        "backoffice-plan" => backoffice_plan_reply(svc, req_body),
        // DATACENTER-12 — storage verbs are served on this responder but built by
        // the sibling storage_ops module (the op-lock above already guards them).
        v if crate::ipc::storage_ops::is_storage_verb(v) => {
            crate::ipc::storage_ops::build_reply(v, req_body)
        }
        _ => err("unknown dc verb".into()),
    }
}

/// Handle a `vm-power` request body: parse, allow-list the dom0, then run the
/// mapped `xe` power verb over SSH.
fn vm_power_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-power: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-power: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_power_command(uuid, op) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => json!({ "ok": true }).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-snapshot` request body: parse, allow-list the dom0, then run
/// `xe vm-snapshot …` over SSH. On success `xe` prints the new snapshot uuid on
/// stdout, which is returned as `{"ok":true,"snapshot":"<uuid>"}`.
fn vm_snapshot_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-snapshot: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-snapshot: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_snapshot_command(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let snapshot = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "snapshot": snapshot }).to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-clone` request body: parse, allow-list the dom0, then run
/// `xe vm-clone …` over SSH. On success `xe` prints the new clone's uuid on
/// stdout, which is returned as `{"ok":true,"clone":"<uuid>"}`.
fn vm_clone_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-clone: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-clone: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let name = req.get("name").and_then(serde_json::Value::as_str);

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_clone_command(uuid, name) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let clone = String::from_utf8_lossy(&o.stdout).trim().to_string();
            json!({ "ok": true, "clone": clone }).to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-delete` request body: parse, REQUIRE `confirm == true`,
/// allow-list the dom0, then run `xe vm-uninstall uuid=<uuid> force=true` over
/// SSH. Reply `{"ok":true}` on success.
fn vm_delete_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-delete: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-delete: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // DESTRUCTIVE: refuse unless the caller explicitly confirms. Checked BEFORE
    // the dom0 allow-list and before building/running anything.
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return err("delete requires confirm:true".into());
    }

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_uninstall_command(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => json!({ "ok": true }).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Build the remote `xe` argument string for a VM uninstall (delete). PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`]. Returns
/// `"vm-uninstall uuid=<uuid> force=true"`.
///
/// # Errors
/// Returns `Err` for an empty `uuid`, or a `uuid` containing any character that
/// is not an ASCII hex digit or `-`.
pub fn vm_uninstall_command(uuid: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!("vm-uninstall uuid={uuid} force=true"))
}

/// Build the remote `xe` argument string for reading a VM's console location. PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`], since the result is
/// interpolated into a remote shell `xe …` string. Returns
/// `"console-list vm-uuid=<uuid> params=location --minimal"` — `--minimal` prints
/// just the console object's `location` (the connection URL the noVNC viewer uses).
///
/// # Errors
/// Returns `Err` for an empty `uuid`, or a `uuid` containing any character that is
/// not an ASCII hex digit or `-`.
pub fn vm_console_command(uuid: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!(
        "console-list vm-uuid={uuid} params=location --minimal"
    ))
}

/// Build the remote shell command that LISTS a VM's snapshots. PURE.
///
/// Validates `uuid` is non-empty and contains only `[0-9a-fA-F-]` — the same
/// command-injection guard as [`vm_power_command`], since the result is
/// interpolated into a remote shell string. Emits `<snap-uuid>|<name-label>` per
/// line using the exact `$u|$(…)` per-line idiom the orchestrator's VM read uses
/// ([`crate::workers::datacenter_orchestrator`]), so the reply parses back into
/// `(uuid, name)` pairs with [`parse_xe_snapshots`]. Returns the full `for …`
/// shell line (NOT prefixed with `xe`, since it runs `xe` internally per snapshot).
///
/// # Errors
/// Returns `Err` for an empty `uuid`, or a `uuid` containing any character that is
/// not an ASCII hex digit or `-`.
pub fn vm_snapshots_command(uuid: &str) -> Result<String, String> {
    if uuid.is_empty() {
        return Err("empty uuid".into());
    }
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("uuid contains invalid characters".into());
    }
    Ok(format!(
        "for s in $(xe snapshot-list snapshot-of={uuid} params=uuid --minimal | tr , ' '); \
         do echo \"$s|$(xe snapshot-param-get uuid=$s param-name=name-label)\"; done"
    ))
}

/// Parse the `vm-snapshots` remote output (one `<uuid>|<name-label>` per line)
/// into `(uuid, name)` pairs. PURE. Blank lines and lines missing the `|`
/// delimiter are skipped (best-effort, mirroring `parse_xe_vms`); a trimmed empty
/// uuid is dropped.
#[must_use]
pub fn parse_xe_snapshots(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|line| {
            let (uuid, name) = line.split_once('|')?;
            let uuid = uuid.trim();
            if uuid.is_empty() {
                return None;
            }
            Some((uuid.to_string(), name.trim().to_string()))
        })
        .collect()
}

/// Build the remote `xe` argument string to REVERT a VM to a snapshot. PURE.
///
/// Validates `snapshot` (the snapshot uuid) is non-empty and contains only
/// `[0-9a-fA-F-]` — the same command-injection guard as [`vm_power_command`].
/// `xe snapshot-revert` takes `snapshot-uuid=<uuid>` (it reverts the snapshot's
/// parent VM to that point). Returns `"snapshot-revert snapshot-uuid=<snapshot>"`.
///
/// # Errors
/// Returns `Err` for an empty `snapshot`, or a `snapshot` containing any character
/// that is not an ASCII hex digit or `-`.
pub fn vm_snapshot_revert_command(snapshot: &str) -> Result<String, String> {
    if snapshot.is_empty() {
        return Err("empty snapshot".into());
    }
    if !snapshot.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("snapshot contains invalid characters".into());
    }
    Ok(format!("snapshot-revert snapshot-uuid={snapshot}"))
}

/// Build the remote `xe` argument string to DESTROY (delete) a snapshot. PURE.
///
/// Validates `snapshot` (the snapshot uuid) is non-empty and contains only
/// `[0-9a-fA-F-]` — the same command-injection guard as [`vm_power_command`].
/// `xe snapshot-destroy` takes `uuid=<uuid>`. Returns
/// `"snapshot-destroy uuid=<snapshot>"`.
///
/// # Errors
/// Returns `Err` for an empty `snapshot`, or a `snapshot` containing any character
/// that is not an ASCII hex digit or `-`.
pub fn vm_snapshot_delete_command(snapshot: &str) -> Result<String, String> {
    if snapshot.is_empty() {
        return Err("empty snapshot".into());
    }
    if !snapshot.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("snapshot contains invalid characters".into());
    }
    Ok(format!("snapshot-destroy uuid={snapshot}"))
}

/// Handle a `vm-console` request body: parse, allow-list the dom0, then read the
/// XAPI console `location` over SSH (read-only). On success the trimmed stdout is
/// the connection URL; an empty result means the VM has no console (halted / not
/// running), reported as `{"error":"no console (vm not running?)"}`.
fn vm_console_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-console: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-console: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything. Read-only,
    // so there is no confirm gate, but we still SSH there → keep the allow-list.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let cmd = match vm_console_command(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let location = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if location.is_empty() {
                err("no console (vm not running?)".into())
            } else {
                json!({ "ok": true, "location": location }).to_string()
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-snapshots` request body: parse, allow-list the dom0, then run the
/// snapshot-list remote command (read-only) over SSH and reply with the parsed
/// `(uuid, name)` pairs. An empty list is a success (the VM simply has no
/// snapshots).
fn vm_snapshots_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-snapshots: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-snapshots: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // SECURITY: only act on a dom0 in the configured allowed set — read-only, but
    // we still SSH there, so keep the allow-list.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }
    // The command runs `xe` internally per snapshot, so it is NOT `xe`-prefixed.
    let remote = match vm_snapshots_command(uuid) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let snapshots: Vec<serde_json::Value> = parse_xe_snapshots(&stdout)
                .into_iter()
                .map(|(uuid, name)| json!({ "uuid": uuid, "name": name }))
                .collect();
            json!({ "ok": true, "snapshots": snapshots }).to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-snapshot-revert` request body: parse, allow-list the dom0, then
/// run `xe snapshot-revert snapshot-uuid=<snapshot>` over SSH.
fn vm_snapshot_revert_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-snapshot-revert: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-snapshot-revert: bad json: {e}")),
    };
    let snapshot = req
        .get("snapshot")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // SECURITY: only act on a dom0 in the configured allowed set.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }
    let cmd = match vm_snapshot_revert_command(snapshot) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe_ok(dom0, &cmd)
}

/// Handle a `vm-snapshot-delete` request body: parse, REQUIRE `confirm == true`
/// (mirrors `vm-delete` — a snapshot-destroy is irreversible), allow-list the
/// dom0, then run `xe snapshot-destroy uuid=<snapshot>` over SSH.
fn vm_snapshot_delete_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-snapshot-delete: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-snapshot-delete: bad json: {e}")),
    };
    let snapshot = req
        .get("snapshot")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // DESTRUCTIVE: refuse unless the caller explicitly confirms. Checked BEFORE
    // the dom0 allow-list and before building/running anything (mirrors vm-delete).
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return err("snapshot-delete requires confirm:true".into());
    }
    // SECURITY: only act on a dom0 in the configured allowed set.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }
    let cmd = match vm_snapshot_delete_command(snapshot) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe_ok(dom0, &cmd)
}

/// Run one allow-listed `xe <cmd>` on `dom0` over SSH and turn the outcome into the
/// standard `{"ok":true}` / `{"error":..}` reply. Used by the simple mutating verbs
/// (`vm-suspend` / `vm-migrate`) whose only success signal is the exit status. The
/// dom0 allow-list is the caller's responsibility (checked before this).
fn run_xe_ok(dom0: &str, cmd: &str) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => json!({ "ok": true }).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Handle a `vm-suspend` request body: parse, allow-list the dom0, then run the
/// mapped `xe vm-{suspend,resume}` verb over SSH.
fn vm_suspend_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-suspend: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-suspend: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // SECURITY: only act on a dom0 in the configured allowed set.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }
    let cmd = match vm_suspend_command(uuid, op) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe_ok(dom0, &cmd)
}

/// Handle a `vm-migrate` request body: parse, allow-list the dom0, then run
/// `xe vm-migrate uuid=<uuid> host=<host> live=true` over SSH.
fn vm_migrate_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-migrate: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-migrate: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // SECURITY: only act on a dom0 in the configured allowed set.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }
    let cmd = match vm_migrate_command(uuid, host) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe_ok(dom0, &cmd)
}

/// Handle a `vm-resize` request body: parse, allow-list the dom0, then run the
/// VCPUs + memory-limit `xe` commands in order. Stops at the first failing command
/// and surfaces its error; only an all-green run replies `{"ok":true}`.
fn vm_resize_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-resize: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-resize: bad json: {e}")),
    };
    let uuid = req
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let vcpus = req.get("vcpus").and_then(serde_json::Value::as_u64);
    let mem_mib = req.get("mem_mib").and_then(serde_json::Value::as_u64);
    let (Some(vcpus), Some(mem_mib)) = (vcpus, mem_mib) else {
        return err("vm-resize: vcpus and mem_mib must be integers".into());
    };
    // SECURITY: only act on a dom0 in the configured allowed set.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }
    let cmds = match vm_resize_commands(uuid, vcpus, mem_mib) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    for cmd in &cmds {
        let reply = run_xe_ok(dom0, cmd);
        // The first non-ok reply is the failure — return it verbatim (it already
        // carries the `xe` error message).
        if !reply.contains("\"ok\":true") {
            return reply;
        }
    }
    json!({ "ok": true }).to_string()
}

/// Handle a `vm-create` request body: parse + validate, allow-list the dom0, then
/// WRITE a golden-template clone resource into the `xen-xapi` workspace's generated
/// `dc-vms.tf` (a structural change recorded in Tofu — the caller applies it via
/// `action/dc/tofu-apply`, so a create never drifts). A duplicate resource address
/// (same name already present in the file) is rejected so a create can't silently
/// overwrite an existing block. Replies `{"ok":true,"resource":..,"path":..}`.
fn vm_create_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("vm-create: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("vm-create: bad json: {e}")),
    };
    let name = req
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // The golden template clones from — default to the project's `MDE-VM-golden`.
    let template = req
        .get("template")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("MDE-VM-golden");
    let network_uuid = req
        .get("network_uuid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let vcpus = req.get("vcpus").and_then(serde_json::Value::as_u64);
    let mem_mib = req.get("mem_mib").and_then(serde_json::Value::as_u64);
    let (Some(vcpus), Some(mem_mib)) = (vcpus, mem_mib) else {
        return err("vm-create: vcpus and mem_mib must be integers".into());
    };
    // SECURITY: only target a dom0 in the configured allowed set (the pool the
    // resource lands in). Checked before any field validation / file write.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }
    let (addr, hcl) = match vm_create_resource(name, template, vcpus, mem_mib, network_uuid) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // The generated file lives in the allow-listed `xen-xapi` workspace under the
    // repo root the daemon runs in — the same tree `action/dc/tofu-apply` plans.
    let tf_dir = svc.workgroup_root.join("infra/tofu/xen-xapi");
    let tf_path = tf_dir.join("dc-vms.tf");
    let rel = "infra/tofu/xen-xapi/dc-vms.tf";
    // Refuse to overwrite an existing block for the same name (idempotent create —
    // the operator deletes via Tofu, not by silently clobbering the HCL).
    let existing = std::fs::read_to_string(&tf_path).unwrap_or_default();
    let marker = format!("resource \"xenserver_vm\" \"{}\"", addr_label(&addr));
    if existing.contains(&marker) {
        return err(format!(
            "a VM resource named {name} already exists in {rel}"
        ));
    }
    if let Err(e) = std::fs::create_dir_all(&tf_dir) {
        return err(format!("vm-create: cannot create {rel} dir: {e}"));
    }
    // Append the new block (a header comment is written once, on the first create).
    let mut out = existing;
    if out.is_empty() {
        out.push_str(
            "# DATACENTER-11 — VMs-tab-created VMs (golden-template clones). Each block\n\
             # is written by the `action/dc/vm-create` wizard and materialized by a\n\
             # `tofu apply` of this workspace, so every create goes through Tofu (no\n\
             # drift). Edit/remove via Tofu, not by hand.\n",
        );
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&hcl);
    if let Err(e) = std::fs::write(&tf_path, out) {
        return err(format!("vm-create: cannot write {rel}: {e}"));
    }
    json!({ "ok": true, "resource": addr, "path": rel }).to_string()
}

/// The HCL block label inside a `xenserver_vm.dc_<ident>` resource address — i.e.
/// the part after the `xenserver_vm.` type prefix. PURE helper for the duplicate
/// check (so the marker matches the block the writer emits).
fn addr_label(addr: &str) -> &str {
    addr.strip_prefix("xenserver_vm.").unwrap_or(addr)
}

/// Parse a `doctl compute region list -o json` array into `(slug, name, available)`
/// triples. PURE.
///
/// Each array element is expected to be an object with string `slug`/`name` and a
/// boolean `available`. Missing string fields default to empty, a missing/non-bool
/// `available` defaults to `false`. Non-array or unparsable input yields an empty
/// vector (best-effort — the caller turns that into the doctl-failed error).
#[must_use]
pub fn parse_regions(json: &str) -> Vec<(String, String, bool)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|r| {
            let slug = r
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = r
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let available = r
                .get("available")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            (slug, name, available)
        })
        .collect()
}

/// Handle a `do-regions` request: run `doctl compute region list` (read-only) and
/// reply with the parsed regions. The doctl context is `MCNF_DOCTL_CONTEXT`
/// (default `mackes`). Best-effort: doctl missing/failed → the doctl-failed error.
fn do_regions_reply() -> String {
    let err = |m: &str| json!({ "error": m }).to_string();
    let context = std::env::var("MCNF_DOCTL_CONTEXT").unwrap_or_else(|_| "mackes".to_string());
    let output = std::process::Command::new("doctl")
        .args([
            "compute",
            "region",
            "list",
            "--context",
            &context,
            "-o",
            "json",
        ])
        .output();
    let Ok(out) = output else {
        return err("doctl region list failed");
    };
    if !out.status.success() {
        return err("doctl region list failed");
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let regions: Vec<serde_json::Value> = parse_regions(&stdout)
        .into_iter()
        .map(
            |(slug, name, available)| json!({ "slug": slug, "name": name, "available": available }),
        )
        .collect();
    json!({ "ok": true, "regions": regions }).to_string()
}

/// DATACENTER-19 — `true` iff `s` is a non-empty `DigitalOcean` region slug:
/// lowercase ASCII alphanumerics + dash (e.g. `nyc3`, `sfo3`, `fra1`). PURE — used
/// to validate the `region`/`size`/`image` slugs before they reach the HCL.
#[must_use]
fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// DATACENTER-19 — recommend a region for a NEW lighthouse that ADDS geographic
/// spread. PURE.
///
/// `available` is the available-region universe (the `do-regions` reply's slugs);
/// `used` is the regions the EXISTING lighthouses already sit in (read off the
/// panel's `droplet` rows). The recommendation:
///   * never recommends a region that already hosts a lighthouse (no spread gain);
///   * prefers a region in a DIFFERENT geo group (the slug's leading letters, e.g.
///     `nyc`/`sfo`/`fra`/`sgp`) from every used region — that's the honest
///     geo-spread nudge (doctl's region list exposes no latency/price, so the nudge
///     is geo-based, not latency/price-based);
///   * failing a new geo, falls back to any available region not already used;
///   * returns `None` when every available region is already used (or the universe
///     is empty) — the caller then surfaces "no spread-adding region".
///
/// `available` is taken in slug order, so the pick is deterministic for a given
/// input (first new-geo slug, else first unused slug).
#[must_use]
pub fn recommend_spread_region(available: &[String], used: &[String]) -> Option<String> {
    // The geo prefix of a slug: its leading ASCII letters (`nyc3` → `nyc`). An
    // all-digit / empty slug folds to "" — its own (degenerate) group.
    let geo =
        |slug: &str| -> String { slug.chars().take_while(char::is_ascii_alphabetic).collect() };
    let used_set: BTreeSet<&str> = used.iter().map(String::as_str).collect();
    let used_geos: BTreeSet<String> = used.iter().map(|s| geo(s)).collect();
    // First pass: an unused region whose geo group no existing lighthouse occupies.
    if let Some(r) = available
        .iter()
        .find(|s| !used_set.contains(s.as_str()) && !used_geos.contains(&geo(s)))
    {
        return Some(r.clone());
    }
    // Fallback: any unused region (same geo as an existing one is still a distinct
    // failure domain — better than recommending a region that's already hosting a
    // lighthouse).
    available
        .iter()
        .find(|s| !used_set.contains(s.as_str()))
        .cloned()
}

/// DATACENTER-19 — build a `digitalocean_droplet` Tofu resource for a new
/// lighthouse. Mirrors [`vm_create_resource`] (validate-then-interpolate, return
/// `(resource_address, hcl_block)`). PURE — no I/O.
///
/// Every interpolated field is validated first: `name` → `[A-Za-z0-9._-]` (also the
/// droplet's `name`); `region`/`size`/`image` → DO region/size/image slug chars
/// (`[a-z0-9-]`).
///
/// The resource address is `digitalocean_droplet.lighthouse_<sanitized-name>`
/// (dots/dashes → underscores — an HCL block label must be a bare identifier). The
/// block tags `magic-lighthouse` (so the orchestrator's droplet inventory picks it
/// up) and registers the mesh SSH key, matching the `zone1-do` grow-path comment.
///
/// # Errors
/// Returns `Err` for any field that fails its validation.
pub fn lighthouse_create_resource(
    name: &str,
    region: &str,
    size: &str,
    image: &str,
) -> Result<(String, String), String> {
    if name.is_empty() {
        return Err("empty name".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("name contains invalid characters".into());
    }
    if !is_slug(region) {
        return Err("region is not a valid slug".into());
    }
    if !is_slug(size) {
        return Err("size is not a valid slug".into());
    }
    if !is_slug(image) {
        return Err("image is not a valid slug".into());
    }
    // An HCL block label must be a bare identifier — fold the name's `.`/`-` to `_`.
    let ident: String = name
        .chars()
        .map(|c| if matches!(c, '.' | '-') { '_' } else { c })
        .collect();
    let addr = format!("digitalocean_droplet.lighthouse_{ident}");
    let hcl = format!(
        "resource \"digitalocean_droplet\" \"lighthouse_{ident}\" {{\n  \
         name     = \"{name}\"\n  \
         region   = \"{region}\"\n  \
         size     = \"{size}\"\n  \
         image    = \"{image}\"\n  \
         tags     = [\"magic-lighthouse\"]\n  \
         ssh_keys = [digitalocean_ssh_key.mackes_mesh_claude.id]\n  \
         lifecycle {{\n    \
         ignore_changes = [image, user_data, ssh_keys, tags]\n  \
         }}\n}}\n"
    );
    Ok((addr, hcl))
}

/// DATACENTER-19 — the HCL block label inside a
/// `digitalocean_droplet.lighthouse_<ident>` address (the part after the
/// `digitalocean_droplet.` type prefix). PURE — for the duplicate check.
fn droplet_addr_label(addr: &str) -> &str {
    addr.strip_prefix("digitalocean_droplet.").unwrap_or(addr)
}

/// DATACENTER-19 — handle a `lighthouse-create` request: parse + validate, then
/// WRITE a `digitalocean_droplet` resource into the `zone1-do` workspace's
/// generated `dc-lighthouses.tf`. Mirrors [`vm_create_reply`] exactly (idempotent —
/// a duplicate `name` is rejected so a create never silently overwrites).
///
/// The actual `tofu apply` (live droplet provision) + the bootstrap (`mackesd found
/// --role lighthouse`) + the DNS record are the CARRIED live-DO step — this only
/// records the structural change in Tofu, so the provision goes through Tofu (no
/// drift). Replies `{"ok":true,"resource":..,"path":..}`.
fn lighthouse_create_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("lighthouse-create: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("lighthouse-create: bad json: {e}")),
    };
    let name = req
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let region = req
        .get("region")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // `size`/`image` default to the standard lighthouse slugs (the `zone1-do`
    // workspace's `lighthouse_size`/`lighthouse_image` variable defaults), so the
    // guided flow only requires name + region.
    let size = req
        .get("size")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("s-2vcpu-2gb");
    let image = req
        .get("image")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("fedora-43-x64");
    let (addr, hcl) = match lighthouse_create_resource(name, region, size, image) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // The generated file lives in the allow-listed `zone1-do` workspace under the
    // repo root the daemon runs in — the same tree `action/dc/tofu-apply` plans.
    let tf_dir = svc.workgroup_root.join("infra/tofu/zone1-do");
    let tf_path = tf_dir.join("dc-lighthouses.tf");
    let rel = "infra/tofu/zone1-do/dc-lighthouses.tf";
    // Refuse to overwrite an existing block for the same name (idempotent create).
    let existing = std::fs::read_to_string(&tf_path).unwrap_or_default();
    let marker = format!(
        "resource \"digitalocean_droplet\" \"{}\"",
        droplet_addr_label(&addr)
    );
    if existing.contains(&marker) {
        return err(format!(
            "a lighthouse resource named {name} already exists in {rel}"
        ));
    }
    if let Err(e) = std::fs::create_dir_all(&tf_dir) {
        return err(format!("lighthouse-create: cannot create {rel} dir: {e}"));
    }
    // Append the new block (a header comment is written once, on the first create).
    let mut out = existing;
    if out.is_empty() {
        out.push_str(
            "# DATACENTER-19 — Network-tab-created lighthouses (the guided\n\
             # new-lighthouse flow). Each block is written by the\n\
             # `action/dc/lighthouse-create` flow and PROVISIONED by a `tofu apply`\n\
             # of this workspace, so every create goes through Tofu (no drift). After\n\
             # apply: bootstrap mackesd + `mackesd found --role lighthouse`, then add\n\
             # the lighthouse-NN A record. Edit/remove via Tofu, not by hand.\n",
        );
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&hcl);
    if let Err(e) = std::fs::write(&tf_path, out) {
        return err(format!("lighthouse-create: cannot write {rel}: {e}"));
    }
    json!({ "ok": true, "resource": addr, "path": rel }).to_string()
}

// ── DATACENTER-18 — New-Mesh genesis ("give birth to a new Nebula") ──────────
//
// The genesis wizard's backend half — GLUE over DC-19, not a rewrite. It does NOT
// found a live mesh itself (founding is irreversible + costs real DO money + would
// create a rogue mesh), so — mirroring DC-19's `lighthouse-create` honesty — the
// verbs here only PLAN the genesis (`genesis-plan`, read-only) and WRITE the
// founding lighthouse's Tofu (`genesis-write`). The droplet half REUSES
// [`lighthouse_create_resource`] (the same `digitalocean_droplet` DC-19 emits);
// genesis adds the founding DNS A-record. The real `tofu apply` (droplet spend)
// goes through the gated `action/dc/tofu-apply` on `zone1-do`; the real `mackesd
// found` runs on the booted droplet (the founding cloud-init,
// `install-helpers/do-lighthouse-cloudinit.sh`). No credential is ever written to
// the repo/HCL/log here.

/// The ordered genesis step labels.
///
/// Shown in the wizard's review step and echoed in the `genesis-plan` reply so the
/// GUI and the backend agree on the sequence. PURE constant — the plan is
/// descriptive; each step is executed by a distinct, already-shipped primitive
/// (the DC-19 lighthouse Tofu-write + gated apply, the cloud-init `mackesd found`,
/// the DNS record, the printed join token).
pub const GENESIS_STEPS: [&str; 6] = [
    "generate the mesh CA (minted by `mackesd found` on the new lighthouse)",
    "provision the first lighthouse droplet (DigitalOcean, via the gated zone1-do tofu apply)",
    "found the mesh on it (`mackesd found` — self-signs, brings up the overlay)",
    "seed the founding bundle + bring up QNM-Shared / Caddy",
    "register the lighthouse DNS A-record",
    "emit the first single-use join token",
];

/// Validate a new-mesh id as typed. PURE.
///
/// A mesh id is a DNS-ish label: non-empty, ASCII lowercase alphanumeric + hyphen,
/// not starting/ending with a hyphen, at most 63 chars (it becomes the founding
/// droplet's name + DNS record + HCL block label). Rejects anything that could
/// widen the resource namespace or inject HCL.
///
/// # Errors
/// Returns `Err` describing the first rule the id breaks.
pub fn genesis_mesh_id_valid(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("empty mesh id".into());
    }
    if id.len() > 63 {
        return Err("mesh id too long (max 63 chars)".into());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("mesh id must be lowercase letters, digits, or hyphens".into());
    }
    if id.starts_with('-') || id.ends_with('-') {
        return Err("mesh id must not start or end with a hyphen".into());
    }
    Ok(())
}

/// Build the founding lighthouse's `digitalocean_droplet` + `digitalocean_record`
/// HCL for a brand-new `mesh_id` in `region`. PURE.
///
/// GLUE over DC-19: the droplet half is exactly [`lighthouse_create_resource`]
/// (named `lh-<mesh_id>-01`, standard lighthouse size/image), so the genesis
/// droplet is byte-identical to a DC-19 lighthouse the orchestrator already manages.
/// Genesis ADDS the founding DNS A-record (`lighthouse-<mesh_id>`) pointing at the
/// droplet's `ipv4_address` — the DC-19 flow leaves DNS to a manual step, but a
/// genesis IS the DNS-registration step, so it's emitted here. NO credential is
/// ever interpolated.
///
/// Returns `(droplet_resource_address, hcl_block)` (the droplet+record blocks).
///
/// # Errors
/// Returns `Err` if `mesh_id` / `region` fail their validation.
pub fn genesis_lighthouse_resource(
    mesh_id: &str,
    region: &str,
) -> Result<(String, String), String> {
    genesis_mesh_id_valid(mesh_id)?;
    if !is_slug(region) {
        return Err("region is not a valid slug".into());
    }
    let droplet_name = format!("lh-{mesh_id}-01");
    // REUSE DC-19's droplet HCL (standard lighthouse size/image defaults).
    let (addr, droplet_hcl) =
        lighthouse_create_resource(&droplet_name, region, "s-2vcpu-2gb", "fedora-43-x64")?;
    // The block label DC-19 minted (`lighthouse_<ident>`) — reuse it for the record.
    let ident = droplet_addr_label(&addr)
        .strip_prefix("lighthouse_")
        .unwrap_or_else(|| droplet_addr_label(&addr));
    let record_name = format!("lighthouse-{mesh_id}");
    let record_hcl = format!(
        "\nresource \"digitalocean_record\" \"genesis_{ident}\" {{\n  \
         domain = digitalocean_domain.primary.id\n  \
         type   = \"A\"\n  \
         name   = \"{record_name}\"\n  \
         value  = digitalocean_droplet.lighthouse_{ident}.ipv4_address\n  \
         ttl    = 3600\n}}\n"
    );
    Ok((addr, format!("{droplet_hcl}{record_hcl}")))
}

/// Probe the mesh credential store for the presence of the `do-token` (the DO API
/// credential a live genesis apply needs). Returns only a boolean — the token is
/// NEVER read into a reply/log. A store/tooling failure is treated as "absent" (the
/// wizard then warns to provision it), never as a fake-present.
fn do_token_present() -> bool {
    let store = crate::ipc::secret_store::SecretStore::resolve(
        &crate::ipc::secret_store::repo_root(),
        &crate::default_qnm_shared_root(),
    );
    matches!(store.get("do-token"), Ok(Some(_)))
}

/// Build the `genesis-plan` reply value for `(mesh_id, region)`. PURE except for
/// the credential-store presence probe the caller injects via `do_token_present`.
///
/// Validates both inputs, then reports the ordered [`GENESIS_STEPS`], the Tofu
/// resource address the write would land, the rel path it writes, the gated
/// workspace (`zone1-do`), and `secrets_ready` (whether `do-token` is already in
/// the store — the boolean only, never the token). Read-only: it plans, never founds.
///
/// # Errors
/// Returns `Err(message)` for an invalid `mesh_id` / `region`.
pub fn genesis_plan(
    mesh_id: &str,
    region: &str,
    do_token_present: bool,
) -> Result<serde_json::Value, String> {
    let (addr, _hcl) = genesis_lighthouse_resource(mesh_id, region)?;
    Ok(json!({
        "ok": true,
        "mesh_id": mesh_id,
        "region": region,
        "steps": GENESIS_STEPS,
        "resource": addr,
        "path": "infra/tofu/zone1-do/dc-lighthouses.tf",
        "workspace": "zone1-do",
        // Only the PRESENCE boolean — never the credential itself.
        "secrets_ready": do_token_present,
    }))
}

/// Handle a `genesis-plan` request body `{ "mesh_id", "region" }` (read-only):
/// validate, probe the credential store for the `do-token` presence (the boolean
/// only), and reply with the ordered genesis steps + the Tofu resource preview +
/// the gated `zone1-do` workspace. It PLANS the genesis; it never founds a mesh.
fn genesis_plan_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("genesis-plan: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("genesis-plan: bad json: {e}")),
    };
    let mesh_id = req
        .get("mesh_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let region = req
        .get("region")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match genesis_plan(mesh_id, region, do_token_present()) {
        Ok(v) => v.to_string(),
        Err(e) => err(e),
    }
}

/// Handle a `genesis-write` request body `{ "mesh_id", "region", "confirm": true }`.
///
/// A STRUCTURAL change → it WRITES the founding lighthouse's `digitalocean_droplet`
/// resource (REUSING DC-19's [`lighthouse_create_resource`]) plus its founding DNS
/// A-record into the allow-listed `zone1-do` workspace's generated
/// `dc-lighthouses.tf` (idempotent; a repeated `mesh_id` is rejected so a write
/// never silently overwrites). It does NOT apply — the caller then runs the gated
/// `action/dc/tofu-apply` on `zone1-do` (the real droplet spend), and the live
/// `mackesd found` runs on the booted droplet (the founding cloud-init). The
/// destructive write requires `confirm == true` (the wizard's arm→confirm gate).
/// Replies `{"ok":true,"resource":..,"path":..,"workspace":"zone1-do"}`. NO
/// credential is read or written here.
fn genesis_write_reply(svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("genesis-write: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("genesis-write: bad json: {e}")),
    };
    // DESTRUCTIVE-ish (writes Tofu that founds a real mesh on apply): refuse unless
    // the caller explicitly confirms — the same fail-closed gate as `vm-delete`.
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return err("genesis-write requires confirm:true".into());
    }
    let mesh_id = req
        .get("mesh_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let region = req
        .get("region")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let (addr, hcl) = match genesis_lighthouse_resource(mesh_id, region) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // The generated file lives in the allow-listed `zone1-do` workspace under the
    // repo root the daemon runs in — the same tree DC-19 + `tofu-apply` use.
    let tf_dir = svc.workgroup_root.join("infra/tofu/zone1-do");
    let tf_path = tf_dir.join("dc-lighthouses.tf");
    let rel = "infra/tofu/zone1-do/dc-lighthouses.tf";
    // Refuse to overwrite an existing block for the same mesh's founding droplet
    // (idempotent — the operator removes via Tofu, not by silently clobbering).
    let existing = std::fs::read_to_string(&tf_path).unwrap_or_default();
    let marker = format!(
        "resource \"digitalocean_droplet\" \"{}\"",
        droplet_addr_label(&addr)
    );
    if existing.contains(&marker) {
        return err(format!(
            "a genesis lighthouse for mesh {mesh_id} already exists in {rel}"
        ));
    }
    if let Err(e) = std::fs::create_dir_all(&tf_dir) {
        return err(format!("genesis-write: cannot create {rel} dir: {e}"));
    }
    // Append the new blocks (a header comment is written once, on the first write).
    let mut out = existing;
    if out.is_empty() {
        out.push_str(
            "# DATACENTER-18/19 — DO lighthouses written by the Datacenter flows\n\
             # (DC-19 add-lighthouse + DC-18 New-Mesh genesis) and PROVISIONED by a\n\
             # gated `tofu apply` of this workspace, so every create goes through Tofu\n\
             # (no drift). A genesis block also founds the mesh on the booted droplet\n\
             # via the founding cloud-init (`mackesd found`). All credentials come\n\
             # from the mesh credential store, never from this file. Edit/remove via\n\
             # Tofu, not by hand.\n",
        );
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&hcl);
    if let Err(e) = std::fs::write(&tf_path, out) {
        return err(format!("genesis-write: cannot write {rel}: {e}"));
    }
    // DAR-45 — echo the chosen backoffice tier so the wizard knows the genesis
    // recorded a backoffice opt-in. `backoffice_tier` in the body (minimal|full)
    // → `backoffice_intent {tier}` in the reply; ABSENT/off → null (behavior
    // unchanged — genesis-write does NOT itself record intent or run the
    // orchestrator; that stays `mackesd found --with-backoffice` / the operator).
    let backoffice_intent = match req
        .get("backoffice_tier")
        .and_then(serde_json::Value::as_str)
    {
        Some(t) if backoffice_tier_valid(t).is_ok() => json!({ "tier": t }),
        _ => serde_json::Value::Null,
    };
    json!({
        "ok": true,
        "resource": addr,
        "path": rel,
        "workspace": "zone1-do",
        "backoffice_intent": backoffice_intent,
    })
    .to_string()
}

/// DAR-45 — validate a backoffice tier. Accepts only `minimal` / `full`. PURE.
///
/// # Errors
/// Returns `Err` for any other tier.
fn backoffice_tier_valid(tier: &str) -> Result<&str, String> {
    match tier {
        "minimal" | "full" => Ok(tier),
        _ => Err(format!(
            "invalid backoffice tier '{tier}' (expected minimal|full)"
        )),
    }
}

/// Handle a `backoffice-plan` request body `{ "tier": "minimal"|"full" }` (READ-ONLY).
///
/// DAR-45 — the genesis-wizard's backoffice step fires this to render the ordered
/// unit list the orchestrator (`backoffice-up.sh`) would enable. It is a REAL
/// PROBE, not a canned list: it shells out to `automation/backoffice/backoffice-plan.sh
/// --tier <t>` (the single source of truth, which reads the tier manifest) and
/// passes its JSON `units` through verbatim — so the acceptance "RPC units MATCH
/// `backoffice-plan.sh --tier <t>`" holds by construction. The `secrets_ready`
/// boolean is RE-STAMPED from the SAME [`do_token_present`] probe the genesis-plan
/// step uses, so both wizard steps report the credential state identically (and the
/// wizard never has to trust the script's own probe for that one field). The script
/// mutates nothing; this verb takes no op-lock (read-only).
///
/// On a missing script / non-zero exit (e.g. a broken manifest with a dangling
/// `via_script`), replies `{"error":..}` carrying the script's stderr — surfaced
/// honestly rather than faked-present.
fn backoffice_plan_reply(_svc: &DatacenterService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("backoffice-plan: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("backoffice-plan: bad json: {e}")),
    };
    let tier = req
        .get("tier")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if let Err(e) = backoffice_tier_valid(tier) {
        return err(e);
    }
    // Resolve the planner under the deployed repo root (the `MCNF_REPO` convention,
    // same as the secret store) so the relative `automation/...` path resolves
    // regardless of the daemon's cwd (`/` under systemd).
    let repo = crate::ipc::secret_store::repo_root();
    let script = repo.join("automation/backoffice/backoffice-plan.sh");
    if !script.is_file() {
        return err(format!(
            "backoffice-plan: planner not found at {} (is the repo deployed?)",
            script.display()
        ));
    }
    let output = std::process::Command::new("bash")
        .arg(&script)
        .arg("--tier")
        .arg(tier)
        .current_dir(&repo)
        .output();
    let out = match output {
        Ok(o) => o,
        Err(e) => return err(format!("backoffice-plan: spawn failed: {e}")),
    };
    if !out.status.success() {
        // The script prints its JSON on stdout and the broken-unit diagnostic on
        // stderr; surface stderr so a dangling via_script is named, not hidden.
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = msg.trim();
        return err(format!(
            "backoffice-plan: planner failed{}",
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut plan: serde_json::Value = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(e) => {
            return err(format!(
                "backoffice-plan: planner output not decodable: {e}"
            ))
        }
    };
    // Re-stamp secrets_ready from the SAME Rust probe genesis-plan uses, so the two
    // wizard steps agree (and the boolean is the daemon's view, not the shell's).
    if let Some(obj) = plan.as_object_mut() {
        obj.insert("secrets_ready".into(), json!(do_token_present()));
    }
    plan.to_string()
}

/// Run the datacenter Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DatacenterService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(
    persist: &Persist,
    svc: &DatacenterService,
    cursors: &mut HashMap<String, String>,
) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "dc responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dom0 allow-list (`xen_dom0s`) reads a process-wide env var. The tests
    /// that mutate it (the `vm-create` happy path) and the ones that assert the
    /// default-empty allow-list rejects (the op-lock + create-reject tests) must
    /// not observe each other's env, so they serialize behind this one lock — the
    /// same idiom the panel's saved-views tests use.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("vm-power"), "action/dc/vm-power");
        assert_eq!(action_topic("vm-snapshot"), "action/dc/vm-snapshot");
        assert_eq!(action_topic("vm-clone"), "action/dc/vm-clone");
        assert_eq!(action_topic("vm-delete"), "action/dc/vm-delete");
        assert_eq!(action_topic("vm-console"), "action/dc/vm-console");
        assert_eq!(action_topic("vm-suspend"), "action/dc/vm-suspend");
        assert_eq!(action_topic("vm-migrate"), "action/dc/vm-migrate");
        assert_eq!(action_topic("vm-resize"), "action/dc/vm-resize");
        assert_eq!(action_topic("vm-create"), "action/dc/vm-create");
        assert_eq!(action_topic("vm-snapshots"), "action/dc/vm-snapshots");
        assert_eq!(
            action_topic("vm-snapshot-revert"),
            "action/dc/vm-snapshot-revert"
        );
        assert_eq!(
            action_topic("vm-snapshot-delete"),
            "action/dc/vm-snapshot-delete"
        );
        assert_eq!(action_topic("do-regions"), "action/dc/do-regions");
        assert_eq!(
            action_topic("lighthouse-create"),
            "action/dc/lighthouse-create"
        );
        assert!(ACTION_VERBS.contains(&"vm-power"));
        assert!(ACTION_VERBS.contains(&"vm-snapshot"));
        assert!(ACTION_VERBS.contains(&"vm-clone"));
        assert!(ACTION_VERBS.contains(&"vm-delete"));
        assert!(ACTION_VERBS.contains(&"vm-console"));
        assert!(ACTION_VERBS.contains(&"vm-suspend"));
        assert!(ACTION_VERBS.contains(&"vm-migrate"));
        assert!(ACTION_VERBS.contains(&"vm-resize"));
        assert!(ACTION_VERBS.contains(&"vm-create"));
        assert!(ACTION_VERBS.contains(&"vm-snapshots"));
        assert!(ACTION_VERBS.contains(&"vm-snapshot-revert"));
        assert!(ACTION_VERBS.contains(&"vm-snapshot-delete"));
        assert!(ACTION_VERBS.contains(&"do-regions"));
        assert!(ACTION_VERBS.contains(&"lighthouse-create"));
    }

    // ── DAR-45 — genesis-wizard backoffice step + backoffice-plan verb ──

    #[test]
    fn backoffice_plan_verb_and_topic_registered() {
        assert!(ACTION_VERBS.contains(&"backoffice-plan"));
        assert_eq!(action_topic("backoffice-plan"), "action/dc/backoffice-plan");
        // read-only → no op-lock key (so two plan probes never collide / reject).
        assert!(lock_key("backoffice-plan", Some(r#"{"tier":"full"}"#)).is_none());
    }

    #[test]
    fn backoffice_tier_validation() {
        assert!(backoffice_tier_valid("minimal").is_ok());
        assert!(backoffice_tier_valid("full").is_ok());
        assert!(backoffice_tier_valid("bogus").is_err());
        assert!(backoffice_tier_valid("").is_err());
    }

    #[test]
    fn backoffice_plan_reply_rejects_bad_input() {
        let svc = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // Missing body.
        let r = backoffice_plan_reply(&svc, None);
        assert!(r.contains("error"), "{r}");
        // Bad json.
        let r = backoffice_plan_reply(&svc, Some("not json"));
        assert!(r.contains("error") && r.contains("bad json"), "{r}");
        // Invalid tier.
        let r = backoffice_plan_reply(&svc, Some(r#"{"tier":"bogus"}"#));
        assert!(
            r.contains("error") && r.contains("invalid backoffice tier"),
            "{r}"
        );
    }

    #[test]
    fn backoffice_plan_reply_matches_the_script_output() {
        // The acceptance: the RPC's rendered unit list MUST match
        // `backoffice-plan.sh --tier <t>` output (a REAL probe, not canned). We
        // point MCNF_REPO at the worktree (CARGO_MANIFEST_DIR is crates/mesh/mackesd,
        // so ../../.. is the repo root) and run BOTH the verb and the script, then
        // assert their `units` arrays are identical. Skips gracefully if the script
        // isn't present in this checkout (so the suite stays green off-repo).
        let _g = lock_env();
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .expect("repo root");
        let script = repo.join("automation/backoffice/backoffice-plan.sh");
        if !script.is_file() {
            eprintln!(
                "skipping: {} not present in this checkout",
                script.display()
            );
            return;
        }
        // Test-only env set under the serializing ENV_LOCK.
        let prev_repo = std::env::var_os("MCNF_REPO");
        std::env::set_var("MCNF_REPO", &repo);
        let svc = DatacenterService::new(repo.clone());

        for tier in ["minimal", "full"] {
            // The verb's reply.
            let body = format!(r#"{{"tier":"{tier}"}}"#);
            let reply = backoffice_plan_reply(&svc, Some(&body));
            let rpc: serde_json::Value = serde_json::from_str(&reply)
                .unwrap_or_else(|e| panic!("rpc json {tier}: {e}\n{reply}"));
            assert_eq!(rpc["ok"], true, "rpc not ok for {tier}: {reply}");
            assert_eq!(rpc["tier"], tier, "rpc tier mismatch: {reply}");

            // The script's own output (the source of truth).
            let out = std::process::Command::new("bash")
                .arg(&script)
                .arg("--tier")
                .arg(tier)
                .current_dir(&repo)
                .output()
                .expect("run backoffice-plan.sh");
            assert!(
                out.status.success(),
                "script failed for {tier}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let script_json: serde_json::Value =
                serde_json::from_slice(&out.stdout).expect("script json");

            // The REAL probe: the rendered unit list matches byte-for-byte (same
            // ids, phases, ordering, live_gated, via_script). This is the canned-vs-
            // real assertion the critique demanded.
            assert_eq!(
                rpc["units"], script_json["units"],
                "RPC units must MATCH backoffice-plan.sh --tier {tier} (not a canned list)"
            );
            // secrets_ready is a bool either way (re-stamped from the Rust probe).
            assert!(rpc["secrets_ready"].is_boolean(), "{reply}");
        }
        match prev_repo {
            Some(v) => std::env::set_var("MCNF_REPO", v),
            None => std::env::remove_var("MCNF_REPO"),
        }
    }

    #[test]
    fn parse_regions_parses_doctl_json() {
        let json = r#"[
            {"slug":"nyc3","name":"New York 3","available":true,"sizes":["s-1vcpu-1gb"]},
            {"slug":"ams2","name":"Amsterdam 2","available":false}
        ]"#;
        let regions = parse_regions(json);
        assert_eq!(
            regions,
            vec![
                ("nyc3".to_string(), "New York 3".to_string(), true),
                ("ams2".to_string(), "Amsterdam 2".to_string(), false),
            ]
        );
    }

    #[test]
    fn parse_regions_garbage_is_empty() {
        assert!(parse_regions("not json at all").is_empty());
        // valid JSON but not an array
        assert!(parse_regions(r#"{"slug":"nyc3"}"#).is_empty());
        // empty array
        assert!(parse_regions("[]").is_empty());
    }

    // ── DATACENTER-18 — New-Mesh genesis ──

    #[test]
    fn genesis_verbs_and_topics_are_registered() {
        assert!(ACTION_VERBS.contains(&"genesis-plan"));
        assert!(ACTION_VERBS.contains(&"genesis-write"));
        assert_eq!(action_topic("genesis-plan"), "action/dc/genesis-plan");
        assert_eq!(action_topic("genesis-write"), "action/dc/genesis-write");
    }

    #[test]
    fn genesis_mesh_id_validation() {
        assert!(genesis_mesh_id_valid("home-mesh").is_ok());
        assert!(genesis_mesh_id_valid("m1").is_ok());
        assert!(genesis_mesh_id_valid("").is_err());
        // uppercase / underscore / space / dot are rejected (DNS-ish label only)
        assert!(genesis_mesh_id_valid("HomeMesh").is_err());
        assert!(genesis_mesh_id_valid("home_mesh").is_err());
        assert!(genesis_mesh_id_valid("home mesh").is_err());
        assert!(genesis_mesh_id_valid("home.mesh").is_err());
        // injection-ish characters rejected
        assert!(genesis_mesh_id_valid("a;rm -rf /").is_err());
        assert!(genesis_mesh_id_valid("a\"b").is_err());
        // leading/trailing hyphen rejected
        assert!(genesis_mesh_id_valid("-mesh").is_err());
        assert!(genesis_mesh_id_valid("mesh-").is_err());
        // too long
        assert!(genesis_mesh_id_valid(&"a".repeat(64)).is_err());
    }

    #[test]
    fn genesis_lighthouse_resource_reuses_dc19_droplet_adds_dns_no_secret() {
        let (addr, hcl) = genesis_lighthouse_resource("home-mesh", "nyc3").unwrap();
        // The droplet half is exactly DC-19's lighthouse resource for lh-<id>-01.
        let (dc19_addr, _) =
            lighthouse_create_resource("lh-home-mesh-01", "nyc3", "s-2vcpu-2gb", "fedora-43-x64")
                .unwrap();
        assert_eq!(addr, dc19_addr);
        assert!(hcl.contains("resource \"digitalocean_droplet\" \"lighthouse_lh_home_mesh_01\""));
        assert!(hcl.contains("name     = \"lh-home-mesh-01\""));
        assert!(hcl.contains("region   = \"nyc3\""));
        // Genesis ADDS the founding DNS A-record.
        assert!(hcl.contains("resource \"digitalocean_record\" \"genesis_lh_home_mesh_01\""));
        assert!(hcl.contains("name   = \"lighthouse-home-mesh\""));
        assert!(hcl.contains(".ipv4_address"));
        // SECRET-HANDLING: no credential material is ever emitted into the HCL.
        let lc = hcl.to_lowercase();
        assert!(
            !lc.contains("token"),
            "HCL must not carry a DO token: {hcl}"
        );
        assert!(
            !lc.contains("passphrase"),
            "HCL must not carry a passphrase"
        );
        assert!(
            !lc.contains("private"),
            "HCL must not carry private key material"
        );
    }

    #[test]
    fn genesis_lighthouse_resource_rejects_invalid_inputs() {
        assert!(genesis_lighthouse_resource("", "nyc3").is_err());
        assert!(genesis_lighthouse_resource("home_mesh", "nyc3").is_err());
        // an invalid region slug is rejected by the reused DC-19 is_slug guard
        assert!(genesis_lighthouse_resource("home-mesh", "NYC3").is_err());
    }

    #[test]
    fn genesis_plan_reports_steps_resource_and_secret_presence() {
        // do_token absent → secrets_ready:false (the wizard warns before a live apply).
        let plan = genesis_plan("home-mesh", "nyc3", false).unwrap();
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["mesh_id"], "home-mesh");
        assert_eq!(plan["region"], "nyc3");
        assert_eq!(plan["workspace"], "zone1-do");
        assert_eq!(plan["path"], "infra/tofu/zone1-do/dc-lighthouses.tf");
        assert_eq!(
            plan["resource"],
            "digitalocean_droplet.lighthouse_lh_home_mesh_01"
        );
        assert_eq!(plan["secrets_ready"], false);
        let steps = plan["steps"].as_array().unwrap();
        assert_eq!(steps.len(), GENESIS_STEPS.len());
        // do_token present → secrets_ready:true.
        let ready = genesis_plan("home-mesh", "nyc3", true).unwrap();
        assert_eq!(ready["secrets_ready"], true);
    }

    #[test]
    fn genesis_plan_reply_validates_inputs() {
        assert!(genesis_plan_reply(None).contains("missing request body"));
        assert!(genesis_plan_reply(Some("not json")).contains("bad json"));
        let bad = json!({ "mesh_id": "Bad_Id", "region": "nyc3" }).to_string();
        assert!(genesis_plan_reply(Some(&bad)).contains("error"));
    }

    #[test]
    fn genesis_write_requires_confirm_true() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp/dc18-noconfirm"));
        // confirm missing
        let body = json!({ "mesh_id": "home-mesh", "region": "nyc3" }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("genesis-write requires confirm:true"), "{r}");
        // confirm false
        let body =
            json!({ "mesh_id": "home-mesh", "region": "nyc3", "confirm": false }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("genesis-write requires confirm:true"), "{r}");
        // confirm as a non-bool string does not satisfy the gate
        let body =
            json!({ "mesh_id": "home-mesh", "region": "nyc3", "confirm": "true" }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("genesis-write requires confirm:true"), "{r}");
    }

    #[test]
    fn genesis_write_lands_tofu_and_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!("dc18-genesis-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let s = DatacenterService::new(tmp.clone());
        let body = json!({ "mesh_id": "home-mesh", "region": "nyc3", "confirm": true }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("\"ok\":true"), "{r}");
        assert!(
            r.contains("digitalocean_droplet.lighthouse_lh_home_mesh_01"),
            "{r}"
        );
        let tf = tmp.join("infra/tofu/zone1-do/dc-lighthouses.tf");
        let written = std::fs::read_to_string(&tf).unwrap();
        assert!(
            written.contains("resource \"digitalocean_droplet\" \"lighthouse_lh_home_mesh_01\"")
        );
        assert!(written.contains("resource \"digitalocean_record\" \"genesis_lh_home_mesh_01\""));
        assert!(written.contains("DATACENTER-18/19"));
        // No credential material reached the file.
        assert!(!written.to_lowercase().contains("token"));
        // A second write for the SAME mesh id is rejected.
        let r2 = build_reply(&s, "genesis-write", Some(&body));
        assert!(r2.contains("already exists"), "{r2}");
        // A DIFFERENT mesh id appends a second pair of blocks.
        let body2 = json!({ "mesh_id": "lab-mesh", "region": "fra1", "confirm": true }).to_string();
        let r3 = build_reply(&s, "genesis-write", Some(&body2));
        assert!(r3.contains("\"ok\":true"), "{r3}");
        let written2 = std::fs::read_to_string(&tf).unwrap();
        assert!(written2.contains("lighthouse_lh_home_mesh_01"));
        assert!(written2.contains("lighthouse_lh_lab_mesh_01"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn genesis_write_rejects_invalid_mesh_id_before_writing() {
        let tmp = std::env::temp_dir().join(format!("dc18-genesis-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let s = DatacenterService::new(tmp.clone());
        let body = json!({ "mesh_id": "Bad_Id", "region": "nyc3", "confirm": true }).to_string();
        let r = build_reply(&s, "genesis-write", Some(&body));
        assert!(r.contains("error"), "{r}");
        assert!(!tmp.join("infra/tofu/zone1-do/dc-lighthouses.tf").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn genesis_plan_takes_no_lock_write_locks_on_mesh_id() {
        assert_eq!(
            lock_key("genesis-plan", Some(r#"{"mesh_id":"home-mesh"}"#)),
            None
        );
        assert_eq!(
            lock_key("genesis-write", Some(r#"{"mesh_id":"home-mesh"}"#)),
            Some("mesh-new:home-mesh".to_string())
        );
        assert_eq!(lock_key("genesis-write", Some(r#"{"mesh_id":""}"#)), None);
        assert_eq!(lock_key("genesis-write", Some("{}")), None);
    }

    #[test]
    fn vm_power_command_maps_each_valid_op() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_power_command(uuid, "start").unwrap(),
            format!("vm-start uuid={uuid}")
        );
        assert_eq!(
            vm_power_command(uuid, "shutdown").unwrap(),
            format!("vm-shutdown uuid={uuid}")
        );
        assert_eq!(
            vm_power_command(uuid, "reboot").unwrap(),
            format!("vm-reboot uuid={uuid}")
        );
    }

    #[test]
    fn vm_power_command_unknown_op_errors() {
        assert!(vm_power_command("abcd-1234", "destroy").is_err());
        assert!(vm_power_command("abcd-1234", "").is_err());
    }

    #[test]
    fn vm_power_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_power_command("", "start").is_err());
        // a `;` to chain a second command
        assert!(vm_power_command("abcd;rm -rf /", "start").is_err());
        // a space (would split into extra args)
        assert!(vm_power_command("abcd 1234", "start").is_err());
        // backtick / non-hex
        assert!(vm_power_command("abcd`whoami`", "start").is_err());
        assert!(vm_power_command("ghij", "start").is_err());
    }

    #[test]
    fn vm_snapshot_command_builds_labelled_snapshot() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_snapshot_command(uuid).unwrap(),
            // name-label uses the first 8 chars of the uuid
            format!("vm-snapshot uuid={uuid} new-name-label=dc-snap-abcd1234")
        );
        // a uuid shorter than 8 chars uses whatever is there (still hex+dash)
        assert_eq!(
            vm_snapshot_command("ab-12").unwrap(),
            "vm-snapshot uuid=ab-12 new-name-label=dc-snap-ab-12"
        );
    }

    #[test]
    fn vm_snapshot_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_snapshot_command("").is_err());
        // a `;` to chain a second command
        assert!(vm_snapshot_command("abcd;rm -rf /").is_err());
        // a space (would split into extra args)
        assert!(vm_snapshot_command("abcd 1234").is_err());
        // backtick / command substitution
        assert!(vm_snapshot_command("abcd`whoami`").is_err());
        // non-hex letters
        assert!(vm_snapshot_command("ghij").is_err());
        // a `=` that could inject an extra xe arg
        assert!(vm_snapshot_command("abcd=evil").is_err());
    }

    #[test]
    fn vm_clone_command_default_label_uses_uuid_prefix() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_clone_command(uuid, None).unwrap(),
            // default name-label uses the first 8 chars of the uuid
            format!("vm-clone uuid={uuid} new-name-label=dc-clone-abcd1234")
        );
        // a uuid shorter than 8 chars uses whatever is there (still hex+dash)
        assert_eq!(
            vm_clone_command("ab-12", None).unwrap(),
            "vm-clone uuid=ab-12 new-name-label=dc-clone-ab-12"
        );
    }

    #[test]
    fn vm_clone_command_uses_sanitized_supplied_name() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_clone_command(uuid, Some("my.vm_clone-1")).unwrap(),
            format!("vm-clone uuid={uuid} new-name-label=my.vm_clone-1")
        );
    }

    #[test]
    fn vm_clone_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_clone_command("", None).is_err());
        // a `;` to chain a second command
        assert!(vm_clone_command("abcd;rm -rf /", None).is_err());
        // a space (would split into extra args)
        assert!(vm_clone_command("abcd 1234", None).is_err());
        // backtick / command substitution
        assert!(vm_clone_command("abcd`whoami`", None).is_err());
        // non-hex letters
        assert!(vm_clone_command("ghij", None).is_err());
    }

    #[test]
    fn vm_clone_command_rejects_unsafe_name() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        // empty name
        assert!(vm_clone_command(uuid, Some("")).is_err());
        // space would split into extra xe args
        assert!(vm_clone_command(uuid, Some("evil name")).is_err());
        // `;` chains a command
        assert!(vm_clone_command(uuid, Some("a;rm -rf /")).is_err());
        // `=` could inject an extra xe arg
        assert!(vm_clone_command(uuid, Some("a=b")).is_err());
        // backtick / command substitution
        assert!(vm_clone_command(uuid, Some("a`whoami`")).is_err());
    }

    #[test]
    fn vm_uninstall_command_builds_forced_uninstall() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_uninstall_command(uuid).unwrap(),
            format!("vm-uninstall uuid={uuid} force=true")
        );
    }

    #[test]
    fn vm_uninstall_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_uninstall_command("").is_err());
        // a `;` to chain a second command
        assert!(vm_uninstall_command("abcd;rm -rf /").is_err());
        // a space (would split into extra args)
        assert!(vm_uninstall_command("abcd 1234").is_err());
        // backtick / command substitution
        assert!(vm_uninstall_command("abcd`whoami`").is_err());
        // non-hex letters
        assert!(vm_uninstall_command("ghij").is_err());
    }

    #[test]
    fn vm_console_command_builds_location_query() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_console_command(uuid).unwrap(),
            format!("console-list vm-uuid={uuid} params=location --minimal")
        );
        // a uuid shorter than 8 chars still works (hex+dash only)
        assert_eq!(
            vm_console_command("ab-12").unwrap(),
            "console-list vm-uuid=ab-12 params=location --minimal"
        );
    }

    #[test]
    fn vm_console_command_rejects_injection_in_uuid() {
        // empty
        assert!(vm_console_command("").is_err());
        // a `;` to chain a second command
        assert!(vm_console_command("abcd;rm -rf /").is_err());
        // a space (would split into extra args)
        assert!(vm_console_command("abcd 1234").is_err());
        // backtick / command substitution
        assert!(vm_console_command("abcd`whoami`").is_err());
        // non-hex letters
        assert!(vm_console_command("ghij").is_err());
        // a `=` that could inject an extra xe arg
        assert!(vm_console_command("abcd=evil").is_err());
    }

    #[test]
    fn vm_console_dom0_not_in_allowed_set_is_rejected() {
        // Read-only, but still SSHes → the dom0 allow-list guard applies. With
        // MCNF_XEN_DOM0S unset the allowed set is empty, so the dom0 is rejected
        // before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-console", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_delete_requires_confirm_true() {
        // The confirm gate is checked BEFORE the dom0 allow-list, so even with an
        // empty allowed set the missing/false confirm is what we observe.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // confirm missing
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
        // confirm false
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": false
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
        // confirm as a non-bool ("true" string) does not satisfy the gate
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": "true"
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("delete requires confirm:true"), "{r}");
    }

    #[test]
    fn vm_delete_confirmed_then_checks_dom0_allow_list() {
        // With confirm:true the gate passes and the dom0 allow-list is the next
        // guard — with MCNF_XEN_DOM0S unset the allowed set is empty, so the
        // (unlisted) dom0 is rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1",
            "confirm": true
        })
        .to_string();
        let r = build_reply(&s, "vm-delete", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_clone_dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-clone", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "vm-power", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-snapshot", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-clone", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-delete", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-console", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-snapshots", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-snapshot-revert", None).contains("missing request body"));
        assert!(build_reply(&s, "vm-snapshot-delete", None).contains("missing request body"));
    }

    #[test]
    fn dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "op": "start",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-power", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_snapshot_dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-snapshot", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    // ---- DATACENTER-6: per-resource op-lock ----

    #[test]
    fn lock_key_maps_mutating_verbs_to_namespaced_resource() {
        let body = json!({ "uuid": "abcd-1234", "op": "start", "dom0": "10.0.0.1" }).to_string();
        // every vm-* mutating verb keys on the vm uuid (namespaced)
        for verb in [
            "vm-power",
            "vm-snapshot",
            "vm-clone",
            "vm-delete",
            "vm-suspend",
            "vm-migrate",
            "vm-resize",
        ] {
            assert_eq!(
                lock_key(verb, Some(&body)),
                Some("vm:abcd-1234".to_string()),
                "verb {verb} should lock on the vm uuid"
            );
        }
        // vm-create has no uuid yet → it locks on the new VM's name.
        let create = json!({ "name": "web-1", "dom0": "10.0.0.1" }).to_string();
        assert_eq!(
            lock_key("vm-create", Some(&create)),
            Some("vm-new:web-1".to_string())
        );
        assert_eq!(lock_key("vm-create", Some(r#"{"name":""}"#)), None);
        assert_eq!(lock_key("vm-create", None), None);
        // the snapshot-mutating verbs key on the SNAPSHOT uuid (a distinct `snap:`
        // namespace), read from the body's `snapshot` field — never the parent VM.
        let snap = json!({ "snapshot": "ffaa-0011", "dom0": "10.0.0.1" }).to_string();
        for verb in ["vm-snapshot-revert", "vm-snapshot-delete"] {
            assert_eq!(
                lock_key(verb, Some(&snap)),
                Some("snap:ffaa-0011".to_string()),
                "verb {verb} should lock on the snapshot uuid"
            );
        }
        assert_eq!(
            lock_key("vm-snapshot-revert", Some(r#"{"snapshot":""}"#)),
            None
        );
        assert_eq!(lock_key("vm-snapshot-delete", None), None);
    }

    #[test]
    fn lock_key_read_only_and_unlockable_return_none() {
        let body = json!({ "uuid": "abcd-1234", "dom0": "10.0.0.1" }).to_string();
        // read-only verbs take no lock
        assert_eq!(lock_key("vm-console", Some(&body)), None);
        assert_eq!(lock_key("vm-snapshots", Some(&body)), None);
        assert_eq!(lock_key("do-regions", Some(&body)), None);
        // unknown verb → no lock
        assert_eq!(lock_key("bogus", Some(&body)), None);
        // mutating verb but nothing to lock on → no lock (the handler emits the
        // real validation error instead of us inventing an empty key)
        assert_eq!(lock_key("vm-power", None), None);
        assert_eq!(lock_key("vm-power", Some("not json")), None);
        assert_eq!(lock_key("vm-power", Some(r#"{"op":"start"}"#)), None);
        assert_eq!(lock_key("vm-power", Some(r#"{"uuid":""}"#)), None);
    }

    #[test]
    fn second_concurrent_mutation_on_same_uuid_is_busy_rejected() {
        // Two concurrent vm-power on the SAME uuid: model the first being still
        // in flight by holding its op-lock guard, then issue the second through
        // build_reply. The second must be rejected with the clear busy reason,
        // and crucially WITHOUT reaching the dom0 allow-list (the lock is the
        // first gate). A different uuid is unaffected.
        let _env = lock_env();
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";

        // First op still running: hold the guard for vm:<uuid>.
        let held = s
            .try_lock(format!("vm:{uuid}"))
            .expect("first claim succeeds on a free resource");

        // Second vm-power on the same uuid → busy-reject, NOT the dom0 error.
        let body = json!({ "uuid": uuid, "op": "start", "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "vm-power", Some(&body));
        assert!(
            r.contains(&format!("resource vm:{uuid} busy")),
            "expected busy reject, got: {r}"
        );
        assert!(
            r.contains("a vm-power is already in flight"),
            "expected the clear reason, got: {r}"
        );
        assert!(
            !r.contains("dom0 not in allowed set"),
            "lock must gate BEFORE the dom0 check: {r}"
        );

        // A DIFFERENT uuid is not locked → it proceeds to the next gate (the
        // empty dom0 allow-list), proving the lock is per-resource.
        let other = json!({
            "uuid": "ffff0000-1111-2222-3333-444455556666",
            "op": "start",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r2 = build_reply(&s, "vm-power", Some(&other));
        assert!(r2.contains("dom0 not in allowed set"), "{r2}");

        // Release the first op; the same uuid is now claimable again.
        drop(held);
        assert!(
            s.try_lock(format!("vm:{uuid}")).is_some(),
            "the resource is free again after the first op completes"
        );
    }

    #[test]
    fn op_lock_releases_after_a_completed_dispatch() {
        // build_reply's guard is dropped when it returns, so back-to-back (not
        // overlapping) mutations on the same uuid both run — the lock only blocks
        // CONCURRENT ones. With an empty dom0 set both hit the allow-list error,
        // proving neither was spuriously busy-rejected.
        let _env = lock_env();
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "op": "start",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r1 = build_reply(&s, "vm-power", Some(&body));
        assert!(r1.contains("dom0 not in allowed set"), "{r1}");
        let r2 = build_reply(&s, "vm-power", Some(&body));
        assert!(
            r2.contains("dom0 not in allowed set"),
            "lock must have released after the first call: {r2}"
        );
    }

    #[test]
    fn op_lock_is_shared_across_cloned_handles() {
        // The responder thread gets a Clone of the service; the op-lock must be
        // shared (Arc), so a resource claimed on one handle is busy on its clone.
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        let _held = s.try_lock(format!("vm:{uuid}")).expect("claim on original");
        let clone = s.clone();
        let body = json!({ "uuid": uuid, "op": "reboot", "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&clone, "vm-power", Some(&body));
        assert!(
            r.contains(&format!("resource vm:{uuid} busy")),
            "a clone must see the same in-flight set: {r}"
        );
    }

    #[test]
    fn read_only_verb_is_never_busy_rejected() {
        // vm-console is read-only: even with the same uuid "in flight" it is not
        // gated by the lock (concurrent reads are fine). It falls through to its
        // own dom0 allow-list check.
        let _env = lock_env();
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        let _held = s.try_lock(format!("vm:{uuid}")).expect("claim the uuid");
        let body = json!({ "uuid": uuid, "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "vm-console", Some(&body));
        assert!(
            !r.contains("busy"),
            "read-only verb must not be locked: {r}"
        );
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    // DATACENTER-11 — suspend / migrate / resize / create command builders -------

    #[test]
    fn vm_suspend_command_maps_each_valid_op() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_suspend_command(uuid, "suspend").unwrap(),
            format!("vm-suspend uuid={uuid}")
        );
        assert_eq!(
            vm_suspend_command(uuid, "resume").unwrap(),
            format!("vm-resume uuid={uuid}")
        );
    }

    #[test]
    fn vm_suspend_command_rejects_bad_op_and_injection() {
        assert!(vm_suspend_command("abcd-1234", "shutdown").is_err());
        assert!(vm_suspend_command("abcd-1234", "").is_err());
        assert!(vm_suspend_command("", "suspend").is_err());
        assert!(vm_suspend_command("abcd;rm -rf /", "suspend").is_err());
        assert!(vm_suspend_command("abcd`whoami`", "suspend").is_err());
        assert!(vm_suspend_command("ghij", "suspend").is_err());
    }

    #[test]
    fn vm_migrate_command_builds_live_migrate() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_migrate_command(uuid, "xcp-big").unwrap(),
            format!("vm-migrate uuid={uuid} host=xcp-big live=true")
        );
    }

    #[test]
    fn vm_migrate_command_rejects_injection() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert!(vm_migrate_command("", "h").is_err());
        assert!(vm_migrate_command("abcd;rm", "h").is_err());
        assert!(vm_migrate_command(uuid, "").is_err());
        assert!(vm_migrate_command(uuid, "host;rm -rf /").is_err());
        assert!(vm_migrate_command(uuid, "host name").is_err());
        assert!(vm_migrate_command(uuid, "host`whoami`").is_err());
        // a uuid-form host (host-uuid migration) is allowed.
        assert!(vm_migrate_command(uuid, "11112222-3333").is_ok());
    }

    #[test]
    fn vm_resize_commands_build_vcpu_and_memory_sets() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        let cmds = vm_resize_commands(uuid, 4, 2048).unwrap();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], format!("vm-param-set uuid={uuid} VCPUs-max=4"));
        assert_eq!(
            cmds[1],
            format!("vm-param-set uuid={uuid} VCPUs-at-startup=4")
        );
        // 2048 MiB → 2147483648 bytes, pinned across all four limits.
        assert!(cmds[2].contains("static-max=2147483648"));
        assert!(cmds[2].contains("dynamic-max=2147483648"));
    }

    #[test]
    fn vm_resize_commands_bounds_check() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert!(vm_resize_commands(uuid, 0, 2048).is_err());
        assert!(vm_resize_commands(uuid, u64::from(RESIZE_MAX_VCPUS) + 1, 2048).is_err());
        assert!(vm_resize_commands(uuid, 4, 0).is_err());
        assert!(vm_resize_commands(uuid, 4, RESIZE_MAX_MEM_MIB + 1).is_err());
        assert!(vm_resize_commands("bad uuid", 4, 2048).is_err());
        assert!(vm_resize_commands(uuid, RESIZE_MAX_VCPUS.into(), RESIZE_MAX_MEM_MIB).is_ok());
    }

    #[test]
    fn vm_create_resource_emits_valid_hcl() {
        let (addr, hcl) =
            vm_create_resource("web-1", "MDE-VM-golden", 4, 4096, "420c5872-dd49").unwrap();
        assert_eq!(addr, "xenserver_vm.dc_web_1");
        assert!(hcl.contains("resource \"xenserver_vm\" \"dc_web_1\""));
        assert!(hcl.contains("name_label        = \"web-1\""));
        assert!(hcl.contains("template_name     = \"MDE-VM-golden\""));
        assert!(hcl.contains("vcpus             = 4"));
        // 4096 MiB → 4294967296 bytes.
        assert!(hcl.contains("static_mem_max    = 4294967296"));
        assert!(hcl.contains("network_uuid = \"420c5872-dd49\""));
        assert!(hcl.contains("ignore_changes"));
    }

    #[test]
    fn vm_create_resource_rejects_unsafe_fields() {
        assert!(vm_create_resource("", "g", 4, 4096, "abcd").is_err());
        assert!(vm_create_resource("a b", "g", 4, 4096, "abcd").is_err());
        assert!(vm_create_resource("a;rm", "g", 4, 4096, "abcd").is_err());
        assert!(vm_create_resource("ok", "", 4, 4096, "abcd").is_err());
        assert!(vm_create_resource("ok", "g h", 4, 4096, "abcd").is_err());
        assert!(vm_create_resource("ok", "g", 4, 4096, "").is_err());
        assert!(vm_create_resource("ok", "g", 4, 4096, "net;rm").is_err());
        assert!(vm_create_resource("ok", "g", 0, 4096, "abcd").is_err());
        assert!(vm_create_resource("ok", "g", 4, 0, "abcd").is_err());
    }

    #[test]
    fn vm_create_reply_writes_a_tofu_resource_and_rejects_a_dup() {
        let _env = lock_env();
        // The dom0 allow-list comes from env (default-empty in tests), so point it
        // at a known dom0 for the duration of this test.
        let prev = std::env::var_os("MCNF_XEN_DOM0S");
        std::env::set_var("MCNF_XEN_DOM0S", "10.9.9.9");

        let tmp = tempfile::tempdir().unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        let body = json!({
            "name": "web-1",
            "vcpus": 4,
            "mem_mib": 4096,
            "network_uuid": "420c5872-dd49",
            "dom0": "10.9.9.9"
        })
        .to_string();
        let r = build_reply(&svc, "vm-create", Some(&body));
        assert!(r.contains("\"ok\":true"), "expected ok, got: {r}");
        assert!(r.contains("xenserver_vm.dc_web_1"), "{r}");

        // The generated file exists and carries the block + the one-time header.
        let tf = std::fs::read_to_string(tmp.path().join("infra/tofu/xen-xapi/dc-vms.tf")).unwrap();
        assert!(tf.contains("DATACENTER-11"));
        assert!(tf.contains("resource \"xenserver_vm\" \"dc_web_1\""));

        // A second create of the SAME name is rejected (no silent overwrite).
        let r2 = build_reply(&svc, "vm-create", Some(&body));
        assert!(r2.contains("already exists"), "expected dup reject: {r2}");

        match prev {
            Some(v) => std::env::set_var("MCNF_XEN_DOM0S", v),
            None => std::env::remove_var("MCNF_XEN_DOM0S"),
        }
    }

    #[test]
    fn vm_create_reply_rejects_a_dom0_outside_the_allow_list() {
        let _env = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        // An empty/unset allow-list → no dom0 is allowed → reject before any write.
        let body = json!({
            "name": "web-1",
            "vcpus": 4,
            "mem_mib": 4096,
            "network_uuid": "abcd",
            "dom0": "1.2.3.4"
        })
        .to_string();
        let r = build_reply(&svc, "vm-create", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
        assert!(!tmp.path().join("infra/tofu/xen-xapi/dc-vms.tf").exists());
    }

    // DATACENTER-19 — the guided new-lighthouse flow: spread recommendation, the
    // do-regions parse, and the Tofu-droplet-resource write. --------------------

    #[test]
    fn recommend_spread_prefers_a_new_geo() {
        let available = vec![
            "nyc1".to_string(),
            "nyc3".to_string(),
            "sfo3".to_string(),
            "fra1".to_string(),
        ];
        // Lighthouses already sit in nyc3 (geo `nyc`). The pick must skip every
        // `nyc*` region and land on the first region of a NEW geo (sfo).
        let used = vec!["nyc3".to_string()];
        assert_eq!(
            recommend_spread_region(&available, &used),
            Some("sfo3".to_string())
        );
    }

    #[test]
    fn recommend_spread_never_recommends_a_used_region() {
        let available = vec!["nyc3".to_string(), "sfo3".to_string()];
        let used = vec!["nyc3".to_string(), "sfo3".to_string()];
        // Every available region already hosts a lighthouse → no spread to add.
        assert_eq!(recommend_spread_region(&available, &used), None);
    }

    #[test]
    fn recommend_spread_falls_back_to_an_unused_region_when_all_geos_taken() {
        // Both geos (nyc, sfo) are occupied, but a SECOND nyc region (nyc1) is free.
        // No new geo is available, so the fallback picks the unused nyc1 (a distinct
        // failure domain still beats recommending a region already in use).
        let available = vec!["nyc1".to_string(), "nyc3".to_string(), "sfo3".to_string()];
        let used = vec!["nyc3".to_string(), "sfo3".to_string()];
        assert_eq!(
            recommend_spread_region(&available, &used),
            Some("nyc1".to_string())
        );
    }

    #[test]
    fn recommend_spread_with_no_used_picks_the_first_available() {
        let available = vec!["ams3".to_string(), "nyc3".to_string()];
        // No existing lighthouses → any region adds spread; deterministic first pick.
        assert_eq!(
            recommend_spread_region(&available, &[]),
            Some("ams3".to_string())
        );
        // Empty universe → nothing to recommend.
        assert_eq!(recommend_spread_region(&[], &["nyc3".to_string()]), None);
    }

    #[test]
    fn lighthouse_create_resource_emits_valid_hcl() {
        let (addr, hcl) =
            lighthouse_create_resource("lighthouse-04", "sfo3", "s-2vcpu-2gb", "fedora-43-x64")
                .unwrap();
        assert_eq!(addr, "digitalocean_droplet.lighthouse_lighthouse_04");
        assert!(hcl.contains("resource \"digitalocean_droplet\" \"lighthouse_lighthouse_04\""));
        assert!(hcl.contains("name     = \"lighthouse-04\""));
        assert!(hcl.contains("region   = \"sfo3\""));
        assert!(hcl.contains("size     = \"s-2vcpu-2gb\""));
        assert!(hcl.contains("image    = \"fedora-43-x64\""));
        assert!(hcl.contains("tags     = [\"magic-lighthouse\"]"));
        assert!(hcl.contains("digitalocean_ssh_key.mackes_mesh_claude.id"));
    }

    #[test]
    fn lighthouse_create_resource_rejects_unsafe_fields() {
        assert!(lighthouse_create_resource("", "sfo3", "s", "f").is_err());
        assert!(lighthouse_create_resource("a b", "sfo3", "s", "f").is_err());
        assert!(lighthouse_create_resource("a;rm", "sfo3", "s", "f").is_err());
        // A region/size/image must be a lowercase slug — uppercase / shell metachars
        // / spaces are rejected before they reach the HCL.
        assert!(lighthouse_create_resource("ok", "SFO3", "s", "f").is_err());
        assert!(lighthouse_create_resource("ok", "sfo3;rm", "s", "f").is_err());
        assert!(lighthouse_create_resource("ok", "", "s", "f").is_err());
        assert!(lighthouse_create_resource("ok", "sfo3", "s 1", "f").is_err());
        assert!(lighthouse_create_resource("ok", "sfo3", "s", "F").is_err());
    }

    #[test]
    fn lighthouse_create_reply_writes_a_tofu_resource_and_rejects_a_dup() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        // Only name + region are required; size/image default to the lighthouse slugs.
        let body = json!({ "name": "lighthouse-04", "region": "sfo3" }).to_string();
        let r = build_reply(&svc, "lighthouse-create", Some(&body));
        assert!(r.contains("\"ok\":true"), "expected ok, got: {r}");
        assert!(
            r.contains("digitalocean_droplet.lighthouse_lighthouse_04"),
            "{r}"
        );

        // The generated file exists and carries the block + the one-time header, and
        // the defaulted size/image slugs.
        let tf = std::fs::read_to_string(tmp.path().join("infra/tofu/zone1-do/dc-lighthouses.tf"))
            .unwrap();
        assert!(tf.contains("DATACENTER-19"));
        assert!(tf.contains("resource \"digitalocean_droplet\" \"lighthouse_lighthouse_04\""));
        assert!(tf.contains("region   = \"sfo3\""));
        assert!(tf.contains("size     = \"s-2vcpu-2gb\""));
        assert!(tf.contains("image    = \"fedora-43-x64\""));

        // A second create of the SAME name is rejected (no silent overwrite).
        let r2 = build_reply(&svc, "lighthouse-create", Some(&body));
        assert!(r2.contains("already exists"), "expected dup reject: {r2}");
    }

    #[test]
    fn lighthouse_create_reply_rejects_a_bad_region() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DatacenterService::new(tmp.path().to_path_buf());
        let body = json!({ "name": "lighthouse-04", "region": "SFO3;rm" }).to_string();
        let r = build_reply(&svc, "lighthouse-create", Some(&body));
        assert!(r.contains("region is not a valid slug"), "{r}");
        // No file is written on a validation failure.
        assert!(!tmp
            .path()
            .join("infra/tofu/zone1-do/dc-lighthouses.tf")
            .exists());
    }

    #[test]
    fn lighthouse_create_lock_key_is_name_scoped() {
        let body = json!({ "name": "lighthouse-04", "region": "sfo3" }).to_string();
        assert_eq!(
            lock_key("lighthouse-create", Some(&body)),
            Some("lighthouse-new:lighthouse-04".to_string())
        );
        // An empty/missing name → no lock (the per-verb handler produces the error).
        assert_eq!(lock_key("lighthouse-create", Some(r#"{"name":""}"#)), None);
        assert_eq!(lock_key("lighthouse-create", Some("{}")), None);
    }

    // DATACENTER-11 — snapshot list / revert / delete command builders + replies --

    #[test]
    fn vm_snapshots_command_builds_listing_idiom() {
        let uuid = "abcd1234-5678-90ab-cdef-1234567890ab";
        let cmd = vm_snapshots_command(uuid).unwrap();
        // It runs `xe` internally per snapshot, so it is NOT `xe`-prefixed; it
        // filters the snapshots to this VM and emits `<uuid>|<name>` per line.
        assert!(cmd.starts_with("for s in $("), "{cmd}");
        assert!(cmd.contains(&format!(
            "snapshot-list snapshot-of={uuid} params=uuid --minimal"
        )));
        assert!(cmd.contains("echo \"$s|$(xe snapshot-param-get uuid=$s param-name=name-label)\""));
    }

    #[test]
    fn vm_snapshots_command_rejects_injection_in_uuid() {
        assert!(vm_snapshots_command("").is_err());
        assert!(vm_snapshots_command("abcd;rm -rf /").is_err());
        assert!(vm_snapshots_command("abcd 1234").is_err());
        assert!(vm_snapshots_command("abcd`whoami`").is_err());
        assert!(vm_snapshots_command("ghij").is_err());
    }

    #[test]
    fn parse_xe_snapshots_pairs_uuid_and_name() {
        let out = "aaaa-1111|nightly-2026-06-20\nbbbb-2222|dc-snap-abcd1234\n";
        assert_eq!(
            parse_xe_snapshots(out),
            vec![
                ("aaaa-1111".to_string(), "nightly-2026-06-20".to_string()),
                ("bbbb-2222".to_string(), "dc-snap-abcd1234".to_string()),
            ]
        );
        // blank lines, missing-delimiter lines, and an empty-uuid line are dropped.
        let messy = "\ncccc-3333|named\nno-delimiter-line\n|orphan-name\n";
        assert_eq!(
            parse_xe_snapshots(messy),
            vec![("cccc-3333".to_string(), "named".to_string())]
        );
        // a VM with no snapshots → empty.
        assert!(parse_xe_snapshots("").is_empty());
    }

    #[test]
    fn vm_snapshot_revert_command_builds_revert() {
        let snap = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_snapshot_revert_command(snap).unwrap(),
            format!("snapshot-revert snapshot-uuid={snap}")
        );
    }

    #[test]
    fn vm_snapshot_revert_command_rejects_injection() {
        assert!(vm_snapshot_revert_command("").is_err());
        assert!(vm_snapshot_revert_command("abcd;rm -rf /").is_err());
        assert!(vm_snapshot_revert_command("abcd 1234").is_err());
        assert!(vm_snapshot_revert_command("abcd`whoami`").is_err());
        assert!(vm_snapshot_revert_command("abcd=evil").is_err());
        assert!(vm_snapshot_revert_command("ghij").is_err());
    }

    #[test]
    fn vm_snapshot_delete_command_builds_destroy() {
        let snap = "abcd1234-5678-90ab-cdef-1234567890ab";
        assert_eq!(
            vm_snapshot_delete_command(snap).unwrap(),
            format!("snapshot-destroy uuid={snap}")
        );
    }

    #[test]
    fn vm_snapshot_delete_command_rejects_injection() {
        assert!(vm_snapshot_delete_command("").is_err());
        assert!(vm_snapshot_delete_command("abcd;rm -rf /").is_err());
        assert!(vm_snapshot_delete_command("abcd 1234").is_err());
        assert!(vm_snapshot_delete_command("abcd`whoami`").is_err());
        assert!(vm_snapshot_delete_command("abcd=evil").is_err());
        assert!(vm_snapshot_delete_command("ghij").is_err());
    }

    #[test]
    fn vm_snapshots_dom0_not_in_allowed_set_is_rejected() {
        // Read-only, but still SSHes → the dom0 allow-list guard applies. With
        // MCNF_XEN_DOM0S unset the allowed set is empty, so the dom0 is rejected.
        let _env = lock_env();
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "uuid": "abcd1234-5678-90ab-cdef-1234567890ab",
            "dom0": "10.0.0.1"
        })
        .to_string();
        let r = build_reply(&s, "vm-snapshots", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_snapshot_revert_dom0_not_in_allowed_set_is_rejected() {
        let _env = lock_env();
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "snapshot": "ffaa-0011", "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "vm-snapshot-revert", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn vm_snapshot_delete_requires_confirm_true() {
        // The confirm gate is checked BEFORE the dom0 allow-list, so even with an
        // empty allowed set the missing/false confirm is what we observe (mirrors
        // vm-delete).
        let _env = lock_env();
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        // confirm missing
        let body = json!({ "snapshot": "ffaa-0011", "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "vm-snapshot-delete", Some(&body));
        assert!(r.contains("snapshot-delete requires confirm:true"), "{r}");
        // confirm false
        let body =
            json!({ "snapshot": "ffaa-0011", "dom0": "10.0.0.1", "confirm": false }).to_string();
        let r = build_reply(&s, "vm-snapshot-delete", Some(&body));
        assert!(r.contains("snapshot-delete requires confirm:true"), "{r}");
        // confirm as a non-bool ("true" string) does not satisfy the gate
        let body =
            json!({ "snapshot": "ffaa-0011", "dom0": "10.0.0.1", "confirm": "true" }).to_string();
        let r = build_reply(&s, "vm-snapshot-delete", Some(&body));
        assert!(r.contains("snapshot-delete requires confirm:true"), "{r}");
    }

    #[test]
    fn vm_snapshot_delete_confirmed_then_checks_dom0_allow_list() {
        // With confirm:true the gate passes and the dom0 allow-list is the next
        // guard — with MCNF_XEN_DOM0S unset the allowed set is empty, so the
        // (unlisted) dom0 is rejected before any SSH is attempted.
        let _env = lock_env();
        let s = DatacenterService::new(std::path::PathBuf::from("/tmp"));
        let body =
            json!({ "snapshot": "ffaa-0011", "dom0": "10.0.0.1", "confirm": true }).to_string();
        let r = build_reply(&s, "vm-snapshot-delete", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }
}
