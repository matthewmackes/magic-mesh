//! VIRT-6 (v5.0.0) — `compute_provision`: create a KVM VM from a
//! Bus request.
//!
//! Each peer drains its own `compute/create/<own-nebula-addr>`
//! topic. For each request it runs the full provision flow
//! (operator design locks 2026-05-30, recorded in the worklist):
//!
//! 1. **Ensure the `mde-vms` storage pool** (VIRT-3) — define +
//!    start + autostart if absent; idempotent no-op otherwise.
//! 2. **Allocate the VM's Nebula IP** (lock 1) — a per-peer
//!    deterministic /24 derived from the peer's own overlay IP
//!    (`3rd octet = 128 + own 4th octet`), picking the lowest-free
//!    host from local inventory. No coordination, no central
//!    allocator; the ≤8-peer cap guarantees /24s never overlap.
//! 3. **Requester-side keygen** (lock 5) — `nebula-cert keygen`
//!    locally; the private key never leaves this peer. Only the
//!    public key is sent to `cert_authority`.
//! 4. **Cert RPC** (VIRT-5) — publish to
//!    `action/compute/cert-sign-request` with the public key,
//!    `await` the `reply/<ulid>` (30 s, one retry) → `{cert_pem,
//!    ca_pem}`.
//! 5. **Render the guest Nebula config** — `render_guest_config_yaml`
//!    (peer config minus the host-only VM-subnet route).
//! 6. **Build the NoCloud seed** (lock 2) — `--cloud-init
//!    user-data=…,meta-data=…` with `write_files` for
//!    `/etc/nebula/{host.key,host.crt,ca.crt,config.yml}` + a
//!    `runcmd` enabling nebula in the guest.
//! 7. **virt-install** — with `--filesystem` + `--memorybacking
//!    access.mode=shared` when `share_meshfs` AND `/mnt/mesh-storage`
//!    is mounted (lock 3). When asked but unmounted: create WITHOUT
//!    the share + flag `meshfs_skipped` (lock 4).
//! 8. **Reply** on `compute/create-ack/<request-ulid>` with
//!    `{vm_id, nebula_ip, meshfs_skipped?}` or `{error}`.
//! 9. **Immediate inventory publish** via
//!    `compute_registry::snapshot_inventory` so the new VM appears
//!    inside the §3 5 s budget rather than waiting a 10 s tick.
//!
//! VIRT-4.b (the VM-subnet route going live) needs no action here:
//! VIRT-4.a renders the `10.42.128.0/17` `unsafe_route` into every
//! host peer's config unconditionally, and `nebula_supervisor`
//! re-materializes `/etc/nebula/config.yaml` + reloads nebula on its
//! own tick — so the route is present + live independent of VM
//! creation. The design doc §10 "push on first VM" is mechanically
//! subsumed by always-render.
//!
//! ## Async / Persist
//!
//! `Persist` is `!Sync`, so it is never held across an `.await`.
//! The create-topic read is a short sync open-read-drop on each
//! tick; each provision runs on `spawn_blocking` (it shells out +
//! synchronously polls the cert reply for up to 30 s) with its own
//! `Persist` handle, keeping the async run-loop responsive.
//!
//! The actual VM-boots-and-joins-mesh behavior is HW-bench-gated
//! (VIRT-12 + §0.15); the pure helpers below (IP allocation, pool
//! args, cloud-init build, virt-install args, cert RPC, meshfs
//! decision, ack build) are fully unit-tested here.

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use base64::Engine as _;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

use super::cert_authority::ACTION_TOPIC as CERT_SIGN_TOPIC;
use super::compute_registry;
use super::nebula_supervisor;
use super::{ShutdownToken, Worker};

/// Topic prefix this worker subscribes to (suffix = own overlay IP).
pub const CREATE_TOPIC_PREFIX: &str = "compute/create/";

/// Reply-topic prefix for create acks (suffix = request ULID).
pub const CREATE_ACK_PREFIX: &str = "compute/create-ack/";

/// Default poll cadence — control surface.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// Nebula overlay interface name.
pub const DEFAULT_NEBULA_INTERFACE: &str = "nebula1";

/// VM disk + ISO storage root.
pub const DEFAULT_VM_STORAGE: &str = "/var/lib/mde-vms";

/// MeshFS mount checked for the `--filesystem` attach decision.
pub const DEFAULT_MESHFS_MOUNT: &str = "/mnt/mesh-storage";

/// libvirt storage pool name (VIRT-3).
pub const POOL_NAME: &str = "mde-vms";

/// virtiofs target tag the guest mounts (`mount -t virtiofs
/// mesh-storage /mnt/mesh-storage`).
pub const MESHFS_VIRTIOFS_TAG: &str = "mesh-storage";

/// Default Nebula group every VM cert carries.
pub const DEFAULT_NEBULA_GROUP: &str = "mde-vms";

/// Cert-sign RPC timeout (design doc §3 / VIRT-5 bullet 4).
pub const CERT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Cert-sign RPC poll cadence while awaiting the reply.
pub const CERT_RPC_POLL: Duration = Duration::from_millis(250);

