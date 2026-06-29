//! XCP-1 — the XCP-ng hypervisor-access layer
//! (design: `docs/design/xcp-ng-integration.md`, lock A1).
//!
//! mackesd drives an XCP-ng **dom0** by running `xe` — either **locally** (when
//! mackesd runs ON the dom0, the Half-B "full partner" case) or **over SSH**
//! (driving a remote host, Half-A provisioning). Both share one code path: the
//! `xe` sub-command argv is built by pure functions and the executor only chooses
//! how to run it (local `xe …` vs `ssh root@host xe …`).
//!
//! Per §6 this is **glue, not reimplementation**: `std::process` shells out to
//! the host's own `xe`/`ssh`. Everything mechanically checkable — the `xe` argv
//! construction and the output parsing — is a pure function and unit-tested here;
//! the side-effecting [`XeSsh`] is the thin shell around it. A native rustls
//! XAPI backend can later implement [`Hypervisor`] behind the same trait.
//!
//! ## XCP-7 — the dom0 credential never reaches `ps`
//!
//! When a dom0 isn't reachable by the mesh SSH key (a freshly enrolled host, or
//! one whose XAPI/root login is password-only), mackesd authenticates with a
//! per-host secret sourced from the mesh secret store ([`HostTarget::Ssh::password`]).
//! That password is fed to `sshpass` on its **stdin** (its no-flag mode) — never
//! an argv token (`sshpass -p`), never a temp file. So the credential is absent
//! from the process listing and leaves nothing on disk (the XCP-7 acceptance).
//! [`full_command`] stays pure: it only consults whether a password is present
//! (to emit the `sshpass` wrapper), never the secret itself, which the runner
//! writes to the child's stdin at spawn time.

use std::process::Command;

use thiserror::Error;

/// Where (and how) to run `xe`.
///
/// `Debug` is hand-rolled below (not derived) to redact the XCP-7 password.
#[derive(Clone, PartialEq, Eq)]
pub enum HostTarget {
    /// mackesd runs on the dom0 itself — run `xe` locally (no SSH). The
    /// Half-B compute-provider case: the hypervisor *is* the node.
    Local,
    /// Drive a remote dom0 over SSH as `user@host` with an identity file.
    Ssh {
        /// dom0 address (overlay or LAN IP / hostname).
        host: String,
        /// SSH user (XCP-ng dom0 login — `root`).
        user: String,
        /// Path to the SSH identity (the mesh key); `None` uses the agent/default.
        identity: Option<String>,
        /// XCP-7 dom0 credential (the XAPI/root password) when key auth isn't
        /// available, sourced from the mesh secret store. `None` ⇒ key-only auth.
        /// NEVER placed in argv — fed to `sshpass` on its stdin by the runner so
        /// it can't appear in `ps`. `Debug` is hand-rolled to redact it.
        password: Option<String>,
    },
}

// Hand-rolled so a `{:?}` of a target (e.g. in a `tracing` line or a panic
// message) can NEVER leak the dom0 password — it renders as `***` (XCP-7: the
// credential must not appear in logs).
impl std::fmt::Debug for HostTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("Local"),
            Self::Ssh {
                host,
                user,
                identity,
                password,
            } => f
                .debug_struct("Ssh")
                .field("host", host)
                .field("user", user)
                .field("identity", identity)
                .field("password", &password.as_ref().map(|_| "***"))
                .finish(),
        }
    }
}

impl HostTarget {
    /// A remote SSH target with the conventional `root` dom0 login, key auth only.
    #[must_use]
    pub fn ssh_root(host: impl Into<String>, identity: Option<String>) -> Self {
        Self::Ssh {
            host: host.into(),
            user: "root".to_string(),
            identity,
            password: None,
        }
    }

    /// As [`HostTarget::ssh_root`] but also carries the XCP-7 dom0 password
    /// (from the mesh secret store) for hosts that aren't reachable by key.
    /// `password` is `None` ⇒ key-only (identical to [`HostTarget::ssh_root`]).
    #[must_use]
    pub fn ssh_root_with_password(
        host: impl Into<String>,
        identity: Option<String>,
        password: Option<String>,
    ) -> Self {
        Self::Ssh {
            host: host.into(),
            user: "root".to_string(),
            identity,
            password,
        }
    }

    /// The dom0 password for this target, if any (used by the runner to feed the
    /// `sshpass` pipe). `None` for `Local` and for key-only SSH targets.
    #[must_use]
    pub fn password(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Ssh { password, .. } => password.as_deref(),
        }
    }
}

/// One VM as reported by `xe vm-list` (XCP-1; the provisioning + capacity
/// surfaces consume these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmInfo {
    /// XAPI VM uuid.
    pub uuid: String,
    /// `name-label`.
    pub name: String,
    /// `power-state` (`running` / `halted` / `paused` / `suspended`).
    pub power_state: String,
}

impl VmInfo {
    /// Whether this VM is currently running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.power_state == "running"
    }
}

/// A dom0's advertised compute capacity (XCP-6 / B2 — published into the mesh
/// directory so any node can target this host for a spawn).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCapacity {
    /// Physical CPU count.
    pub cpu_count: u32,
    /// Total host memory (KiB).
    pub mem_total_kib: u64,
    /// Free host memory (KiB).
    pub mem_free_kib: u64,
    /// Largest free SR space (bytes) across the host's SRs — the spawn ceiling.
    pub sr_free_bytes: u64,
    /// Number of running VMs on the host.
    pub running_vms: u32,
}

/// A hypervisor-access failure.
#[derive(Debug, Error)]
pub enum XcpError {
    /// The `xe`/`ssh` process couldn't be spawned (binary missing, etc.).
    #[error("spawn {0}: {1}")]
    Spawn(String, std::io::Error),
    /// `xe` exited non-zero — carries the command + captured stderr.
    #[error("xe {cmd} failed (exit {code}): {stderr}")]
    Xe {
        /// The xe sub-command that failed (first argv element).
        cmd: String,
        /// Process exit code (or -1 if killed by signal).
        code: i32,
        /// Captured stderr (trimmed).
        stderr: String,
    },
    /// `xe` succeeded but its output didn't parse as expected.
    #[error("parse {0}")]
    Parse(String),
}

// ───────────────────────── pure: xe argv builders ─────────────────────────
// Each returns the `xe` sub-command argv (WITHOUT the leading `xe`). The
// executor prepends `xe` (local) or `ssh … xe` (remote). Kept pure + tested so
// the command surface can't silently drift.

/// `xe vm-clone vm=<golden> new-name-label=<new_name>` — the fast spawn (A2).
#[must_use]
pub fn argv_clone(golden: &str, new_name: &str) -> Vec<String> {
    vec![
        "vm-clone".into(),
        format!("vm={golden}"),
        format!("new-name-label={new_name}"),
    ]
}

/// `xe vm-start uuid=<uuid>`.
#[must_use]
pub fn argv_start(uuid: &str) -> Vec<String> {
    vec!["vm-start".into(), format!("uuid={uuid}")]
}

