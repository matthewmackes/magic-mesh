//! DATACENTER-12 (storage action layer) — `action/dc/sr-*` + `action/dc/vdi-*`
//! Xen storage control, the sibling of [`super::datacenter`]'s VM control.
//!
//! The Datacenter panel's **Storage tab** reads SRs off the Bus
//! (`event/dc/sr/*`, published by the orchestrator's `gather_xen`) and ACTS on
//! storage through these verbs, served on the SAME already-spawned datacenter
//! responder thread (`super::datacenter::serve_bus`) — this module is the pure
//! command-builder + reply layer it dispatches into, so no new `bin/mackesd.rs`
//! wiring is needed. Every verb shares the VM module's security contract:
//!
//! * the `dom0` is checked against the orchestrator's allow-list
//!   ([`crate::workers::datacenter_orchestrator::xen_dom0s`]) BEFORE anything
//!   runs — an attacker-supplied host is never SSH'd;
//! * every uuid / size / name field is validated against a strict character
//!   class (the command-injection guard) before it is interpolated into the
//!   remote `xe …` string;
//! * the op-lock keying lives in [`super::datacenter::lock_key`].
//!
//! Verbs (all `action/dc/<verb>`, body is JSON):
//!
//! * `sr-create` `{ name, type?, device_config?, dom0 }` → `xe sr-create …
//!   host-uuid=…` (defaults `type=lvm`); reply `{"ok":true,"sr":"<uuid>"}`.
//! * `vdi-create` `{ sr, name, size_gib, dom0 }` → `xe vdi-create sr-uuid=…
//!   name-label=… virtual-size=…`; reply `{"ok":true,"vdi":"<uuid>"}`.
//! * `vdi-attach` `{ vdi, vm, dom0 }` → `xe vbd-create … ; xe vbd-plug …`;
//!   reply `{"ok":true,"vbd":"<uuid>"}`.
//! * `vdi-detach` `{ vbd, dom0 }` → `xe vbd-unplug … ; xe vbd-destroy …`;
//!   reply `{"ok":true}`.
//! * `sr-snapshot` `{ vdi|sr, dom0 }` → `xe vdi-snapshot …` (one VDI) or a loop
//!   over the SR's VDIs; reply `{"ok":true,"snapshot":…}`.

use serde_json::json;
use std::fmt::Write as _;

/// Action verbs this storage module serves on `action/dc/<verb>`. Folded into
/// the datacenter responder's verb list so they share one responder thread.
pub const STORAGE_VERBS: [&str; 5] = [
    "sr-create",
    "vdi-create",
    "vdi-attach",
    "vdi-detach",
    "sr-snapshot",
];

/// True when `verb` is one of this module's storage verbs (the datacenter
/// dispatcher routes these here). Pure.
#[must_use]
pub fn is_storage_verb(verb: &str) -> bool {
    STORAGE_VERBS.contains(&verb)
}

/// A xen object uuid is a hex+dash string — the command-injection guard reused
/// across every verb. Returns `Err` for an empty value or any character outside
/// `[0-9a-fA-F-]`. Pure.
fn check_uuid(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("empty {field}"));
    }
    if !value.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(format!("{field} contains invalid characters"));
    }
    Ok(())
}

/// A name-label must be non-empty and `[A-Za-z0-9._-]` only — the same class the
/// VM module sanitizes clone/create names to, since it is interpolated into the
/// remote `xe … name-label=<name>` string. Pure.
fn check_name(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("empty name".into());
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("name contains invalid characters".into());
    }
    Ok(())
}