/// Create-request payload per design doc §3.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateRequest {
    /// Workbench-generated correlation ULID; the ack lands on
    /// `compute/create-ack/<request_ulid>`.
    pub request_ulid: String,
    /// libvirt domain name (also used to derive the VM id).
    pub name: String,
    /// Virtual CPUs.
    pub vcpus: u32,
    /// RAM in MiB.
    pub ram_mb: u64,
    /// Disk size in GiB.
    pub disk_gb: u64,
    /// Optional installer / cloud-image ISO path.
    #[serde(default)]
    pub iso_path: Option<String>,
    /// Whether to attach the MeshFS virtiofs share.
    #[serde(default)]
    pub share_meshfs: bool,
}

/// Create-ack payload — `{vm_id, nebula_ip, meshfs_skipped?}` on
/// success, `{error}` on failure.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateAck {
    /// libvirt domain id/name on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
    /// Allocated Nebula overlay IP on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nebula_ip: Option<String>,
    /// `true` when `share_meshfs` was requested but `/mnt/mesh-storage`
    /// wasn't mounted, so the VM was created without the share
    /// (lock 4). Omitted when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub meshfs_skipped: bool,
    /// Error description on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// Parse a create-request body.
///
/// # Errors
///
/// Returns a human-readable error string on malformed JSON / missing
/// required fields.
pub fn parse_create_request(body: &str) -> Result<CreateRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed create request: {e}"))
}

/// The VM /24's third octet for this peer (lock 1):
/// `128 + (peer's 4th octet)`. Requires the peer to be a
/// `10.42.x.y` address with `y <= 127` (so `128 + y <= 255` stays
/// inside the VM `/17`, third octet 128..255). Returns `None`
/// otherwise — the caller then fails the create with a clear error
/// rather than allocating into the wrong subnet.
///
/// With the ≤8-peer cap and standard sequential enrollment
/// (`10.42.0.1..8`) the per-peer /24s never overlap. (If a future
/// mesh spans multiple peer /24s, harden this to the full host
/// index — tracked as VIRT-6.followup.)
#[must_use]
pub fn vm_subnet_third_octet(own_addr: &str) -> Option<u8> {
    let ip = own_addr.split('/').next().unwrap_or(own_addr);
    let octets: Vec<&str> = ip.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let o0: u8 = octets[0].parse().ok()?;
    let o1: u8 = octets[1].parse().ok()?;
    let o3: u8 = octets[3].parse().ok()?;
    if o0 != 10 || o1 != 42 || o3 > 127 {
        return None;
    }
    Some(128 + o3)
}

/// Extract the set of in-use host octets (the 4th octet) from a list
/// of VM IPs that fall in `10.42.<third>.0/24`. IPs outside that /24
/// are ignored.
#[must_use]
pub fn used_hosts_in_subnet(known_vm_ips: &[String], third_octet: u8) -> BTreeSet<u8> {
    let prefix = format!("10.42.{third_octet}.");
    known_vm_ips
        .iter()
        .filter_map(|ip| {
            let bare = ip.split('/').next().unwrap_or(ip);
            let host = bare.strip_prefix(&prefix)?;
            host.parse::<u8>().ok()
        })
        .collect()
}

/// Allocate the lowest-free VM IP in `10.42.<third>.0/24`, skipping
/// `.0` (network) and `.255` (broadcast) and any host in
/// `used_hosts`. Returns the bare IP (no CIDR suffix), or `None`
/// when the /24 is exhausted.
#[must_use]
pub fn allocate_vm_ip(third_octet: u8, used_hosts: &BTreeSet<u8>) -> Option<String> {
    (1u8..=254)
        .find(|h| !used_hosts.contains(h))
        .map(|h| format!("10.42.{third_octet}.{h}"))
}

/// Decide whether to attach the virtiofs MeshFS share + whether the
/// request asked for it but couldn't get it (lock 4). Returns
/// `(attach, skipped)`.
#[must_use]
pub fn meshfs_attach_decision(share_meshfs: bool, meshfs_mounted: bool) -> (bool, bool) {
    let attach = share_meshfs && meshfs_mounted;
    let skipped = share_meshfs && !meshfs_mounted;
    (attach, skipped)
}

/// `nebula-cert keygen` args (requester-side keygen, lock 5).
#[must_use]
pub fn build_keygen_args(out_key: &str, out_pub: &str) -> Vec<String> {
    vec![
        "keygen".into(),
        "-out-key".into(),
        out_key.into(),
        "-out-pub".into(),
        out_pub.into(),
    ]
}

/// `virsh pool-list --all --name` args (idempotency pre-check).
#[must_use]
pub fn build_pool_list_args() -> Vec<String> {
    vec!["pool-list".into(), "--all".into(), "--name".into()]
}

/// `true` when `pool` appears in a `virsh pool-list --all --name`
/// payload (one pool name per line).
#[must_use]
pub fn pool_exists(pool_list_stdout: &str, pool: &str) -> bool {
    pool_list_stdout.lines().map(str::trim).any(|l| l == pool)
}

/// `virsh pool-define-as` args per VIRT-3 (positional dir-pool form).
#[must_use]
pub fn build_pool_define_args(vm_storage: &str) -> Vec<String> {
    vec![
        "pool-define-as".into(),
        POOL_NAME.into(),
        "dir".into(),
        "-".into(),
        "-".into(),
        "-".into(),
        "-".into(),
        vm_storage.into(),
    ]
}