/// `xe vm-shutdown uuid=<uuid> force=true` then uninstall — destroy is two steps;
/// this is the force-shutdown half (a running VM can't be uninstalled).
#[must_use]
pub fn argv_force_shutdown(uuid: &str) -> Vec<String> {
    vec![
        "vm-shutdown".into(),
        format!("uuid={uuid}"),
        "force=true".into(),
    ]
}

/// `xe vm-uninstall uuid=<uuid> force=true` — removes the VM + its disks.
#[must_use]
pub fn argv_uninstall(uuid: &str) -> Vec<String> {
    vec![
        "vm-uninstall".into(),
        format!("uuid={uuid}"),
        "force=true".into(),
    ]
}

/// `xe vm-suspend uuid=<uuid>` — suspend a running VM to disk (DATACENTER-11).
#[must_use]
pub fn argv_suspend(uuid: &str) -> Vec<String> {
    vec!["vm-suspend".into(), format!("uuid={uuid}")]
}

/// `xe vm-resume uuid=<uuid>` — resume a suspended VM (DATACENTER-11).
#[must_use]
pub fn argv_resume(uuid: &str) -> Vec<String> {
    vec!["vm-resume".into(), format!("uuid={uuid}")]
}

/// `xe vm-migrate uuid=<uuid> host=<target> live=true` — live-migrate a VM to
/// another host in the pool (DATACENTER-11). `host=` accepts a destination
/// host name-label or uuid; `live=true` keeps the guest running during the move.
#[must_use]
pub fn argv_migrate(uuid: &str, target_host: &str) -> Vec<String> {
    vec![
        "vm-migrate".into(),
        format!("uuid={uuid}"),
        format!("host={target_host}"),
        "live=true".into(),
    ]
}

/// `xe console-list vm-uuid=<uuid> params=location --minimal` — read a VM's
/// console connection URL, the `location` the noVNC viewer dials (DATACENTER-11).
/// `--minimal` prints just the console object's `location` value.
#[must_use]
pub fn argv_console_url(uuid: &str) -> Vec<String> {
    vec![
        "console-list".into(),
        format!("vm-uuid={uuid}"),
        "params=location".into(),
        "--minimal".into(),
    ]
}

/// The cloud-init NoCloud seed for one MDE-VM: the rendered `user-data` and
/// `meta-data` documents plus the `instance-id` they pin (XCP-3 / A2).
///
/// Built once per spawn by [`build_identity_seed`] and attached to the freshly
/// cloned VM by [`Hypervisor::set_identity_seed`]. The new `instance-id` is what
/// makes cloud-init treat the clone as a *first boot* — so it regenerates SSH
/// host keys + `machine-id` and applies the new hostname (the A2 "fresh identity
/// per clone" rule), even though the golden template was cloned with the old
/// instance's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySeed {
    /// cloud-init `user-data` (a `#cloud-config` document).
    pub user_data: String,
    /// cloud-init `meta-data` (carries `instance-id` + `local-hostname`).
    pub meta_data: String,
    /// The unique `instance-id` this seed pins (also returned for the directory
    /// record so a spawn is traceable to its seed).
    pub instance_id: String,
}

/// Render the cloud-init NoCloud seed for an MDE-VM clone (XCP-3 / A2). Pure so
/// the rendered documents are testable without a host.
///
/// `name` is the spawn's short name; the guest hostname is forced to the
/// `MDE-VM-<name>` convention (operator rule 2026-06-16) — if `name` already
/// carries the `MDE-VM-` prefix it is not doubled. `op_ssh_key` is the operator's
/// authorized public key (OpenSSH `ssh-… ` line). `instance_id` is the
/// per-clone unique id (e.g. a uuid) that triggers cloud-init's first-boot path.
///
/// The `user-data` instructs cloud-init to:
/// - set the hostname (`hostname` + `fqdn`, `preserve_hostname: false`);
/// - install the operator key for the default user;
/// - **regenerate SSH host keys** (`ssh_deletekeys: true` + `ssh_genkeytypes`);
/// - **reset `machine-id`** on first boot (truncate `/etc/machine-id` so
///   systemd re-seeds it), per the A2 "fresh identity per clone" rule.
#[must_use]
pub fn build_identity_seed(name: &str, op_ssh_key: &str, instance_id: &str) -> IdentitySeed {
    build_seed_with_runcmds(name, op_ssh_key, instance_id, &[])
}

/// As [`build_identity_seed`] but also wires the clone to **self-enroll** on first
/// boot (XPA-7): appends a `mackesd join '<token>' --role <role>` runcmd. `token`
/// is a **v3 add-peer** join token — it pins the lighthouse's PUBLIC `/enroll`
/// endpoint + cert fingerprint, so the clone enrolls over a reachable endpoint,
/// NOT the unreachable Nebula overlay IP the legacy `enroll-token` advertised
/// (this subsumes XPA-5). The join argv rides the cloud-init NoCloud seed the
/// provisioner already attaches, so no extra over-SSH step is needed.
///
/// The join runcmd is rendered as a YAML/exec **argv list** (each element a
/// single-quoted scalar), so the token — which carries shell-significant `#`/`?`
/// — is passed verbatim to `mackesd` with no shell interpretation (the XPA-13
/// flow-style-quoting trap is avoided: no `sh -c`).
#[must_use]
pub fn build_join_seed(
    name: &str,
    op_ssh_key: &str,
    instance_id: &str,
    join_token: &str,
    role: &str,
) -> IdentitySeed {
    let join = render_runcmd_flow_list(&mackesd_join_argv(join_token, role));
    build_seed_with_runcmds(name, op_ssh_key, instance_id, &[join])
}

/// Shared core of [`build_identity_seed`] / [`build_join_seed`]: render the
/// cloud-config with the always-present machine-id reset runcmd plus any
/// `extra_runcmds` (each an already-rendered cloud-init runcmd list item, the
/// text after the `- `), in order.
fn build_seed_with_runcmds(
    name: &str,
    op_ssh_key: &str,
    instance_id: &str,
    extra_runcmds: &[String],
) -> IdentitySeed {
    let hostname = mde_vm_hostname(name);
    let key = op_ssh_key.trim();
    // The always-first runcmd: reset machine-id (A2 fresh-identity rule). Extra
    // runcmds (e.g. the XPA-7 self-join) follow, so the node has its fresh
    // machine-id/hostname before it enrolls.
    let mut runcmds = String::from(
        "\x20\x20- [ cloud-init-per, once, reset-machine-id, sh, -c, 'truncate -s 0 /etc/machine-id && rm -f /var/lib/dbus/machine-id' ]\n",
    );
    for item in extra_runcmds {
        runcmds.push_str("\x20\x20- ");
        runcmds.push_str(item);
        runcmds.push('\n');
    }
    let user_data = format!(
        "#cloud-config\n\
         preserve_hostname: false\n\
         hostname: {hostname}\n\
         fqdn: {hostname}\n\
         ssh_deletekeys: true\n\
         ssh_genkeytypes: [rsa, ecdsa, ed25519]\n\
         ssh_authorized_keys:\n\
         \x20\x20- {key}\n\
         runcmd:\n\
         {runcmds}"
    );
    let meta_data = format!("instance-id: {instance_id}\nlocal-hostname: {hostname}\n");
    IdentitySeed {
        user_data,
        meta_data,
        instance_id: instance_id.to_string(),
    }
}