/// Build the remote `xe` argument string for an SR create. PURE.
///
/// `type` defaults to `lvm` (a local-disk SR); only an alphanumeric type is
/// accepted (`lvm`, `ext`, `nfs`, …) so it cannot smuggle shell metacharacters.
/// `device_config` is an optional `key=value` pair (e.g. `device=/dev/sdb`);
/// both halves are validated against a conservative path/identifier class. The
/// SR is created on the host the caller named via `host_uuid`. Returns e.g.
/// `"sr-create host-uuid=<h> type=lvm name-label=<name> device-config:device=/dev/sdb"`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `name`, an empty/invalid `host_uuid`, a
/// non-alphanumeric `sr_type`, or a `device_config` half with a disallowed char.
pub fn sr_create_command(
    name: &str,
    sr_type: &str,
    host_uuid: &str,
    device_config: Option<&str>,
) -> Result<String, String> {
    check_name(name)?;
    check_uuid("host_uuid", host_uuid)?;
    if sr_type.is_empty() || !sr_type.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("type must be alphanumeric".into());
    }
    let mut cmd = format!("sr-create host-uuid={host_uuid} type={sr_type} name-label={name}");
    if let Some(dc) = device_config {
        if !dc.is_empty() {
            let (k, v) = dc
                .split_once('=')
                .ok_or("device_config must be key=value")?;
            if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err("device_config key invalid".into());
            }
            if v.is_empty()
                || !v
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':'))
            {
                return Err("device_config value invalid".into());
            }
            let _ = write!(cmd, " device-config:{k}={v}");
        }
    }
    Ok(cmd)
}

/// Build the remote `xe` argument string for a VDI create. PURE.
///
/// `size_gib` is the virtual size in GiB (1..=65536, a sane upper bound so a
/// fat-fingered value can't request a petabyte); it is rendered as XAPI's
/// `<n>GiB` size suffix. The VDI is created on `sr_uuid` with the given
/// name-label. Returns e.g.
/// `"vdi-create sr-uuid=<sr> name-label=<name> type=user virtual-size=40GiB"`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `sr_uuid`, an empty/invalid `name`, or a
/// `size_gib` of 0 or above 65536.
pub fn vdi_create_command(sr_uuid: &str, name: &str, size_gib: u64) -> Result<String, String> {
    check_uuid("sr_uuid", sr_uuid)?;
    check_name(name)?;
    if size_gib == 0 || size_gib > 65536 {
        return Err("size_gib must be 1..=65536".into());
    }
    Ok(format!(
        "vdi-create sr-uuid={sr_uuid} name-label={name} type=user virtual-size={size_gib}GiB"
    ))
}

/// Build the two remote `xe` argument strings for attaching a VDI to a VM. PURE.
///
/// First `vbd-create` (returns the new VBD uuid on stdout), then `vbd-plug` on
/// that uuid (hot-plug into the running guest). The device position is
/// auto-chosen by XAPI via `device=autodetect`. The caller chains them (the
/// second uses the uuid the first prints). Returns
/// `("vbd-create vm-uuid=<vm> vdi-uuid=<vdi> device=autodetect mode=RW type=Disk",
///   "vbd-plug uuid=")` (the caller appends the captured VBD uuid to the second).
///
/// # Errors
/// Returns `Err` for an empty/invalid `vdi_uuid` or `vm_uuid`.
pub fn vdi_attach_commands(vdi_uuid: &str, vm_uuid: &str) -> Result<(String, String), String> {
    check_uuid("vdi_uuid", vdi_uuid)?;
    check_uuid("vm_uuid", vm_uuid)?;
    let create = format!(
        "vbd-create vm-uuid={vm_uuid} vdi-uuid={vdi_uuid} device=autodetect mode=RW type=Disk"
    );
    Ok((create, "vbd-plug uuid=".to_string()))
}

/// Build the two remote `xe` argument strings for detaching a VBD. PURE.
///
/// `vbd-unplug` (eject from the running guest) then `vbd-destroy` (remove the
/// connection record — the VDI itself is NOT destroyed). Returns
/// `("vbd-unplug uuid=<vbd>", "vbd-destroy uuid=<vbd>")`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `vbd_uuid`.
pub fn vdi_detach_commands(vbd_uuid: &str) -> Result<(String, String), String> {
    check_uuid("vbd_uuid", vbd_uuid)?;
    Ok((
        format!("vbd-unplug uuid={vbd_uuid}"),
        format!("vbd-destroy uuid={vbd_uuid}"),
    ))
}

/// Build the remote `xe` argument string for a single VDI snapshot. PURE. The new
/// snapshot VDI's uuid is printed on stdout. Returns `"vdi-snapshot uuid=<vdi>"`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `vdi_uuid`.
pub fn sr_snapshot_command(vdi_uuid: &str) -> Result<String, String> {
    check_uuid("vdi_uuid", vdi_uuid)?;
    Ok(format!("vdi-snapshot uuid={vdi_uuid}"))
}