/// `virsh pool-start mde-vms` args.
#[must_use]
pub fn build_pool_start_args() -> Vec<String> {
    vec!["pool-start".into(), POOL_NAME.into()]
}

/// `virsh pool-autostart mde-vms` args.
#[must_use]
pub fn build_pool_autostart_args() -> Vec<String> {
    vec!["pool-autostart".into(), POOL_NAME.into()]
}

/// Build the cloud-init NoCloud `meta-data` document.
#[must_use]
pub fn build_meta_data(instance_id: &str, hostname: &str) -> String {
    format!("instance-id: {instance_id}\nlocal-hostname: {hostname}\n")
}

/// Build the cloud-init NoCloud `user-data` document (lock 2). The
/// four `/etc/nebula` files are base64-encoded (`encoding: b64`) so
/// PEM/YAML newlines survive without fragile YAML block-scalar
/// indentation, and a `runcmd` enables nebula in the guest.
#[must_use]
pub fn build_cloud_init_user_data(
    host_key_pem: &str,
    host_crt_pem: &str,
    ca_crt_pem: &str,
    guest_config_yml: &str,
) -> String {
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    format!(
        "#cloud-config\n\
         write_files:\n\
         \x20 - path: /etc/nebula/host.key\n\
         \x20   encoding: b64\n\
         \x20   permissions: '0600'\n\
         \x20   content: {key}\n\
         \x20 - path: /etc/nebula/host.crt\n\
         \x20   encoding: b64\n\
         \x20   permissions: '0644'\n\
         \x20   content: {crt}\n\
         \x20 - path: /etc/nebula/ca.crt\n\
         \x20   encoding: b64\n\
         \x20   permissions: '0644'\n\
         \x20   content: {ca}\n\
         \x20 - path: /etc/nebula/config.yml\n\
         \x20   encoding: b64\n\
         \x20   permissions: '0644'\n\
         \x20   content: {cfg}\n\
         runcmd:\n\
         \x20 - [ systemctl, enable, --now, nebula ]\n",
        key = b64(host_key_pem),
        crt = b64(host_crt_pem),
        ca = b64(ca_crt_pem),
        cfg = b64(guest_config_yml),
    )
}

/// Build the `virt-install` argument vector (lock 2 + lock 3). The
/// precise flag set is HW-bench-tunable (VIRT-12); this assembles a
/// coherent baseline: name, memory, vcpus, the pool-backed qcow2
/// disk, optional install ISO, default NAT NIC, generic os-variant,
/// the NoCloud seed, and — when `attach_meshfs` — the libvirt-managed
/// virtiofs filesystem + shared memory backing it requires.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_virt_install_args(
    req: &CreateRequest,
    vm_id: &str,
    disk_path: &str,
    user_data_path: &str,
    meta_data_path: &str,
    attach_meshfs: bool,
    meshfs_mount: &str,
) -> Vec<String> {
    let mut args = vec![
        "--name".into(),
        vm_id.into(),
        "--memory".into(),
        req.ram_mb.to_string(),
        "--vcpus".into(),
        req.vcpus.to_string(),
        "--disk".into(),
        format!(
            "path={disk_path},size={},format=qcow2,pool={POOL_NAME}",
            req.disk_gb
        ),
        "--network".into(),
        "network=default".into(),
        "--osinfo".into(),
        "detect=on,name=generic".into(),
        "--graphics".into(),
        "spice".into(),
        "--noautoconsole".into(),
    ];
    if let Some(iso) = &req.iso_path {
        args.push("--cdrom".into());
        args.push(iso.clone());
    }
    if attach_meshfs {
        // libvirt launches + supervises virtiofsd itself when the
        // domain has a virtiofs <filesystem> + shared-memory backing.
        args.push("--memorybacking".into());
        args.push("access.mode=shared".into());
        args.push("--filesystem".into());
        args.push(format!(
            "{meshfs_mount},{MESHFS_VIRTIOFS_TAG},driver.type=virtiofs"
        ));
    }
    args.push("--cloud-init".into());
    args.push(format!(
        "user-data={user_data_path},meta-data={meta_data_path}"
    ));
    args
}

/// Build the cert-sign-request body (VIRT-5 caller side, lock 5).
#[must_use]
pub fn build_cert_sign_request_body(
    common_name: &str,
    ip_cidr: &str,
    public_key_pem: &str,
) -> String {
    let body = serde_json::json!({
        "common_name": common_name,
        "ip": ip_cidr,
        "groups": [DEFAULT_NEBULA_GROUP],
        "public_key_pem": public_key_pem,
    });
    body.to_string()
}

/// Build a success create-ack JSON body.
#[must_use]
pub fn build_create_ack_ok(vm_id: &str, nebula_ip: &str, meshfs_skipped: bool) -> String {
    let ack = CreateAck {
        vm_id: Some(vm_id.to_string()),
        nebula_ip: Some(nebula_ip.to_string()),
        meshfs_skipped,
        error: None,
    };
    serde_json::to_string(&ack).unwrap_or_else(|_| r#"{"error":"ack encode failed"}"#.into())
}

/// Build an error create-ack JSON body.
#[must_use]
pub fn build_create_ack_error(message: &str) -> String {
    let ack = CreateAck {
        vm_id: None,
        nebula_ip: None,
        meshfs_skipped: false,
        error: Some(message.to_string()),
    };
    serde_json::to_string(&ack).unwrap_or_else(|_| r#"{"error":"ack encode failed"}"#.into())
}

/// Scan `<vm_storage>/*.nebula-ip` sidecar files (written here at
/// create time + read by compute_registry) for the IPs already
/// assigned to this peer's VMs. Best-effort: unreadable dir → empty.
#[must_use]
pub fn scan_local_vm_ips(vm_storage: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(vm_storage) else {
        return vec![];
    };
    let mut ips = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n.ends_with(".nebula-ip"))
        {
            if let Ok(s) = std::fs::read_to_string(&path) {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    ips.push(trimmed.to_string());
                }
            }
        }
    }
    ips
}