/// The `mackesd join` argv a freshly-provisioned clone runs on first boot to
/// enroll itself into the mesh (XPA-7). Pure so the command surface is testable.
#[must_use]
pub fn mackesd_join_argv(join_token: &str, role: &str) -> Vec<String> {
    vec![
        "mackesd".into(),
        "join".into(),
        join_token.into(),
        "--role".into(),
        role.into(),
    ]
}

/// Render an argv as a cloud-init runcmd **flow list** (`[ 'a', 'b', … ]`), each
/// element a single-quoted YAML scalar (so a `'` inside an element is doubled per
/// the YAML single-quote rule). cloud-init execs a list-form runcmd directly
/// (no shell), so shell-significant characters in any element are inert.
fn render_runcmd_flow_list(argv: &[String]) -> String {
    let quoted: Vec<String> = argv
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "''")))
        .collect();
    format!("[ {} ]", quoted.join(", "))
}

/// Force the `MDE-VM-<name>` hostname convention (operator rule 2026-06-16),
/// without doubling an already-prefixed `name`.
#[must_use]
pub fn mde_vm_hostname(name: &str) -> String {
    let n = name.trim();
    if n.starts_with("MDE-VM-") || n == "MDE-VM" {
        n.to_string()
    } else if let Some(rest) = n.strip_prefix("MDE-VM") {
        // e.g. "MDE-VM_web1" / "MDE-VMweb1" — normalize to a single dashed prefix.
        format!("MDE-VM-{}", rest.trim_start_matches(['-', '_']))
    } else {
        format!("MDE-VM-{n}")
    }
}