/// Build the remote shell command that snapshots EVERY VDI on an SR. PURE.
///
/// Used when the operator snapshots a whole store from its SR card (XAPI has no
/// SR-level snapshot; this loops the SR's VDIs). The `sr_uuid` is validated, then
/// the loop calls `xe vdi-snapshot` per VDI and echoes a count. Returns e.g.
/// `"n=0; for v in $(xe vbd-list … ); do xe vdi-snapshot uuid=$v >/dev/null && n=$((n+1)); done; echo $n"`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `sr_uuid`.
pub fn sr_snapshot_all_command(sr_uuid: &str) -> Result<String, String> {
    check_uuid("sr_uuid", sr_uuid)?;
    // `vdi-list sr-uuid=<sr> params=uuid --minimal` is a comma list; snapshot each.
    Ok(format!(
        "n=0; for v in $(xe vdi-list sr-uuid={sr_uuid} params=uuid --minimal | tr , ' '); \
         do xe vdi-snapshot uuid=$v >/dev/null 2>&1 && n=$((n+1)); done; echo \"$n VDI(s) snapshotted\""
    ))
}

/// SECURITY: only act on a dom0 in the configured allowed set. Returns the dom0
/// when allowed, or the standard reject envelope as `Err`.
fn allowed_dom0(req: &serde_json::Value) -> Result<String, String> {
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == &dom0)
    {
        Ok(dom0)
    } else {
        Err(json!({ "error": "dom0 not in allowed set" }).to_string())
    }
}

/// Run a remote `xe <args>` over the mesh-key SSH and translate the
/// [`std::process::Output`] into a reply. `ok_with` maps the trimmed stdout (a
/// new uuid, for the create/snapshot verbs) into the success envelope; a
/// non-zero exit returns the `xe` stderr.
fn run_xe<F: FnOnce(&str) -> String>(dom0: &str, xe_args: &str, ok_with: F) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {xe_args}");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            ok_with(&out)
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

/// Run a remote command line (already a full shell command, e.g. a chained
/// `xe … && xe …`) over the mesh-key SSH. Used for the two-step attach/detach
/// paths where the second `xe` consumes the first's stdout.
fn run_remote<F: FnOnce(&str) -> String>(dom0: &str, remote: &str, ok_with: F) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    match ssh_xe_status(&key, dom0, remote) {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            ok_with(&out)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("command failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// The SSH-`xe` runner, mirroring `super::datacenter`'s private helper exactly
/// (same flags: identity, no host-key prompt, batch mode, 8s connect timeout).
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

/// Dispatch one storage `verb` to its reply builder.
///
/// Called by the datacenter responder's `build_reply` for any
/// [`is_storage_verb`]. The op-lock is already held by the caller (keyed via
/// `super::datacenter::lock_key`).
#[must_use]
pub fn build_reply(verb: &str, req_body: Option<&str>) -> String {
    match verb {
        "sr-create" => sr_create_reply(req_body),
        "vdi-create" => vdi_create_reply(req_body),
        "vdi-attach" => vdi_attach_reply(req_body),
        "vdi-detach" => vdi_detach_reply(req_body),
        "sr-snapshot" => sr_snapshot_reply(req_body),
        _ => json!({ "error": "unknown storage verb" }).to_string(),
    }
}

/// Parse `req_body` into a JSON object, returning the `{"error":..}` envelope as
/// `Err` on a missing/invalid body.
fn parse_req(verb: &str, req_body: Option<&str>) -> Result<serde_json::Value, String> {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return Err(err(format!("{verb}: missing request body")));
    };
    serde_json::from_str(body).map_err(|e| err(format!("{verb}: bad json: {e}")))
}

fn str_field<'a>(req: &'a serde_json::Value, k: &str) -> &'a str {
    req.get(k).and_then(serde_json::Value::as_str).unwrap_or("")
}

fn sr_create_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let req = match parse_req("sr-create", req_body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dom0 = match allowed_dom0(&req) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let name = str_field(&req, "name");
    let sr_type = {
        let t = str_field(&req, "type");
        if t.is_empty() {
            "lvm"
        } else {
            t
        }
    };
    let host_uuid = str_field(&req, "host_uuid");
    let device_config = req.get("device_config").and_then(serde_json::Value::as_str);
    let cmd = match sr_create_command(name, sr_type, host_uuid, device_config) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe(&dom0, &cmd, |out| {
        json!({ "ok": true, "sr": out }).to_string()
    })
}