/// Synchronously wait for a reply on `reply/<request_ulid>`. Mirrors
/// `rpc::await_reply` but sync so `Persist` (which is `!Sync`) is
/// never held across an `.await`. Returns the reply body string.
///
/// # Errors
///
/// `Err("timeout …")` when no reply lands within `timeout`.
pub fn await_reply_sync(
    persist: &Persist,
    request_ulid: &str,
    timeout: Duration,
    poll: Duration,
) -> Result<String, String> {
    let topic = reply_topic(request_ulid);
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(msgs) = persist.list_since(&topic, None) {
            if let Some(first) = msgs.into_iter().next() {
                return Ok(first.body.unwrap_or_default());
            }
        }
        if Instant::now() >= deadline {
            return Err(format!("no reply on {topic} within {timeout:?}"));
        }
        std::thread::sleep(poll);
    }
}

fn binary_present(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
}

fn run_virsh_status(args: &[String]) -> bool {
    Command::new("virsh")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_virsh_stdout(args: &[String]) -> String {
    Command::new("virsh")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Ensure the `mde-vms` pool exists (VIRT-3). Idempotent.
///
/// # Errors
///
/// Propagates a description when define/start fails.
fn ensure_pool(vm_storage: &str) -> Result<(), String> {
    let listing = run_virsh_stdout(&build_pool_list_args());
    if pool_exists(&listing, POOL_NAME) {
        return Ok(());
    }
    if !run_virsh_status(&build_pool_define_args(vm_storage)) {
        return Err(format!("virsh pool-define-as {POOL_NAME} failed"));
    }
    if !run_virsh_status(&build_pool_start_args()) {
        return Err(format!("virsh pool-start {POOL_NAME} failed"));
    }
    let _ = run_virsh_status(&build_pool_autostart_args());
    Ok(())
}

fn local_nebula_addr(interface: &str) -> String {
    let Ok(output) = Command::new("ip")
        .args(["-4", "addr", "show", interface])
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix("inet ") {
            if let Some(ip) = rest.split('/').next() {
                return ip.to_string();
            }
        }
    }
    String::new()
}

/// All the owned, `Send` context a blocking provision needs.
#[derive(Clone)]
struct ProvisionCtx {
    bus_root: PathBuf,
    workgroup_root: PathBuf,
    node_id: String,
    hostname: String,
    own_addr: String,
    vm_storage: PathBuf,
    meshfs_mount: PathBuf,
}

/// Run one provision end-to-end on a blocking thread. Writes the
/// create-ack + (on success) the immediate inventory publish + the
/// VIRT-4.b route push. Never panics; all failures become an
/// error-ack.
fn run_create_blocking(ctx: ProvisionCtx, req: CreateRequest) {
    let persist = match Persist::open(ctx.bus_root.clone()) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "compute_provision: persist open failed; cannot ack");
            return;
        }
    };
    let ack_topic = format!("{CREATE_ACK_PREFIX}{}", req.request_ulid);
    let ack_body = match provision(&ctx, &persist, &req) {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!(req = %req.request_ulid, error = %e, "compute_provision: create failed");
            build_create_ack_error(&e)
        }
    };
    if let Err(e) = persist.write(&ack_topic, Priority::Default, None, Some(&ack_body)) {
        tracing::warn!(error = %e, topic = ack_topic, "compute_provision: ack write failed");
    }
}