/// Minimal, dependency-free base64 (standard alphabet, `=` padded). The seed
/// documents are pushed into `xenstore-data` where XCP's cloud-init NoCloud
/// datasource (`vm-data/…`) expects base64-encoded `user-data`/`meta-data`;
/// keeping it pure avoids pulling a base64 crate into this glue layer (§6).
#[must_use]
pub fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied();
        let b2 = chunk.get(2).copied();
        let n =
            (u32::from(b0) << 16) | (u32::from(b1.unwrap_or(0)) << 8) | u32::from(b2.unwrap_or(0));
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if b1.is_some() {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if b2.is_some() {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// `xe vm-param-set uuid=<uuid> xenstore-data:vm-data/user-data=<b64> …` — push
/// the cloud-init NoCloud seed into the cloned VM's `xenstore-data` so the guest
/// agent's NoCloud datasource picks it up on first boot (XCP-3 / A2). One `xe`
/// invocation sets the three map keys (`user-data`, `meta-data`, and the
/// `instance-id` mirror) that the datasource reads. Pure + tested.
#[must_use]
pub fn argv_set_identity_seed(uuid: &str, seed: &IdentitySeed) -> Vec<String> {
    let ud = base64_encode(seed.user_data.as_bytes());
    let md = base64_encode(seed.meta_data.as_bytes());
    vec![
        "vm-param-set".into(),
        format!("uuid={uuid}"),
        format!("xenstore-data:vm-data/user-data={ud}"),
        format!("xenstore-data:vm-data/meta-data={md}"),
        format!("xenstore-data:vm-data/instance-id={}", seed.instance_id),
    ]
}

/// `xe vm-list params=uuid,name-label,power-state --minimal` — the roster.
/// `--minimal` makes `xe` emit one CSV record per VM (semicolon-separated rows),
/// which [`parse_vm_list`] decodes.
#[must_use]
pub fn argv_vm_list() -> Vec<String> {
    vec![
        "vm-list".into(),
        "params=uuid,name-label,power-state".into(),
        "is-control-domain=false".into(),
    ]
}

/// `xe vm-param-get uuid=<uuid> param-name=networks` — the guest-agent-reported
/// addresses (`0/ip: 10.x; …`); [`parse_first_ipv4`] pulls the first IPv4.
#[must_use]
pub fn argv_vm_networks(uuid: &str) -> Vec<String> {
    vec![
        "vm-param-get".into(),
        format!("uuid={uuid}"),
        "param-name=networks".into(),
    ]
}

/// `xe host-list params=…` — the host's CPU/memory totals for capacity (XCP-6).
#[must_use]
pub fn argv_host_params() -> Vec<String> {
    vec![
        "host-list".into(),
        "params=host-metrics-live,memory-total,memory-free,cpu_count".into(),
    ]
}

/// `xe sr-list params=uuid,physical-size,physical-utilisation` — for the largest
/// free-SR-space figure (the spawn ceiling).
#[must_use]
pub fn argv_sr_list() -> Vec<String> {
    vec![
        "sr-list".into(),
        "params=uuid,physical-size,physical-utilisation".into(),
    ]
}

/// XPA-1 — the default memory a **headless Server** MDE-VM is right-sized to
/// after clone (2 GiB), instead of inheriting the golden template's larger size.
/// A 15.9 GB host couldn't start 4×4 GB clones + the base; 2 GB is plenty for a
/// headless `mackesd`/nebula/storage-client Server and quadruples the host's VM
/// density.
pub const DEFAULT_SERVER_VM_MEM_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// XPA-1 — host memory (KiB) the pre-fan-out check reserves for dom0 + slack on
/// top of the requested VMs, so a spawn that would starve the host fails *early*
/// with a clear message rather than after a clone that then can't boot.
pub const HOST_MEM_HEADROOM_KIB: u64 = 512 * 1024;

/// `xe vm-memory-limits-set uuid=<uuid> static-min/max + dynamic-min/max=<bytes>`
/// — pin a clone to a fixed `bytes` allocation (XPA-1). All four limits are set
/// to the same value (a fixed, non-ballooning size); xe requires
/// `static-min ≤ dynamic-min ≤ dynamic-max ≤ static-max`, which equal values
/// satisfy. Pure + tested.
#[must_use]
pub fn argv_memory_set(uuid: &str, bytes: u64) -> Vec<String> {
    let b = bytes.to_string();
    vec![
        "vm-memory-limits-set".into(),
        format!("uuid={uuid}"),
        format!("static-min={b}"),
        format!("static-max={b}"),
        format!("dynamic-min={b}"),
        format!("dynamic-max={b}"),
    ]
}

/// Pre-fan-out capacity guard (XPA-1): does `cap` have enough **free** memory to
/// start `count` VMs of `per_vm_kib` each, leaving `headroom_kib` for the host?
/// Pure so the over-commit math is testable without a live host. Reuses the
/// [`HostCapacity`] the host-params probe ([`build_capacity`]) already parsed.
///
/// # Errors
/// `Err(message)` — a human-readable "insufficient free memory" the provisioner
/// surfaces verbatim — when free memory is below the requirement.
pub fn precheck_host_memory(
    cap: &HostCapacity,
    per_vm_kib: u64,
    count: u32,
    headroom_kib: u64,
) -> Result<(), String> {
    let needed = per_vm_kib
        .saturating_mul(u64::from(count))
        .saturating_add(headroom_kib);
    if cap.mem_free_kib >= needed {
        Ok(())
    } else {
        Err(format!(
            "insufficient host free memory: need {needed} KiB ({count}×{per_vm_kib} KiB/VM + \
             {headroom_kib} KiB headroom) but only {} KiB free",
            cap.mem_free_kib
        ))
    }
}

/// One VIF as reported by `xe vif-list` (XPA-4 — the clone's network interfaces
/// whose MACs are reset so clones don't collide on the golden's copied MAC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VifInfo {
    /// XAPI VIF uuid.
    pub uuid: String,
    /// Device index on the VM (`0`, `1`, …).
    pub device: String,
    /// Current MAC (the golden's, copied by the clone).
    pub mac: String,
    /// The network the VIF is attached to (needed to recreate it).
    pub network_uuid: String,
}

/// `xe vif-list vm-uuid=<uuid> params=uuid,device,MAC,network-uuid` — the clone's
/// VIFs, so each can be reset to a fresh MAC (XPA-4).
#[must_use]
pub fn argv_vif_list(vm_uuid: &str) -> Vec<String> {
    vec![
        "vif-list".into(),
        format!("vm-uuid={vm_uuid}"),
        "params=uuid,device,MAC,network-uuid".into(),
    ]
}

/// Decode `xe vif-list params=uuid,device,MAC,network-uuid` into [`VifInfo`]s.
#[must_use]
pub fn parse_vif_list(out: &str) -> Vec<VifInfo> {
    parse_param_records(out)
        .into_iter()
        .filter_map(|rec| {
            let get = |k: &str| rec.iter().find(|(rk, _)| rk == k).map(|(_, v)| v.clone());
            let uuid = get("uuid")?;
            if uuid.is_empty() {
                return None;
            }
            Some(VifInfo {
                uuid,
                device: get("device").unwrap_or_default(),
                mac: get("MAC").unwrap_or_default(),
                network_uuid: get("network-uuid").unwrap_or_default(),
            })
        })
        .collect()
}

/// `xe vif-destroy uuid=<vif>` — half of the MAC reset (xe forbids mutating a
/// VIF's MAC in place, so the clone's VIF is destroyed + recreated, XPA-4).
#[must_use]
pub fn argv_vif_destroy(vif_uuid: &str) -> Vec<String> {
    vec!["vif-destroy".into(), format!("uuid={vif_uuid}")]
}

/// `xe vif-create vm-uuid=<uuid> network-uuid=<net> device=<dev> mac=<mac>` — the
/// recreate half of the MAC reset (XPA-4): same network + device as the destroyed
/// VIF, but a fresh clone-unique `mac` from [`mac_for_clone`].
#[must_use]
pub fn argv_vif_create(vm_uuid: &str, network_uuid: &str, device: &str, mac: &str) -> Vec<String> {
    vec![
        "vif-create".into(),
        format!("vm-uuid={vm_uuid}"),
        format!("network-uuid={network_uuid}"),
        format!("device={device}"),
        format!("mac={mac}"),
    ]
}

/// Derive a stable, **locally-administered unicast** MAC for a clone's VIF from
/// the (globally unique) clone `uuid` + the VIF `device` index (XPA-4).
/// Deterministic (so it's unit-testable) and collision-free across clones —
/// distinct uuids hash to distinct MACs. The first octet has the multicast bit
/// cleared + the locally-administered bit set (low nibble `2/6/A/E`), the
/// convention for a generated (non-OUI) MAC.
#[must_use]
pub fn mac_for_clone(uuid: &str, device: &str) -> String {
    // FNV-1a 64-bit over `uuid '/' device` — dependency-free, good dispersion.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in uuid
        .bytes()
        .chain(std::iter::once(b'/'))
        .chain(device.bytes())
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let o = h.to_be_bytes();
    // Clear multicast (bit 0), set locally-administered (bit 1) on the first octet.
    let first = (o[0] & 0xfe) | 0x02;
    format!(
        "{first:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        o[1], o[2], o[3], o[4], o[5]
    )
}

/// Build the full executable + argv for a target: `xe <args…>` locally, or
/// `ssh [-i id] -o BatchMode=yes user@host xe <args…>` remotely. Pure + tested.
///
/// XCP-7: when the SSH target carries a password, the command is wrapped in
/// `sshpass ssh …` so the runner can feed the credential on the child's **stdin**
/// (`sshpass`'s no-flag mode) — the password is NEVER an argv element here (so it
/// can't surface in `ps`) and never touches disk. `BatchMode=yes` is also dropped
/// in that case, since it would refuse the password prompt `sshpass` answers.
#[must_use]
pub fn full_command(target: &HostTarget, xe_args: &[String]) -> (String, Vec<String>) {
    match target {
        HostTarget::Local => ("xe".to_string(), xe_args.to_vec()),
        HostTarget::Ssh {
            host,
            user,
            identity,
            password,
        } => {
            let has_password = password.is_some();
            // The `ssh …` argv (shared whether or not it's wrapped by sshpass).
            let mut ssh_argv = Vec::new();
            if let Some(id) = identity {
                ssh_argv.push("-i".to_string());
                ssh_argv.push(id.clone());
            }
            // BatchMode refuses interactive auth; only set it for key-only auth.
            // With a password we let sshpass answer the prompt.
            if !has_password {
                ssh_argv.push("-o".to_string());
                ssh_argv.push("BatchMode=yes".to_string());
            }
            ssh_argv.push("-o".to_string());
            ssh_argv.push("StrictHostKeyChecking=accept-new".to_string());
            ssh_argv.push(format!("{user}@{host}"));
            ssh_argv.push("xe".to_string());
            ssh_argv.extend(xe_args.iter().cloned());

            if has_password {
                // `sshpass ssh …` — `sshpass` with no -p/-f/-d/-e reads the
                // password from its OWN stdin, NOT from argv (the whole point of
                // XCP-7). The runner writes the secret to the child's stdin.
                let mut argv = vec!["ssh".to_string()];
                argv.extend(ssh_argv);
                ("sshpass".to_string(), argv)
            } else {
                ("ssh".to_string(), ssh_argv)
            }
        }
    }
}

// ───────────────────────── pure: output parsers ─────────────────────────

/// Parse `xe … params=a,b,c` (non-`--minimal`) output into records. `xe` prints
/// blank-line-separated records of `key ( RO): value` lines; returns each record
/// as `(key→value)` pairs (keys trimmed of the `( RO)`/`( RW)` suffix).
#[must_use]
pub fn parse_param_records(out: &str) -> Vec<Vec<(String, String)>> {
    let mut records = Vec::new();
    let mut cur: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            if !cur.is_empty() {
                records.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            // k looks like "memory-free ( RO)" — strip the parenthetical.
            let key = k.split('(').next().unwrap_or(k).trim().to_string();
            if !key.is_empty() {
                cur.push((key, v.trim().to_string()));
            }
        }
    }
    if !cur.is_empty() {
        records.push(cur);
    }
    records
}

/// Decode `xe vm-list params=uuid,name-label,power-state` into [`VmInfo`]s.
#[must_use]
pub fn parse_vm_list(out: &str) -> Vec<VmInfo> {
    parse_param_records(out)
        .into_iter()
        .filter_map(|rec| {
            let get = |k: &str| rec.iter().find(|(rk, _)| rk == k).map(|(_, v)| v.clone());
            Some(VmInfo {
                uuid: get("uuid")?,
                name: get("name-label").unwrap_or_default(),
                power_state: get("power-state").unwrap_or_default(),
            })
        })
        .filter(|v| !v.uuid.is_empty())
        .collect()
}