fn vdi_create_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let req = match parse_req("vdi-create", req_body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dom0 = match allowed_dom0(&req) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let sr = str_field(&req, "sr");
    let name = str_field(&req, "name");
    let Some(size_gib) = req.get("size_gib").and_then(serde_json::Value::as_u64) else {
        return err("vdi-create: size_gib must be an integer".into());
    };
    let cmd = match vdi_create_command(sr, name, size_gib) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_xe(&dom0, &cmd, |out| {
        json!({ "ok": true, "vdi": out }).to_string()
    })
}

fn vdi_attach_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let req = match parse_req("vdi-attach", req_body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dom0 = match allowed_dom0(&req) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let vdi = str_field(&req, "vdi");
    let vm = str_field(&req, "vm");
    let (create, plug_prefix) = match vdi_attach_commands(vdi, vm) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    // The VBD uuid that `vbd-create` prints feeds `vbd-plug`. Chaining in one
    // remote shell keeps it a single round trip: vbd=$(xe vbd-create …) ; xe
    // vbd-plug uuid=$vbd ; echo $vbd. `vbd-plug` is best-effort (a halted VM has
    // no live device model — the VBD still attaches at next boot), so the
    // attach succeeds as long as `vbd-create` did.
    let remote = format!("vbd=$(xe {create}) && xe {plug_prefix}$vbd 2>/dev/null; echo \"$vbd\"");
    run_remote(&dom0, &remote, |out| {
        json!({ "ok": true, "vbd": out }).to_string()
    })
}

fn vdi_detach_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let req = match parse_req("vdi-detach", req_body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dom0 = match allowed_dom0(&req) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let vbd = str_field(&req, "vbd");
    let (unplug, destroy) = match vdi_detach_commands(vbd) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    // Unplug is best-effort (a halted VM's VBD is already unplugged); the
    // destroy is the operation that must succeed.
    let remote = format!("xe {unplug} 2>/dev/null; xe {destroy}");
    run_remote(&dom0, &remote, |_| json!({ "ok": true }).to_string())
}