/// The provision flow proper. Returns the success-ack body on
/// success; `Err(description)` becomes an error-ack in the caller.
fn provision(ctx: &ProvisionCtx, persist: &Persist, req: &CreateRequest) -> Result<String, String> {
    if !binary_present("virt-install") {
        return Err("virt-install not available on this peer".into());
    }

    // 1. Storage pool.
    ensure_pool(&ctx.vm_storage.to_string_lossy())?;

    // 2. Allocate the VM IP in this peer's /24.
    let third = vm_subnet_third_octet(&ctx.own_addr)
        .ok_or_else(|| format!("cannot derive VM /24 from peer addr {:?}", ctx.own_addr))?;
    let used = used_hosts_in_subnet(&scan_local_vm_ips(&ctx.vm_storage), third);
    let nebula_ip = allocate_vm_ip(third, &used)
        .ok_or_else(|| format!("VM subnet 10.42.{third}.0/24 is full"))?;
    let ip_cidr = format!("{nebula_ip}/17");

    let vm_id = format!("vm-{}", req.request_ulid);

    // 3. Requester-side keygen (key stays local).
    let tmp_dir = std::env::temp_dir().join("mde-vm-provision").join(&vm_id);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("mkdir tmp: {e}"))?;
    let key_path = tmp_dir.join("host.key");
    let pub_path = tmp_dir.join("host.pub");
    let keygen_ok = run_virsh_keygen(&key_path, &pub_path);
    if !keygen_ok {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("nebula-cert keygen failed".into());
    }
    let host_key_pem = std::fs::read_to_string(&key_path).map_err(|e| format!("read key: {e}"))?;
    let public_key_pem =
        std::fs::read_to_string(&pub_path).map_err(|e| format!("read pub: {e}"))?;

    // 4. Cert RPC — request, await reply (one retry).
    let (cert_pem, ca_pem) =
        request_cert(persist, &vm_id, &ip_cidr, &public_key_pem).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            e
        })?;

    // 5. Guest Nebula config (peer config minus the host-only route).
    //    The lighthouse roster comes from this peer's own bundle.
    let bundle_path = crate::ca::bundle::bundle_path(&ctx.workgroup_root, &ctx.node_id);
    let bundle = crate::ca::bundle::read_bundle(&bundle_path)
        .map_err(|e| format!("load host nebula bundle {}: {e}", bundle_path.display()))?;
    let guest_config = nebula_supervisor::render_guest_config_yaml(&bundle);

    // 6. cloud-init seed.
    let user_data = build_cloud_init_user_data(&host_key_pem, &cert_pem, &ca_pem, &guest_config);
    let meta_data = build_meta_data(&vm_id, &req.name);
    let ud_path = tmp_dir.join("user-data");
    let md_path = tmp_dir.join("meta-data");
    std::fs::write(&ud_path, &user_data).map_err(|e| format!("write user-data: {e}"))?;
    std::fs::write(&md_path, &meta_data).map_err(|e| format!("write meta-data: {e}"))?;

    // 7. virt-install (lock 3 + lock 4).
    let meshfs_mounted = compute_registry::is_meshfs_mounted(&ctx.meshfs_mount);
    let (attach_meshfs, meshfs_skipped) = meshfs_attach_decision(req.share_meshfs, meshfs_mounted);
    let disk_path = ctx
        .vm_storage
        .join(format!("{vm_id}.qcow2"))
        .to_string_lossy()
        .into_owned();
    let args = build_virt_install_args(
        req,
        &vm_id,
        &disk_path,
        &ud_path.to_string_lossy(),
        &md_path.to_string_lossy(),
        attach_meshfs,
        &ctx.meshfs_mount.to_string_lossy(),
    );
    let install_ok = Command::new("virt-install")
        .args(&args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !install_ok {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!("virt-install failed for {vm_id}"));
    }

    // Sidecar so compute_registry reports nebula_ip + future
    // allocations see this host octet as used.
    let sidecar = ctx.vm_storage.join(format!("{vm_id}.nebula-ip"));
    let _ = std::fs::write(&sidecar, &nebula_ip);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // 9. Immediate inventory publish (≤5 s budget).
    let inv = compute_registry::snapshot_inventory(
        &ctx.hostname,
        &ctx.own_addr,
        &ctx.meshfs_mount,
        &ctx.vm_storage,
    );
    compute_registry::publish_inventory(&ctx.own_addr, &inv);

    Ok(build_create_ack_ok(&vm_id, &nebula_ip, meshfs_skipped))
}

fn run_virsh_keygen(key_path: &Path, pub_path: &Path) -> bool {
    Command::new("nebula-cert")
        .args(build_keygen_args(
            &key_path.to_string_lossy(),
            &pub_path.to_string_lossy(),
        ))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Publish the cert-sign request + await the reply, retrying once on
/// timeout (VIRT-5 bullet 4). Returns `(cert_pem, ca_pem)`.
fn request_cert(
    persist: &Persist,
    vm_id: &str,
    ip_cidr: &str,
    public_key_pem: &str,
) -> Result<(String, String), String> {
    let body = build_cert_sign_request_body(vm_id, ip_cidr, public_key_pem);
    for attempt in 0..2 {
        let ulid = publish_request(
            persist,
            CERT_SIGN_TOPIC,
            Priority::Default,
            None,
            Some(&body),
        )
        .map_err(|e| format!("publish cert-sign request: {e}"))?;
        match await_reply_sync(persist, &ulid, CERT_RPC_TIMEOUT, CERT_RPC_POLL) {
            Ok(reply_body) => {
                let v: serde_json::Value = serde_json::from_str(&reply_body)
                    .map_err(|e| format!("parse cert reply: {e}"))?;
                if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                    return Err(format!("cert authority error: {err}"));
                }
                let cert = v
                    .get("cert_pem")
                    .and_then(|c| c.as_str())
                    .ok_or("cert reply missing cert_pem")?;
                let ca = v
                    .get("ca_pem")
                    .and_then(|c| c.as_str())
                    .ok_or("cert reply missing ca_pem")?;
                return Ok((cert.to_string(), ca.to_string()));
            }
            Err(e) if attempt == 0 => {
                tracing::warn!(vm = %vm_id, error = %e, "compute_provision: cert request timed out; retrying once");
            }
            Err(e) => return Err(format!("cert request failed after retry: {e}")),
        }
    }
    Err("cert request exhausted retries".into())
}

