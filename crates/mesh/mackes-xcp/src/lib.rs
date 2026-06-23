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
//! the side-effecting [`Runner`] is the thin shell around it. A native rustls
//! XAPI backend can later implement [`Hypervisor`] behind the same trait.

use std::process::Command;

use thiserror::Error;

/// Where (and how) to run `xe`.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    },
}

impl HostTarget {
    /// A remote SSH target with the conventional `root` dom0 login.
    #[must_use]
    pub fn ssh_root(host: impl Into<String>, identity: Option<String>) -> Self {
        Self::Ssh {
            host: host.into(),
            user: "root".to_string(),
            identity,
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
    let hostname = mde_vm_hostname(name);
    let key = op_ssh_key.trim();
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
         \x20\x20- [ cloud-init-per, once, reset-machine-id, sh, -c, 'truncate -s 0 /etc/machine-id && rm -f /var/lib/dbus/machine-id' ]\n"
    );
    let meta_data = format!("instance-id: {instance_id}\nlocal-hostname: {hostname}\n");
    IdentitySeed {
        user_data,
        meta_data,
        instance_id: instance_id.to_string(),
    }
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

/// Build the full executable + argv for a target: `xe <args…>` locally, or
/// `ssh [-i id] -o BatchMode=yes user@host xe <args…>` remotely. Pure + tested.
#[must_use]
pub fn full_command(target: &HostTarget, xe_args: &[String]) -> (String, Vec<String>) {
    match target {
        HostTarget::Local => ("xe".to_string(), xe_args.to_vec()),
        HostTarget::Ssh {
            host,
            user,
            identity,
        } => {
            let mut argv = Vec::new();
            if let Some(id) = identity {
                argv.push("-i".to_string());
                argv.push(id.clone());
            }
            argv.push("-o".to_string());
            argv.push("BatchMode=yes".to_string());
            argv.push("-o".to_string());
            argv.push("StrictHostKeyChecking=accept-new".to_string());
            argv.push(format!("{user}@{host}"));
            argv.push("xe".to_string());
            argv.extend(xe_args.iter().cloned());
            ("ssh".to_string(), argv)
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
    fn run(&self, xe_args: &[String]) -> Result<String, XcpError> {
        let (exe, argv) = full_command(&self.target, xe_args);
        let cmd_name = xe_args.first().cloned().unwrap_or_default();
        let out = Command::new(&exe)
            .args(&argv)
            .output()
            .map_err(|e| XcpError::Spawn(exe.clone(), e))?;
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
}