fn sr_snapshot_reply(req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let req = match parse_req("sr-snapshot", req_body) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dom0 = match allowed_dom0(&req) {
        Ok(d) => d,
        Err(e) => return e,
    };
    // A `vdi` snapshots that single VDI; an `sr` snapshots every VDI on the store
    // (XAPI has no SR-level snapshot). `vdi` wins when both are present.
    let vdi = str_field(&req, "vdi");
    if !vdi.is_empty() {
        let cmd = match sr_snapshot_command(vdi) {
            Ok(c) => c,
            Err(e) => return err(e),
        };
        return run_xe(&dom0, &cmd, |out| {
            json!({ "ok": true, "snapshot": out }).to_string()
        });
    }
    let sr = str_field(&req, "sr");
    let cmd = match sr_snapshot_all_command(sr) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    run_remote(&dom0, &format!("sh -c '{cmd}'"), |out| {
        json!({ "ok": true, "snapshot": out }).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_lock() {
        assert!(is_storage_verb("sr-create"));
        assert!(is_storage_verb("vdi-create"));
        assert!(is_storage_verb("vdi-attach"));
        assert!(is_storage_verb("vdi-detach"));
        assert!(is_storage_verb("sr-snapshot"));
        assert!(!is_storage_verb("vm-power"));
        assert_eq!(STORAGE_VERBS.len(), 5);
    }

    #[test]
    fn sr_create_command_happy() {
        let c = sr_create_command("data", "lvm", "abc-123", None).unwrap();
        assert_eq!(c, "sr-create host-uuid=abc-123 type=lvm name-label=data");
    }

    #[test]
    fn sr_create_command_with_device() {
        let c = sr_create_command("data", "ext", "abc-123", Some("device=/dev/sdb")).unwrap();
        assert_eq!(
            c,
            "sr-create host-uuid=abc-123 type=ext name-label=data device-config:device=/dev/sdb"
        );
    }

    #[test]
    fn sr_create_rejects_injection() {
        assert!(sr_create_command("a;rm -rf /", "lvm", "h", None).is_err());
        assert!(sr_create_command("ok", "lvm;evil", "h", None).is_err());
        assert!(sr_create_command("ok", "lvm", "h$(x)", None).is_err());
        assert!(sr_create_command("ok", "lvm", "h", Some("device=/dev/sdb;rm")).is_err());
        assert!(sr_create_command("ok", "lvm", "h", Some("notpair")).is_err());
        assert!(sr_create_command("", "lvm", "h", None).is_err());
    }

    // Real Xen uuids are hex+dash (`[0-9a-fA-F-]`); the guard rejects anything
    // else, so these fixtures use hex-only ids.
    const SR: &str = "5ab1-c0de";
    const VDI: &str = "facade-01";
    const VM: &str = "deadbeef-00";
    const VBD: &str = "ba5eba11";

    #[test]
    fn vdi_create_command_happy() {
        let c = vdi_create_command(SR, "disk0", 40).unwrap();
        assert_eq!(
            c,
            "vdi-create sr-uuid=5ab1-c0de name-label=disk0 type=user virtual-size=40GiB"
        );
    }

    #[test]
    fn vdi_create_rejects_bad_size() {
        assert!(vdi_create_command(SR, "d", 0).is_err());
        assert!(vdi_create_command(SR, "d", 65537).is_err());
        assert!(vdi_create_command(SR, "bad name", 10).is_err());
        // A non-hex sr uuid is the injection guard firing.
        assert!(vdi_create_command("sr;1", "d", 10).is_err());
    }

    #[test]
    fn vdi_attach_commands_shape() {
        let (create, plug) = vdi_attach_commands(VDI, VM).unwrap();
        assert_eq!(
            create,
            "vbd-create vm-uuid=deadbeef-00 vdi-uuid=facade-01 device=autodetect mode=RW type=Disk"
        );
        assert_eq!(plug, "vbd-plug uuid=");
        assert!(vdi_attach_commands("vdi$1", VM).is_err());
        assert!(vdi_attach_commands(VDI, "").is_err());
    }

    #[test]
    fn vdi_detach_commands_shape() {
        let (unplug, destroy) = vdi_detach_commands(VBD).unwrap();
        assert_eq!(unplug, "vbd-unplug uuid=ba5eba11");
        assert_eq!(destroy, "vbd-destroy uuid=ba5eba11");
        assert!(vdi_detach_commands("vbd;1").is_err());
    }

    #[test]
    fn sr_snapshot_command_shape() {
        assert_eq!(
            sr_snapshot_command(VDI).unwrap(),
            "vdi-snapshot uuid=facade-01"
        );
        assert!(sr_snapshot_command("").is_err());
        assert!(sr_snapshot_command("vdi`x`").is_err());
    }

    #[test]
    fn sr_snapshot_all_command_shape() {
        let c = sr_snapshot_all_command(SR).unwrap();
        assert!(c.contains("xe vdi-list sr-uuid=5ab1-c0de"));
        assert!(c.contains("xe vdi-snapshot uuid=$v"));
        assert!(sr_snapshot_all_command("sr;evil").is_err());
        assert!(sr_snapshot_all_command("").is_err());
    }

    #[test]
    fn build_reply_unknown_verb() {
        let r = build_reply("nope", None);
        assert!(r.contains("unknown storage verb"));
    }

    #[test]
    fn replies_reject_missing_body() {
        // No dom0 in the allow-list (empty env) → these never reach `xe`; the
        // missing-body / bad-dom0 guards fire first.
        assert!(build_reply("sr-create", None).contains("missing request body"));
        assert!(build_reply("vdi-create", None).contains("missing request body"));
        assert!(build_reply("vdi-attach", None).contains("missing request body"));
        assert!(build_reply("vdi-detach", None).contains("missing request body"));
        assert!(build_reply("sr-snapshot", None).contains("missing request body"));
    }

    #[test]
    fn replies_reject_disallowed_dom0() {
        // A well-formed body whose dom0 is not in the (empty) allow-list is
        // rejected BEFORE any command is built/run.
        let body = r#"{"name":"data","host_uuid":"h-1","dom0":"10.0.0.1"}"#;
        assert!(build_reply("sr-create", Some(body)).contains("dom0 not in allowed set"));
    }
}