/// Read the new create requests on this peer's topic since `cursor`.
/// Opens + drops a `Persist` synchronously so it never crosses an
/// `.await` in the run-loop.
fn read_new_creates(
    bus_root: &Path,
    topic: &str,
    cursor: &mut Option<String>,
) -> Vec<CreateRequest> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(topic, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_create_request(body) {
            Ok(req) => out.push(req),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "compute_provision: bad create request")
            }
        }
    }
    out
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Worker handle.
pub struct ComputeProvisionWorker {
    hostname: String,
    workgroup_root: PathBuf,
    node_id: String,
    nebula_interface: String,
    nebula_addr_hint: String,
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
    vm_storage: PathBuf,
    meshfs_mount: PathBuf,
}

impl ComputeProvisionWorker {
    /// Construct with production defaults. `workgroup_root` + `node_id`
    /// locate this peer's `nebula-bundle.json` (for the guest
    /// lighthouse roster).
    #[must_use]
    pub fn new(hostname: String, workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            hostname,
            workgroup_root,
            node_id,
            nebula_interface: DEFAULT_NEBULA_INTERFACE.into(),
            nebula_addr_hint: String::new(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            vm_storage: PathBuf::from(DEFAULT_VM_STORAGE),
            meshfs_mount: PathBuf::from(DEFAULT_MESHFS_MOUNT),
        }
    }

    /// Override the local peer's Nebula address (skips `ip addr`).
    #[must_use]
    pub fn with_nebula_addr_hint(mut self, addr: String) -> Self {
        self.nebula_addr_hint = addr;
        self
    }

    /// Override the Bus root. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    fn resolve_addr(&self) -> String {
        if !self.nebula_addr_hint.is_empty() {
            return self.nebula_addr_hint.clone();
        }
        local_nebula_addr(&self.nebula_interface)
    }

    fn ctx(&self, own_addr: String, bus_root: PathBuf) -> ProvisionCtx {
        ProvisionCtx {
            bus_root,
            workgroup_root: self.workgroup_root.clone(),
            node_id: self.node_id.clone(),
            hostname: self.hostname.clone(),
            own_addr,
            vm_storage: self.vm_storage.clone(),
            meshfs_mount: self.meshfs_mount.clone(),
        }
    }
}