/// Pull the first IPv4 from a `xe vm-param-get param-name=networks` value, which
/// looks like `0/ip: 10.42.0.9; 0/ipv6/0: fe80::…; …`. `None` until the guest
/// agent has reported an address.
#[must_use]
pub fn parse_first_ipv4(networks: &str) -> Option<String> {
    for part in networks.split(';') {
        let val = part.split(':').nth(1).unwrap_or("").trim();
        if is_ipv4(val) {
            return Some(val.to_string());
        }
    }
    None
}

/// A bare dotted-quad check (no external dep): four `0..=255` octets.
#[must_use]
fn is_ipv4(s: &str) -> bool {
    let octets: Vec<&str> = s.split('.').collect();
    octets.len() == 4
        && octets
            .iter()
            .all(|o| !o.is_empty() && o.parse::<u8>().is_ok())
}

/// Build [`HostCapacity`] from the host-params record, the SR records, and the
/// running-VM count. Pure so the capacity math is testable without a live host.
#[must_use]
pub fn build_capacity(
    host_rec: &[(String, String)],
    sr_records: &[Vec<(String, String)>],
    running_vms: u32,
) -> HostCapacity {
    let get = |k: &str| {
        host_rec
            .iter()
            .find(|(rk, _)| rk == k)
            .map(|(_, v)| v.as_str())
    };
    let parse_u64 = |k: &str| get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    // Largest free space across SRs = physical-size − physical-utilisation.
    let sr_free_bytes = sr_records
        .iter()
        .map(|rec| {
            let g = |k: &str| {
                rec.iter()
                    .find(|(rk, _)| rk == k)
                    .and_then(|(_, v)| v.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            g("physical-size").saturating_sub(g("physical-utilisation"))
        })
        .max()
        .unwrap_or(0);
    HostCapacity {
        cpu_count: u32::try_from(parse_u64("cpu_count")).unwrap_or(0),
        mem_total_kib: parse_u64("memory-total") / 1024,
        mem_free_kib: parse_u64("memory-free") / 1024,
        sr_free_bytes,
        running_vms,
    }
}

// ───────────────────────── trait + executor ─────────────────────────

/// The hypervisor-access surface (XCP-1). `XeSsh` is the xe-over-SSH/local impl;
/// a native XAPI backend can implement this later behind the same trait.
pub trait Hypervisor {
    /// Clone the golden template to `new_name`, returning the new VM's uuid (A2).
    ///
    /// # Errors
    /// Spawn / non-zero `xe` / parse failures.
    fn clone_golden(&self, golden: &str, new_name: &str) -> Result<String, XcpError>;
    /// Attach the cloud-init NoCloud identity seed to a (freshly cloned, still
    /// halted) VM so it boots with a fresh identity: `MDE-VM-<name>` hostname,
    /// the operator's key, regenerated SSH host keys + `machine-id` (A2). Called
    /// between [`Hypervisor::clone_golden`] and [`Hypervisor::start`].
    ///
    /// # Errors
    /// Spawn / non-zero `xe` failures.
    fn set_identity_seed(&self, uuid: &str, seed: &IdentitySeed) -> Result<(), XcpError>;
    /// XPA-1 — pin a (freshly cloned, still halted) VM to a fixed `bytes` memory
    /// allocation so a headless Server clone doesn't inherit the golden's larger
    /// size. Called between [`Hypervisor::clone_golden`] and
    /// [`Hypervisor::start`].
    ///
    /// # Errors
    /// Spawn / non-zero `xe` failures.
    fn set_memory(&self, uuid: &str, bytes: u64) -> Result<(), XcpError>;
    /// XPA-4 — reset every VIF on a (freshly cloned, still halted) VM to a fresh
    /// locally-administered MAC, so clones don't collide on the golden's copied
    /// MAC. Each VIF is destroyed + recreated on the same network/device with a
    /// clone-unique MAC ([`mac_for_clone`]). Called between
    /// [`Hypervisor::clone_golden`] and [`Hypervisor::start`].
    ///
    /// # Errors
    /// Spawn / non-zero `xe` / parse failures.
    fn reset_vif_macs(&self, uuid: &str) -> Result<(), XcpError>;
    /// Start a VM by uuid.
    ///
    /// # Errors
    /// Spawn / non-zero `xe` failures.
    fn start(&self, uuid: &str) -> Result<(), XcpError>;
    /// Force-shutdown (if running) then uninstall a VM + its disks.
    ///
    /// # Errors
    /// Spawn / non-zero `xe` failures (a shutdown error on an already-halted VM
    /// is tolerated; the uninstall is the operative step).
    fn destroy(&self, uuid: &str) -> Result<(), XcpError>;
    /// The first guest-agent-reported IPv4 of a VM, if any yet.
    ///
    /// # Errors
    /// Spawn / non-zero `xe` failures. `Ok(None)` when no address is reported.
    fn vm_ip(&self, uuid: &str) -> Result<Option<String>, XcpError>;
    /// List the host's VMs (excludes the control domain).
    ///
    /// # Errors
    /// Spawn / non-zero `xe` / parse failures.
    fn list(&self) -> Result<Vec<VmInfo>, XcpError>;
    /// The host's advertised compute capacity (XCP-6 / B2).
    ///
    /// # Errors
    /// Spawn / non-zero `xe` / parse failures.
    fn host_capacity(&self) -> Result<HostCapacity, XcpError>;
}

/// xe-over-SSH (or local) hypervisor access (A1). Holds only the target; each
/// call shells out via [`Runner`].
#[derive(Debug, Clone)]
pub struct XeSsh {
    target: HostTarget,
}

impl XeSsh {
    /// New accessor for `target` (`HostTarget::Local` on a dom0, or `Ssh{…}`).
    #[must_use]
    pub fn new(target: HostTarget) -> Self {
        Self { target }
    }

    /// Run an `xe` sub-command, returning trimmed stdout. The single I/O choke
    /// point; everything above it is pure.
    ///
    /// XCP-7: a password-bearing target is run via `sshpass ssh …` with the
    /// credential fed on the child's stdin (never argv / disk) — see [`run_sshpass`].
    fn run(&self, xe_args: &[String]) -> Result<String, XcpError> {
        let (exe, argv) = full_command(&self.target, xe_args);
        let cmd_name = xe_args.first().cloned().unwrap_or_default();
        let out = match self.target.password() {
            Some(pw) => run_sshpass(&exe, &argv, pw)?,
            None => Command::new(&exe)
                .args(&argv)
                .output()
                .map_err(|e| XcpError::Spawn(exe.clone(), e))?,
        };
        if !out.status.success() {
            return Err(XcpError::Xe {
                cmd: cmd_name,
                code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// Spawn `sshpass ssh …`, feeding `password` on the child's **stdin** —
/// `sshpass`'s no-flag mode reads the password there. This is the XCP-7 no-leak
/// path: the password is absent from argv (so not in `ps`), never written to
/// disk, and never logged. `sshpass` consumes only the first line of stdin and
/// hands the rest (none, here) to `ssh`; the `xe` command rides in the ssh argv,
/// so ssh needs no stdin of its own.
fn run_sshpass(
    exe: &str,
    argv: &[String],
    password: &str,
) -> Result<std::process::Output, XcpError> {
    use std::io::Write as _;

    let mut child = Command::new(exe)
        .args(argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| XcpError::Spawn(exe.to_string(), e))?;
    // Write the password + newline to sshpass's stdin, then close it so sshpass
    // stops reading and proceeds. A broken pipe (sshpass already exited, e.g.
    // missing binary races) is tolerated — the exit status is the source of truth.
    {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| XcpError::Parse("sshpass stdin unavailable".to_string()))?;
        let mut stdin = stdin;
        let _ = stdin
            .write_all(password.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"));
        // `stdin` drops here, closing the pipe.
    }
    child
        .wait_with_output()
        .map_err(|e| XcpError::Spawn("sshpass wait".to_string(), e))
}

impl Hypervisor for XeSsh {
    fn clone_golden(&self, golden: &str, new_name: &str) -> Result<String, XcpError> {
        // vm-clone prints the new uuid on stdout.
        let uuid = self.run(&argv_clone(golden, new_name))?;
        if uuid.is_empty() {
            return Err(XcpError::Parse("vm-clone returned no uuid".into()));
        }
        Ok(uuid)
    }

    fn set_identity_seed(&self, uuid: &str, seed: &IdentitySeed) -> Result<(), XcpError> {
        self.run(&argv_set_identity_seed(uuid, seed)).map(|_| ())
    }

    fn set_memory(&self, uuid: &str, bytes: u64) -> Result<(), XcpError> {
        self.run(&argv_memory_set(uuid, bytes)).map(|_| ())
    }

    fn reset_vif_macs(&self, uuid: &str) -> Result<(), XcpError> {
        // List the clone's VIFs, then destroy + recreate each on the same
        // network/device with a fresh clone-unique MAC (xe can't mutate a VIF's
        // MAC in place). Empty VIF list ⇒ nothing to do.
        let vifs = parse_vif_list(&self.run(&argv_vif_list(uuid))?);
        for vif in vifs {
            self.run(&argv_vif_destroy(&vif.uuid))?;
            let mac = mac_for_clone(uuid, &vif.device);
            self.run(&argv_vif_create(uuid, &vif.network_uuid, &vif.device, &mac))?;
        }
        Ok(())
    }

    fn start(&self, uuid: &str) -> Result<(), XcpError> {
        self.run(&argv_start(uuid)).map(|_| ())
    }

    fn destroy(&self, uuid: &str) -> Result<(), XcpError> {
        // Best-effort shutdown (a halted VM errors here — tolerated), then the
        // operative uninstall.
        let _ = self.run(&argv_force_shutdown(uuid));
        self.run(&argv_uninstall(uuid)).map(|_| ())
    }

    fn vm_ip(&self, uuid: &str) -> Result<Option<String>, XcpError> {
        let networks = self.run(&argv_vm_networks(uuid))?;
        Ok(parse_first_ipv4(&networks))
    }

    fn list(&self) -> Result<Vec<VmInfo>, XcpError> {
        Ok(parse_vm_list(&self.run(&argv_vm_list())?))
    }

    fn host_capacity(&self) -> Result<HostCapacity, XcpError> {
        let host_recs = parse_param_records(&self.run(&argv_host_params())?);
        let host_rec = host_recs.into_iter().next().unwrap_or_default();
        let sr_records = parse_param_records(&self.run(&argv_sr_list())?);
        let running = self.list()?.iter().filter(|v| v.is_running()).count();
        Ok(build_capacity(
            &host_rec,
            &sr_records,
            u32::try_from(running).unwrap_or(u32::MAX),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_builders_shape() {
        assert_eq!(
            argv_clone("MDE-VM-golden", "MDE-VM-web1"),
            vec!["vm-clone", "vm=MDE-VM-golden", "new-name-label=MDE-VM-web1"]
        );
        assert_eq!(argv_start("u1"), vec!["vm-start", "uuid=u1"]);
        assert_eq!(
            argv_uninstall("u1"),
            vec!["vm-uninstall", "uuid=u1", "force=true"]
        );
    }

    #[test]
    fn argv_lifecycle_builders_shape() {
        // DATACENTER-11 — suspend/resume/migrate/console-url argv surfaces.
        assert_eq!(argv_suspend("u1"), vec!["vm-suspend", "uuid=u1"]);
        assert_eq!(argv_resume("u1"), vec!["vm-resume", "uuid=u1"]);
        assert_eq!(
            argv_migrate("u1", "MDE-host-b"),
            vec!["vm-migrate", "uuid=u1", "host=MDE-host-b", "live=true"]
        );
        // console-url joins to the exact xe command the responder runs.
        assert_eq!(
            argv_console_url("u1"),
            vec!["console-list", "vm-uuid=u1", "params=location", "--minimal"]
        );
        assert_eq!(
            argv_console_url("u1").join(" "),
            "console-list vm-uuid=u1 params=location --minimal"
        );
    }

    #[test]
    fn full_command_local_vs_ssh() {
        let (exe, argv) = full_command(&HostTarget::Local, &argv_start("u1"));
        assert_eq!(exe, "xe");
        assert_eq!(argv, vec!["vm-start", "uuid=u1"]);

        let t = HostTarget::ssh_root("10.0.0.4", Some("/k/id".into()));
        let (exe, argv) = full_command(&t, &argv_start("u1"));
        assert_eq!(exe, "ssh");
        // -i /k/id -o BatchMode=yes -o StrictHostKeyChecking=accept-new root@10.0.0.4 xe vm-start uuid=u1
        assert_eq!(argv[0], "-i");
        assert_eq!(argv[1], "/k/id");
        assert!(argv.contains(&"root@10.0.0.4".to_string()));
        let xe_pos = argv.iter().position(|a| a == "xe").unwrap();
        assert_eq!(
            &argv[xe_pos + 1..],
            &["vm-start".to_string(), "uuid=u1".to_string()]
        );
        // No identity → no -i.
        let (_, argv2) = full_command(&HostTarget::ssh_root("h", None), &argv_start("u1"));
        assert!(!argv2.contains(&"-i".to_string()));
    }

    #[test]
    fn full_command_with_password_wraps_sshpass_and_never_leaks_the_secret() {
        // XCP-7: a password-bearing target is run as `sshpass ssh …`; the password
        // is fed on stdin by the runner, so it must NOT appear anywhere in argv.
        let secret = "s3cr3t-dom0-pw";
        let t = HostTarget::ssh_root_with_password(
            "10.0.0.4",
            Some("/k/id".into()),
            Some(secret.to_string()),
        );
        let (exe, argv) = full_command(&t, &argv_start("u1"));
        assert_eq!(exe, "sshpass");
        // The wrapper invokes ssh, with no -p/-f/-d/-e flag (stdin mode).
        assert_eq!(argv[0], "ssh");
        assert!(
            !argv
                .iter()
                .any(|a| a == "-p" || a == "-f" || a == "-e" || a.starts_with("-d")),
            "sshpass must read the password from stdin, not a flag: {argv:?}"
        );
        // THE acceptance assertion: the secret is in NO argv token.
        assert!(
            !argv.iter().any(|a| a.contains(secret)),
            "the dom0 password must never appear in argv (ps): {argv:?}"
        );
        // BatchMode is dropped (it would refuse the password prompt) but the host
        // + xe command still ride through.
        assert!(!argv.iter().any(|a| a == "BatchMode=yes"));
        assert!(argv.contains(&"root@10.0.0.4".to_string()));
        let xe_pos = argv.iter().position(|a| a == "xe").unwrap();
        assert_eq!(
            &argv[xe_pos + 1..],
            &["vm-start".to_string(), "uuid=u1".to_string()]
        );
        // A None password is the key-only path (plain ssh, BatchMode on).
        let (exe2, argv2) = full_command(
            &HostTarget::ssh_root_with_password("h", None, None),
            &argv_start("u1"),
        );
        assert_eq!(exe2, "ssh");
        assert!(argv2.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn debug_redacts_the_dom0_password() {
        // XCP-7: a {:?} of the target (a tracing line / panic) must not leak the
        // credential — it renders as ***.
        let t = HostTarget::ssh_root_with_password(
            "10.0.0.4",
            None,
            Some("super-secret-pw".to_string()),
        );
        let dbg = format!("{t:?}");
        assert!(
            !dbg.contains("super-secret-pw"),
            "password leaked into Debug: {dbg}"
        );
        assert!(dbg.contains("***"));
        // The accessor exposes it for the runner, but the type never prints it.
        assert_eq!(t.password(), Some("super-secret-pw"));
        // A key-only target has no password.
        assert_eq!(HostTarget::ssh_root("h", None).password(), None);
        assert_eq!(HostTarget::Local.password(), None);
    }

    #[test]
    fn parse_vm_list_decodes_records() {
        let out = "\
uuid ( RO)        : aaaa-1
    name-label ( RW): MDE-VM-web1
    power-state ( RO): running

uuid ( RO)        : bbbb-2
    name-label ( RW): MDE-VM-db
    power-state ( RO): halted
";
        let vms = parse_vm_list(out);
        assert_eq!(vms.len(), 2);
        assert_eq!(vms[0].uuid, "aaaa-1");
        assert_eq!(vms[0].name, "MDE-VM-web1");
        assert!(vms[0].is_running());
        assert_eq!(vms[1].power_state, "halted");
        assert!(!vms[1].is_running());
    }

    #[test]
    fn parse_first_ipv4_picks_the_v4() {
        assert_eq!(
            parse_first_ipv4("0/ip: 10.42.0.9; 0/ipv6/0: fe80::1; 1/ip: 192.168.1.5").as_deref(),
            Some("10.42.0.9")
        );
        // No address reported yet.
        assert_eq!(parse_first_ipv4(""), None);
        assert_eq!(parse_first_ipv4("0/ipv6/0: fe80::1"), None);
    }

    #[test]
    fn build_capacity_math() {
        let host = vec![
            (
                "memory-total".to_string(),
                (8u64 * 1024 * 1024 * 1024).to_string(),
            ), // 8 GiB in bytes
            (
                "memory-free".to_string(),
                (2u64 * 1024 * 1024 * 1024).to_string(),
            ),
            ("cpu_count".to_string(), "4".to_string()),
        ];
        let srs = vec![
            vec![
                ("physical-size".to_string(), "1000".to_string()),
                ("physical-utilisation".to_string(), "300".to_string()),
            ],
            vec![
                ("physical-size".to_string(), "5000".to_string()),
                ("physical-utilisation".to_string(), "4900".to_string()),
            ],
        ];
        let cap = build_capacity(&host, &srs, 3);
        assert_eq!(cap.cpu_count, 4);
        assert_eq!(cap.mem_total_kib, 8 * 1024 * 1024); // bytes/1024 → KiB
        assert_eq!(cap.mem_free_kib, 2 * 1024 * 1024);
        assert_eq!(cap.sr_free_bytes, 700); // max(1000-300, 5000-4900)=700
        assert_eq!(cap.running_vms, 3);
    }

    #[test]
    fn mde_vm_hostname_enforces_prefix() {
        assert_eq!(mde_vm_hostname("web1"), "MDE-VM-web1");
        // Already prefixed — not doubled.
        assert_eq!(mde_vm_hostname("MDE-VM-web1"), "MDE-VM-web1");
        assert_eq!(mde_vm_hostname("  db  "), "MDE-VM-db");
        // Odd separators after the prefix are normalized to a single dash.
        assert_eq!(mde_vm_hostname("MDE-VM_db"), "MDE-VM-db");
        assert_eq!(mde_vm_hostname("MDE-VM"), "MDE-VM");
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 §10 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn build_identity_seed_renders_a_first_boot_cloud_config() {
        let seed = build_identity_seed("web1", "ssh-ed25519 AAAAkey op@host", "iid-123");
        // user-data is a cloud-config that sets the MDE-VM hostname…
        assert!(seed.user_data.starts_with("#cloud-config\n"));
        assert!(seed.user_data.contains("hostname: MDE-VM-web1"));
        // …injects the operator key…
        assert!(seed.user_data.contains("- ssh-ed25519 AAAAkey op@host"));
        // …regenerates host keys + resets machine-id (the A2 fresh-identity rule)…
        assert!(seed.user_data.contains("ssh_deletekeys: true"));
        assert!(seed.user_data.contains("/etc/machine-id"));
        assert!(seed.user_data.contains("/var/lib/dbus/machine-id"));
        // …and meta-data pins the per-clone instance-id + hostname.
        assert_eq!(
            seed.meta_data,
            "instance-id: iid-123\nlocal-hostname: MDE-VM-web1\n"
        );
        assert_eq!(seed.instance_id, "iid-123");
    }

    #[test]
    fn argv_set_identity_seed_shape_and_roundtrip() {
        let seed = build_identity_seed("web1", "ssh-ed25519 KEY op@host", "iid-9");
        let argv = argv_set_identity_seed("u-7", &seed);
        assert_eq!(argv[0], "vm-param-set");
        assert_eq!(argv[1], "uuid=u-7");
        // Three xenstore-data map keys, base64-encoded payloads.
        let ud = argv[2]
            .strip_prefix("xenstore-data:vm-data/user-data=")
            .unwrap();
        let md = argv[3]
            .strip_prefix("xenstore-data:vm-data/meta-data=")
            .unwrap();
        assert_eq!(argv[4], "xenstore-data:vm-data/instance-id=iid-9");
        // The encoded payloads decode back to the rendered seed documents.
        assert_eq!(ud, base64_encode(seed.user_data.as_bytes()));
        assert_eq!(md, base64_encode(seed.meta_data.as_bytes()));
        // No raw newlines leaked into argv (base64 keeps it one xe-safe token).
        assert!(!argv[2].contains('\n'));
        assert!(!argv[3].contains('\n'));
    }

    #[test]
    fn is_ipv4_guards() {
        assert!(is_ipv4("10.42.0.9"));
        assert!(!is_ipv4("10.42.0")); // 3 octets
        assert!(!is_ipv4("10.42.0.999")); // >255
        assert!(!is_ipv4("fe80::1"));
    }

    // ── XPA-1: right-size memory + the over-commit precheck ──

    #[test]
    fn argv_memory_set_pins_a_fixed_allocation() {
        let argv = argv_memory_set("u-7", DEFAULT_SERVER_VM_MEM_BYTES);
        assert_eq!(argv[0], "vm-memory-limits-set");
        assert_eq!(argv[1], "uuid=u-7");
        // All four limits are the SAME fixed value (non-ballooning) and satisfy
        // static-min ≤ dynamic-min ≤ dynamic-max ≤ static-max trivially.
        let b = (2u64 * 1024 * 1024 * 1024).to_string();
        assert_eq!(argv[2], format!("static-min={b}"));
        assert_eq!(argv[3], format!("static-max={b}"));
        assert_eq!(argv[4], format!("dynamic-min={b}"));
        assert_eq!(argv[5], format!("dynamic-max={b}"));
    }

    #[test]
    fn precheck_host_memory_math() {
        // 2 GiB free, asking for one 2 GiB VM + 512 MiB headroom → over by the
        // headroom, so it must FAIL early (the XPA-1 over-commit case).
        let two_gib_kib = 2 * 1024 * 1024;
        let cap = HostCapacity {
            mem_free_kib: two_gib_kib,
            ..HostCapacity::default()
        };
        let per_vm = DEFAULT_SERVER_VM_MEM_BYTES / 1024;
        assert!(precheck_host_memory(&cap, per_vm, 1, HOST_MEM_HEADROOM_KIB).is_err());
        // Exactly enough (VM + headroom) → Ok.
        let cap_ok = HostCapacity {
            mem_free_kib: per_vm + HOST_MEM_HEADROOM_KIB,
            ..HostCapacity::default()
        };
        assert!(precheck_host_memory(&cap_ok, per_vm, 1, HOST_MEM_HEADROOM_KIB).is_ok());
        // The original report: a 15.9 GB host, 4×4 GB VMs — must fail.
        let host = HostCapacity {
            mem_free_kib: 15_900 * 1024,
            ..HostCapacity::default()
        };
        let four_gib_kib = 4 * 1024 * 1024;
        let err = precheck_host_memory(&host, four_gib_kib, 4, HOST_MEM_HEADROOM_KIB)
            .expect_err("4×4 GB over-commits a 15.9 GB host");
        assert!(err.contains("insufficient host free memory"), "{err}");
    }

    // ── XPA-4: reset clone VIF MACs ──

    #[test]
    fn vif_argv_builders_shape() {
        assert_eq!(
            argv_vif_list("vm-1"),
            vec![
                "vif-list",
                "vm-uuid=vm-1",
                "params=uuid,device,MAC,network-uuid"
            ]
        );
        assert_eq!(argv_vif_destroy("vif-9"), vec!["vif-destroy", "uuid=vif-9"]);
        assert_eq!(
            argv_vif_create("vm-1", "net-2", "0", "02:ab:cd:ef:01:23"),
            vec![
                "vif-create",
                "vm-uuid=vm-1",
                "network-uuid=net-2",
                "device=0",
                "mac=02:ab:cd:ef:01:23"
            ]
        );
    }

    #[test]
    fn parse_vif_list_decodes_records() {
        let out = "\
uuid ( RO)         : vif-aaaa
    vm-uuid ( RO): vm-1
    device ( RO): 0
    MAC ( RO): aa:bb:cc:dd:ee:ff
    network-uuid ( RO): net-7

uuid ( RO)         : vif-bbbb
    device ( RO): 1
    MAC ( RO): aa:bb:cc:dd:ee:00
    network-uuid ( RO): net-8
";
        let vifs = parse_vif_list(out);
        assert_eq!(vifs.len(), 2);
        assert_eq!(vifs[0].uuid, "vif-aaaa");
        assert_eq!(vifs[0].device, "0");
        assert_eq!(vifs[0].mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(vifs[0].network_uuid, "net-7");
        assert_eq!(vifs[1].device, "1");
    }

    #[test]
    fn mac_for_clone_is_locally_administered_unique_and_stable() {
        let m = mac_for_clone("uuid-xyz", "0");
        // Shape: 6 lowercase-hex octets.
        let octets: Vec<&str> = m.split(':').collect();
        assert_eq!(octets.len(), 6, "{m}");
        assert!(octets
            .iter()
            .all(|o| o.len() == 2 && o.chars().all(|c| c.is_ascii_hexdigit())));
        // First octet: multicast bit clear (even) + locally-administered bit set.
        let first = u8::from_str_radix(octets[0], 16).unwrap();
        assert_eq!(first & 0x01, 0x00, "unicast (multicast bit clear)");
        assert_eq!(first & 0x02, 0x02, "locally-administered bit set");
        // Stable for the same inputs…
        assert_eq!(mac_for_clone("uuid-xyz", "0"), m);
        // …distinct per device, and per clone uuid (no collisions across clones).
        assert_ne!(mac_for_clone("uuid-xyz", "1"), m);
        assert_ne!(mac_for_clone("uuid-other", "0"), m);
    }

    // ── XPA-7: the clone self-joins via a v3 token in its cloud-init seed ──

    #[test]
    fn mackesd_join_argv_shape() {
        assert_eq!(
            mackesd_join_argv("mesh:m@1.2.3.4:4243#b?fp=ab", "server"),
            vec![
                "mackesd",
                "join",
                "mesh:m@1.2.3.4:4243#b?fp=ab",
                "--role",
                "server"
            ]
        );
    }

    #[test]
    fn build_join_seed_embeds_a_no_shell_self_join_runcmd() {
        let token = "mesh:home@203.0.113.5:4243#BEARERxyz?fp=deadbeef";
        let seed = build_join_seed("web1", "ssh-ed25519 KEY op@h", "iid-1", token, "server");
        // It's still a first-boot identity seed (hostname + machine-id reset)…
        assert!(seed.user_data.contains("hostname: MDE-VM-web1"));
        assert!(seed.user_data.contains("reset-machine-id"));
        // …plus the XPA-7 self-join, rendered as an EXEC LIST (no `sh -c`), so the
        // token's `#`/`?` are passed verbatim to mackesd (no shell interpretation).
        assert!(
            seed.user_data.contains(&format!(
                "[ 'mackesd', 'join', '{token}', '--role', 'server' ]"
            )),
            "join runcmd missing/!exec-list: {}",
            seed.user_data
        );
        // The reset-machine-id runcmd is ordered BEFORE the join (fresh id first).
        let reset_at = seed.user_data.find("reset-machine-id").unwrap();
        let join_at = seed.user_data.find("'mackesd', 'join'").unwrap();
        assert!(reset_at < join_at, "machine-id reset must precede the join");
        // A plain identity seed (no token) carries NO join runcmd.
        let plain = build_identity_seed("web1", "ssh-ed25519 KEY op@h", "iid-1");
        assert!(!plain.user_data.contains("mackesd"));
    }
}