#[async_trait::async_trait]
impl Worker for ComputeProvisionWorker {
    fn name(&self) -> &'static str {
        "compute_provision"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("compute_provision: no bus root; worker idle");
                return Ok(());
            }
        };
        let mut cursor: Option<String> = None;
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let own = self.resolve_addr();
                    if own.is_empty() {
                        continue;
                    }
                    let topic = format!("{CREATE_TOPIC_PREFIX}{own}");
                    // Sync read — Persist opened + dropped inside; never
                    // crosses the await below.
                    let new_reqs = read_new_creates(&bus_root, &topic, &mut cursor);
                    for req in new_reqs {
                        let ctx = self.ctx(own.clone(), bus_root.clone());
                        // Each provision shells out + polls the cert
                        // reply (up to 30 s) on a blocking thread so the
                        // async run-loop stays responsive.
                        if let Err(e) =
                            tokio::task::spawn_blocking(move || run_create_blocking(ctx, req)).await
                        {
                            tracing::warn!(error = %e, "compute_provision: provision task join failed");
                        }
                    }
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_create_request ──

    #[test]
    fn parse_create_happy_path() {
        let body = r#"{"request_ulid":"01JAN","name":"dev","vcpus":2,"ram_mb":2048,"disk_gb":20,"iso_path":"/isos/f.iso","share_meshfs":true}"#;
        let req = parse_create_request(body).expect("parse");
        assert_eq!(req.request_ulid, "01JAN");
        assert_eq!(req.name, "dev");
        assert_eq!(req.vcpus, 2);
        assert_eq!(req.ram_mb, 2048);
        assert_eq!(req.disk_gb, 20);
        assert_eq!(req.iso_path.as_deref(), Some("/isos/f.iso"));
        assert!(req.share_meshfs);
    }

    #[test]
    fn parse_create_optional_fields_default() {
        let body = r#"{"request_ulid":"01","name":"d","vcpus":1,"ram_mb":512,"disk_gb":10}"#;
        let req = parse_create_request(body).expect("parse");
        assert!(req.iso_path.is_none());
        assert!(!req.share_meshfs);
    }

    #[test]
    fn parse_create_rejects_malformed() {
        assert!(parse_create_request("nope").is_err());
    }

    // ── vm_subnet_third_octet (lock 1) ──

    #[test]
    fn third_octet_is_128_plus_fourth() {
        assert_eq!(vm_subnet_third_octet("10.42.0.1"), Some(129));
        assert_eq!(vm_subnet_third_octet("10.42.0.2"), Some(130));
        assert_eq!(vm_subnet_third_octet("10.42.0.8"), Some(136));
        // CIDR suffix tolerated.
        assert_eq!(vm_subnet_third_octet("10.42.0.5/17"), Some(133));
    }

    #[test]
    fn third_octet_none_for_non_mesh_addr() {
        assert_eq!(vm_subnet_third_octet("192.168.1.1"), None);
        assert_eq!(vm_subnet_third_octet("10.43.0.1"), None);
        assert_eq!(vm_subnet_third_octet("garbage"), None);
        // 4th octet > 127 would overflow the VM /17 — refused.
        assert_eq!(vm_subnet_third_octet("10.42.0.200"), None);
    }

    // ── used_hosts_in_subnet + allocate_vm_ip (lock 1) ──

    #[test]
    fn used_hosts_filters_to_the_subnet() {
        let ips = vec![
            "10.42.129.1".to_string(),
            "10.42.129.5/17".to_string(),
            "10.42.130.1".to_string(), // different /24 — ignored
            "garbage".to_string(),
        ];
        let used = used_hosts_in_subnet(&ips, 129);
        assert!(used.contains(&1));
        assert!(used.contains(&5));
        assert!(!used.contains(&130));
        assert_eq!(used.len(), 2);
    }

    #[test]
    fn allocate_picks_lowest_free_host() {
        let mut used = BTreeSet::new();
        used.insert(1u8);
        used.insert(2u8);
        used.insert(4u8);
        assert_eq!(allocate_vm_ip(129, &used), Some("10.42.129.3".into()));
    }

    #[test]
    fn allocate_first_host_when_empty() {
        let used = BTreeSet::new();
        assert_eq!(allocate_vm_ip(136, &used), Some("10.42.136.1".into()));
    }

    #[test]
    fn allocate_none_when_subnet_full() {
        let used: BTreeSet<u8> = (1u8..=254).collect();
        assert_eq!(allocate_vm_ip(129, &used), None);
    }

    // ── Required scenarios 4 + 5: meshfs decision (lock 4) ──

    #[test]
    fn meshfs_decision_attaches_when_requested_and_mounted() {
        assert_eq!(meshfs_attach_decision(true, true), (true, false));
    }

    #[test]
    fn meshfs_unavailable_with_share_skips_not_attaches() {
        // share requested, not mounted → don't attach, flag skipped.
        assert_eq!(meshfs_attach_decision(true, false), (false, true));
    }

    #[test]
    fn meshfs_unavailable_without_share_neither_attaches_nor_skips() {
        assert_eq!(meshfs_attach_decision(false, false), (false, false));
    }

    #[test]
    fn meshfs_not_requested_but_mounted_does_not_attach() {
        assert_eq!(meshfs_attach_decision(false, true), (false, false));
    }

    // ── pool helpers (VIRT-3) ──

    #[test]
    fn pool_exists_detects_name() {
        assert!(pool_exists("default\nmde-vms\nimages\n", "mde-vms"));
        assert!(!pool_exists("default\nimages\n", "mde-vms"));
    }

    #[test]
    fn pool_define_args_match_virt3_form() {
        let args = build_pool_define_args("/var/lib/mde-vms");
        assert_eq!(
            args,
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
    }

    // ── keygen args (lock 5) ──

    #[test]
    fn keygen_args_request_key_and_pub() {
        let args = build_keygen_args("/t/host.key", "/t/host.pub");
        assert_eq!(
            args,
            vec![
                "keygen",
                "-out-key",
                "/t/host.key",
                "-out-pub",
                "/t/host.pub"
            ]
        );
    }

    // ── cert-sign request body (lock 5) ──

    #[test]
    fn cert_request_body_carries_pubkey_and_default_group() {
        let body = build_cert_sign_request_body("vm-01", "10.42.129.1/17", "PUBKEY");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["common_name"], "vm-01");
        assert_eq!(v["ip"], "10.42.129.1/17");
        assert_eq!(v["public_key_pem"], "PUBKEY");
        assert_eq!(v["groups"][0], "mde-vms");
    }

    // ── cloud-init build (lock 2) ──

    #[test]
    fn cloud_init_round_trips_pem_via_base64() {
        let ud = build_cloud_init_user_data("KEYDATA\n", "CRTDATA\n", "CADATA\n", "cfg: yes\n");
        assert!(ud.starts_with("#cloud-config\n"));
        assert!(ud.contains("/etc/nebula/host.key"));
        assert!(ud.contains("encoding: b64"));
        assert!(ud.contains("permissions: '0600'")); // key is tight
        assert!(ud.contains("systemctl"));
        // The key content must base64-decode back to the input.
        let b64 = base64::engine::general_purpose::STANDARD.encode("KEYDATA\n".as_bytes());
        assert!(ud.contains(&b64));
    }

    #[test]
    fn meta_data_has_instance_id_and_hostname() {
        let md = build_meta_data("vm-01JAN", "dev-server");
        assert!(md.contains("instance-id: vm-01JAN"));
        assert!(md.contains("local-hostname: dev-server"));
    }

    // ── virt-install args (lock 2 + lock 3) ──

    fn sample_req(share_meshfs: bool, iso: Option<&str>) -> CreateRequest {
        CreateRequest {
            request_ulid: "01JAN".into(),
            name: "dev".into(),
            vcpus: 2,
            ram_mb: 2048,
            disk_gb: 20,
            iso_path: iso.map(String::from),
            share_meshfs,
        }
    }

    #[test]
    fn virt_install_args_baseline_shape() {
        let req = sample_req(false, Some("/isos/f.iso"));
        let args = build_virt_install_args(
            &req,
            "vm-01JAN",
            "/var/lib/mde-vms/vm-01JAN.qcow2",
            "/tmp/ud",
            "/tmp/md",
            false,
            "/mnt/mesh-storage",
        );
        assert!(args.contains(&"--name".to_string()));
        assert!(args.contains(&"vm-01JAN".to_string()));
        assert!(args.contains(&"--memory".to_string()));
        assert!(args.contains(&"2048".to_string()));
        assert!(args.contains(&"--vcpus".to_string()));
        assert!(args.contains(&"--cdrom".to_string()));
        assert!(args.contains(&"/isos/f.iso".to_string()));
        assert!(args.contains(&"--cloud-init".to_string()));
        assert!(args.iter().any(|a| a.contains("user-data=/tmp/ud")));
        // No meshfs flags when not attaching.
        assert!(!args.contains(&"--filesystem".to_string()));
        assert!(!args.contains(&"--memorybacking".to_string()));
    }

    #[test]
    fn virt_install_args_attach_meshfs_adds_filesystem_and_shared_mem() {
        let req = sample_req(true, None);
        let args = build_virt_install_args(
            &req,
            "vm-01",
            "/var/lib/mde-vms/vm-01.qcow2",
            "/tmp/ud",
            "/tmp/md",
            true,
            "/mnt/mesh-storage",
        );
        assert!(args.contains(&"--memorybacking".to_string()));
        assert!(args.contains(&"access.mode=shared".to_string()));
        assert!(args.contains(&"--filesystem".to_string()));
        assert!(args
            .iter()
            .any(|a| a.contains("/mnt/mesh-storage,mesh-storage,driver.type=virtiofs")));
        // No --cdrom when no iso.
        assert!(!args.contains(&"--cdrom".to_string()));
    }

    // ── create-ack shapes ──

    #[test]
    fn create_ack_ok_shape() {
        let body = build_create_ack_ok("vm-01", "10.42.129.1", false);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["vm_id"], "vm-01");
        assert_eq!(v["nebula_ip"], "10.42.129.1");
        assert!(!v.as_object().unwrap().contains_key("error"));
        // meshfs_skipped omitted when false.
        assert!(!v.as_object().unwrap().contains_key("meshfs_skipped"));
    }

    #[test]
    fn create_ack_ok_includes_meshfs_skipped_when_true() {
        let body = build_create_ack_ok("vm-01", "10.42.129.1", true);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["meshfs_skipped"], true);
    }

    #[test]
    fn create_ack_error_shape() {
        let body = build_create_ack_error("virt-install failed for vm-01");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("virt-install"));
        assert!(!v.as_object().unwrap().contains_key("vm_id"));
    }

    // ── scan_local_vm_ips ──

    #[test]
    fn scan_local_vm_ips_reads_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("vm-a.nebula-ip"), "10.42.129.1\n").unwrap();
        std::fs::write(tmp.path().join("vm-b.nebula-ip"), "10.42.129.7").unwrap();
        std::fs::write(tmp.path().join("vm-a.qcow2"), b"disk").unwrap(); // ignored
        let mut ips = scan_local_vm_ips(tmp.path());
        ips.sort();
        assert_eq!(
            ips,
            vec!["10.42.129.1".to_string(), "10.42.129.7".to_string()]
        );
    }

    #[test]
    fn scan_local_vm_ips_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan_local_vm_ips(tmp.path()).is_empty());
    }

    // ── Required scenario 2: cert-request timeout ──

    #[test]
    fn await_reply_sync_times_out_when_no_reply() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let err = await_reply_sync(
            &persist,
            "01NONEXISTENT",
            Duration::from_millis(60),
            Duration::from_millis(10),
        )
        .expect_err("should time out");
        assert!(err.contains("no reply"), "{err}");
    }

    #[test]
    fn await_reply_sync_returns_body_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let ulid = "01HASREPLY";
        persist
            .write(
                &reply_topic(ulid),
                Priority::Default,
                None,
                Some("{\"cert_pem\":\"X\"}"),
            )
            .expect("write reply");
        let body = await_reply_sync(
            &persist,
            ulid,
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .expect("reply present");
        assert!(body.contains("cert_pem"));
    }

    // ── Required scenario 1: happy-path plan compose ──

    #[test]
    fn happy_path_plan_compose() {
        // The provision flow is a deterministic composition of the
        // pure helpers; this asserts the planned chain so a
        // regression in any link breaks visibly.
        let own = "10.42.0.3";
        let third = vm_subnet_third_octet(own).expect("third");
        assert_eq!(third, 131);
        let used = used_hosts_in_subnet(&["10.42.131.1".into()], third);
        let ip = allocate_vm_ip(third, &used).expect("ip");
        assert_eq!(ip, "10.42.131.2");
        let req = sample_req(true, Some("/isos/f.iso"));
        let (attach, skipped) = meshfs_attach_decision(req.share_meshfs, true);
        assert!(attach && !skipped);
        let body = build_cert_sign_request_body("vm-01JAN", &format!("{ip}/17"), "PUB");
        assert!(body.contains("10.42.131.2/17"));
        let ack = build_create_ack_ok("vm-01JAN", &ip, skipped);
        assert!(ack.contains("vm-01JAN"));
    }

    // ── topic-prefix locks ──

    #[test]
    fn topic_prefixes_match_design_doc() {
        assert_eq!(CREATE_TOPIC_PREFIX, "compute/create/");
        assert_eq!(CREATE_ACK_PREFIX, "compute/create-ack/");
        // The cert RPC reuses the VIRT-5 action topic.
        assert!(CERT_SIGN_TOPIC.starts_with("action/"));
    }
}
