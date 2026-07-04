//! QC-4 — the one-state → Kolla config renderer.
//!
//! Turns this node's doctrine view into the per-service
//! `/etc/kolla/<svc>/config.json` (+ the service config it points to) that lets
//! each MVP Kolla container start.
//!
//! The one-state doctrine (design Q30) is authoritative and *rendered*, not
//! hand-deployed: [`super::fleet::MeshFleetState`] reads the doctrine, and
//! this module materializes the Kolla config from it — parameterized by the
//! pinned release, the leader bit (Q15 — the DB is leader-hosted, so the
//! leader's services reach it locally while every other node reaches it over
//! mesh-DNS), and — QC-6 — the node's **resolved Nebula overlay address**
//! (Q22/23 — every API binds plaintext to the overlay IP only; the overlay IS
//! the transport security). A node not on the mesh yet has no overlay IP to
//! bind to, so the render gates the service ([`RenderError::OverlayUnresolved`])
//! rather than fall back to `0.0.0.0`/localhost — a control-plane API is never
//! exposed on the public underlay (§7).
//!
//! Honesty (§7):
//! - the render is **atomic** — every file lands via a tmp-write + rename, and
//!   `config.json` (the [`super::podman::config_rendered`] gate marker) is
//!   written *last*, so a failure mid-render never leaves a half-written
//!   config a container would crash-loop on;
//! - a render failure is a typed [`RenderError`] the reconcile turns into a
//!   `Gated` mirror row with a sharp reason — never a silent partial success.
//!
//! MVP scope (design Q24): the minimal config each foundation/identity/core
//! service needs to *start*. Real service credentials are sealed by QC-5
//! ([`super::secrets`]): the renderer reads the sealed [`SecretView`] off the
//! [`RenderCtx`] and substitutes each service's genuine password into its
//! connection strings. When the set isn't sealed yet (a non-leader before the
//! leader's file has synced), the render returns [`RenderError::SecretsUnsealed`]
//! and the reconcile gates that service — never a blank or fabricated secret
//! (§7).

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use super::capacity::{derive_flavors, derive_quotas, NodeCapacity};
use super::catalog::ServiceKind;
use super::podman::kolla_config_dir;
use super::secrets::{SecretView, Secrets};

/// Mesh-DNS name the leader-hosted `MariaDB` (Q15) answers on — resolves to the
/// current leader over the overlay (Q46, Designate/peer-directory), so a
/// failover moves the name, not the config.
const DB_MESH_NAME: &str = "mariadb.mesh";
/// Mesh-DNS name the clustered `RabbitMQ` (Q16) answers on.
const RABBIT_MESH_NAME: &str = "rabbitmq.mesh";
/// Mesh-DNS name Keystone answers on (Q21/22).
const KEYSTONE_MESH_NAME: &str = "keystone.mesh";
/// AMQP port.
const RABBIT_PORT: u16 = 5672;
/// memcached port (per-node cache, Q17).
const MEMCACHE_PORT: u16 = 11211;
/// Keystone public API port.
const KEYSTONE_PORT: u16 = 5000;

// ── QC-7: the Neutron ML2/OVN flat-mesh network (Q42/43/44/49) ──
/// OVN northbound OVSDB port — Neutron's ML2/OVN driver writes the logical
/// network here; `ovn-northd` reads it.
const OVN_NB_PORT: u16 = 6641;
/// OVN southbound OVSDB port — `ovn-northd` compiles logical → physical flows
/// here and every chassis's `ovn-controller` reads them.
const OVN_SB_PORT: u16 = 6642;
/// Mesh-DNS name the leader-hosted OVN northbound DB (Q15) answers on — resolves
/// to the current leader over the overlay (Q46, like `mariadb.mesh`).
const OVN_NB_MESH_NAME: &str = "ovn-nb.mesh";
/// Mesh-DNS name the leader-hosted OVN southbound DB answers on.
const OVN_SB_MESH_NAME: &str = "ovn-sb.mesh";
/// The single flat provider network's physnet label (Q43 — one flat provider
/// network bridged into the mesh; every instance a peer-equivalent). Matches the
/// `flat_networks` list, the `bridge_mappings`, and the chassis
/// `ovn-bridge-mappings`.
const MESH_PHYSNET: &str = "mesh";
/// The OVS provider bridge the flat physnet maps to — patched to the Nebula
/// interface so an instance on the flat net gets a mesh-reachable address (Q43).
const MESH_PROVIDER_BRIDGE: &str = "br-mesh";
/// The tenant/instance MTU on the flat net, set for Geneve-over-Nebula double
/// encap (Q49): OVN tunnels flat-net east-west between chassis over geneve on
/// the Nebula overlay, so the 38-byte geneve header comes off the mesh underlay
/// (≈ 1342). Rendered as Neutron's `global_physnet_mtu`.
const MESH_NET_MTU: u16 = 1342;

// ── QC-8: the Cinder LVM backend + cinder-backup to the object tier (Q51/56/57/59) ──
/// The LVM volume group cinder carves on **each node's writable partition**
/// (Q59) — the block backend is node-local (Q51), so every node runs its own VG
/// of the same name, sliced from the writable partition beside the Swift dir and
/// the Nova ephemeral pool.
const CINDER_VOLUME_GROUP: &str = "cinder-volumes";
/// Mesh-DNS name the Keystone-native **Swift** hot object tier (Q55) answers on
/// — resolved over the overlay (QC-6 idiom, like `keystone.mesh`). cinder-backup
/// streams volume backups here (Q57); Swift replicates the ring off-site to DO
/// Spaces (Q54 — the two-tier store), so the single cinder `backup_driver`
/// targets the hot tier and the off-site leg rides Swift's own replication, not
/// a second cinder driver.
const SWIFT_MESH_NAME: &str = "swift.mesh";
/// The Swift proxy port the object API answers on (the cinder-backup target).
const SWIFT_PORT: u16 = 8080;
/// The Swift container cinder-backup lands each volume's backup objects in.
const CINDER_BACKUP_CONTAINER: &str = "volumebackups";

// ── QC-9: the Glance local-file store + replication/caching (Q36/53) ──
/// The on-disk **local file store** every API node's glance-api serves images
/// from (Q53 — a node-local file store, carved beside the Cinder VG + Swift dir
/// on the writable partition, Q59). An image lands here once and is replicated
/// to every other API node's store by the QC-9 pipeline
/// ([`super::image_pipeline`]).
const GLANCE_STORE_DATADIR: &str = "/var/lib/glance/images/";
/// The per-node **image cache** directory (Q53 — caching between API nodes): a
/// hot image served off a peer's store is cached locally so the next serve is
/// node-local. Distinct from the store — the store is authoritative, the cache
/// is disposable.
const GLANCE_IMAGE_CACHE_DIR: &str = "/var/lib/glance/image-cache/";
/// The per-node image-cache ceiling in bytes (20 GiB) — the cache pruner trims
/// to this, so the cache never fills the writable partition (§7 — a real bound,
/// not an unbounded cache masquerading as one).
const GLANCE_IMAGE_CACHE_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
/// Glance's `image_cache_stall_time` (seconds) — how long a half-fetched cache
/// entry may sit before the cleaner reaps it (a day).
const GLANCE_IMAGE_CACHE_STALL_SECS: u32 = 86_400;
/// The `[glance_store]` local file-store name (matches the `--store` the QC-9
/// upload targets and the `stores`/`default_store` this renders).
const GLANCE_FILE_STORE: &str = "file";

// ── QC-19: wave-2 services — Heat, Octavia, Horizon (Q25/47/61) ──
/// The Octavia health-manager's amphora-heartbeat UDP listen port — bound to
/// this node's overlay IP (Q23), never the public underlay.
const OCTAVIA_HEALTH_MANAGER_PORT: u16 = 5555;
/// The Horizon dashboard's Apache listen port on the overlay (Q23). Horizon is
/// a web console, not a Keystone-catalog API, so it carries no `api_port`.
const HORIZON_PORT: u16 = 80;
/// The rendered fleet Heat stack's filename, written beside the QC-10 cloud
/// bootstrap seed under `<config_root>/bootstrap/` (design Q61 — the worker
/// renders stacks, Heat executes).
const FLEET_HEAT_STACK_FILE: &str = "fleet-stack.yaml";
/// The stack name the leader creates the fleet-inventory stack under, so
/// `openstack stack list` shows one authoritative, fleet-derived stack.
const FLEET_HEAT_STACK_NAME: &str = "mcnf-fleet";

/// This node's Nebula overlay bind (design Q22/23) — the resolved overlay IP
/// every `OpenStack` API binds plaintext to (the overlay IS the transport
/// security), or the honest reason it couldn't be resolved.
///
/// A socket binds to an address, not a name: QC-6 resolves this node's live
/// overlay IP from the canonical publish file `nebula_supervisor` writes (the
/// same source `sshd_overlay_bind`/`cups_sync`/`boot_readiness` bind to) and
/// threads it here. When the node isn't on the mesh yet, the render **gates**
/// every service with the reason — never a `0.0.0.0`/`127.0.0.1` fallback that
/// would expose a control-plane API on the public underlay (§7 / Q23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayBind {
    /// The resolved Nebula overlay IPv4 the APIs bind + advertise on.
    Resolved(String),
    /// The overlay address couldn't be resolved — the sharp reason (the node
    /// isn't enrolled / no overlay IP published yet). Gates the render.
    Unresolved(String),
}

/// The node-local render inputs folded from the doctrine (design Q30).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderCtx {
    /// The pinned Kolla release (Q69) — echoed into the config for provenance.
    pub release: String,
    /// This node holds the etcd leader lease → it hosts `MariaDB` (Q15) and its
    /// own services reach the DB locally; a non-leader reaches it via mesh-DNS.
    pub leader: bool,
    /// This node's Nebula overlay bind (QC-6, Q22/23) — the resolved overlay IP
    /// every API binds/advertises on + the leader's local DB/cache target, or
    /// the honest unresolved reason that gates the render.
    pub overlay: OverlayBind,
    /// The QC-5 sealed per-service secrets this tick ([`SecretView`]). `Sealed`
    /// → the renderer substitutes real passwords; `Unsealed` → the render gates
    /// the service (never a blank credential, §7).
    pub secrets: SecretView,
}

impl RenderCtx {
    /// A context with no live doctrine (a `Disabled`/`Gated` tick converges no
    /// starts, so nothing is rendered — the overlay + secrets are both left
    /// unresolved).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            release: String::new(),
            leader: false,
            overlay: OverlayBind::Unresolved(
                "no cloud doctrine active — the overlay bind is not resolved for a \
                 Disabled/Gated tick"
                    .to_string(),
            ),
            secrets: SecretView::Unsealed(
                "no cloud doctrine active — secrets are not resolved for a \
                 Disabled/Gated tick"
                    .to_string(),
            ),
        }
    }
}

/// A typed render failure — carried into the mirror as a `Gated` reason (§7).
#[derive(Debug, Error)]
pub enum RenderError {
    /// A config file (or its parent directory) couldn't be written.
    #[error("kolla config for {service}: writing {path} failed — {source}")]
    Io {
        /// The service being rendered.
        service: String,
        /// The path that failed.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// `config.json` couldn't be serialized.
    #[error("kolla config for {service}: serializing config.json failed — {source}")]
    Serialize {
        /// The service being rendered.
        service: String,
        /// The underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// The QC-5 per-service secrets aren't sealed/complete this tick — the
    /// renderer refuses to substitute a blank or fabricated credential and gates
    /// the service instead (§7). The reason names the sub-state (awaiting the
    /// leader / malformed / an incomplete set).
    #[error("kolla config: secrets not sealed — {reason}")]
    SecretsUnsealed {
        /// Why the sealed set isn't usable this tick.
        reason: String,
    },
    /// This node's Nebula overlay address couldn't be resolved (it isn't on the
    /// mesh yet) — QC-6 refuses to bind a control-plane API to `0.0.0.0`/
    /// localhost and gates the service instead (§7 / design Q23). The reason
    /// already reads as an "overlay address unresolved …" sentence.
    #[error("kolla config: {reason}")]
    OverlayUnresolved {
        /// Why the overlay bind address isn't resolvable this tick.
        reason: String,
    },
}

/// The Kolla `config.json` entrypoint descriptor (consumed by the image's
/// `KOLLA_CONFIG_STRATEGY=COPY_ALWAYS` bootstrap — [`super::podman::build_kolla_run_argv`]).
#[derive(Debug, Serialize)]
struct KollaConfig {
    /// The container's main process.
    command: String,
    /// Each mounted source file → its in-container destination.
    config_files: Vec<KollaConfigFile>,
}

/// One `config_files` entry: copy `source` (under the mounted config dir) to
/// `dest` with `owner`/`perm`.
#[derive(Debug, Serialize)]
struct KollaConfigFile {
    /// The mounted source, always under `/var/lib/kolla/config_files/`.
    source: String,
    /// The in-container destination path.
    dest: String,
    /// `<user>:<group>` the copied file is chowned to.
    owner: String,
    /// The octal permission the copied file is chmod'd to.
    perm: String,
}

/// One rendered service config file (its name under the config dir + body +
/// where the Kolla bootstrap copies it).
struct ConfFile {
    /// The filename under `<config_root>/<service>/`.
    name: &'static str,
    /// The in-container destination.
    dest: &'static str,
    /// `<user>:<group>` owner.
    owner: &'static str,
    /// The rendered body.
    body: String,
}

/// The whole plan for one service: its launch command + the config files it
/// needs (which `config.json` then references).
struct ServicePlan {
    /// The container's main process (the `config.json` `command`).
    command: String,
    /// The service config files (`config.json` `config_files` are derived from
    /// these). May be empty (memcached is command-only).
    files: Vec<ConfFile>,
}

/// Render `kind`'s Kolla config under `config_root` from the doctrine `ctx`.
///
/// Writes `<config_root>/<service>/<conf files>` then `config.json` **last**,
/// each via a tmp-write + rename. On success `config.json` exists (the
/// [`super::podman::config_rendered`] gate flips true); on failure no new
/// `config.json` is written (the gate stays false) and the prior good config,
/// if any, stands.
///
/// # Errors
/// A [`RenderError`] on an I/O failure (create dir / write / rename) or a
/// `config.json` serialization failure. The caller gates the service on it.
pub fn render_service_config(
    config_root: &Path,
    kind: ServiceKind,
    ctx: &RenderCtx,
) -> Result<(), RenderError> {
    // QC-6 — the overlay bind gate (checked first: a node off the mesh can't
    // serve at all). Every API binds plaintext to this node's Nebula overlay
    // address (Q22/23; the overlay IS the transport security). An unresolved
    // overlay gates the service with the sharp reason rather than fall back to
    // 0.0.0.0/localhost, which would expose a control-plane API on the public
    // underlay (§7).
    let overlay = match &ctx.overlay {
        OverlayBind::Resolved(ip) => ip.as_str(),
        OverlayBind::Unresolved(reason) => {
            return Err(RenderError::OverlayUnresolved {
                reason: reason.clone(),
            })
        }
    };

    // QC-5 — the sealed secrets gate. An unsealed set (a non-leader before the
    // leader's file synced, or a malformed/unreadable companion) gates the
    // service with its sharp reason; a complete Sealed set is required before we
    // ever substitute a password, so no blank credential reaches a live config.
    let secrets = match &ctx.secrets {
        SecretView::Sealed(secrets) => secrets,
        SecretView::Unsealed(reason) => {
            return Err(RenderError::SecretsUnsealed {
                reason: reason.clone(),
            })
        }
    };
    if let Some(missing) = secrets.first_missing() {
        return Err(RenderError::SecretsUnsealed {
            reason: format!(
                "the sealed secret set is missing `{missing}` — awaiting a complete \
                 re-seal from the leader (never rendering a blank credential)"
            ),
        });
    }

    let dir = kolla_config_dir(config_root, kind);
    let service = kind.container_name();
    let plan = service_plan(kind, overlay, ctx, secrets);

    // Provenance header — stamps the pinned Kolla release (Q69) the doctrine
    // rendered this config from, so a config on disk names its own release.
    let provenance = format!(
        "# rendered by mackesd openstack worker (QC-4) — kolla release {}\n",
        ctx.release
    );

    // 1 — the referenced config files (each atomic). A failure here returns
    // before config.json is (re)written, so the gate marker never points at a
    // file that isn't there.
    for f in &plan.files {
        let path = dir.join(f.name);
        write_atomic(&path, &format!("{provenance}{}", f.body)).map_err(|source| {
            RenderError::Io {
                service: service.to_string(),
                path: path.display().to_string(),
                source,
            }
        })?;
    }

    // 2 — config.json LAST (the render marker the start gates on).
    let config = KollaConfig {
        command: plan.command,
        config_files: plan
            .files
            .iter()
            .map(|f| KollaConfigFile {
                source: format!("/var/lib/kolla/config_files/{}", f.name),
                dest: f.dest.to_string(),
                owner: f.owner.to_string(),
                perm: "0600".to_string(),
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&config).map_err(|source| RenderError::Serialize {
        service: service.to_string(),
        source,
    })?;
    let path = dir.join("config.json");
    write_atomic(&path, &json).map_err(|source| RenderError::Io {
        service: service.to_string(),
        path: path.display().to_string(),
        source,
    })
}

// ─────────────────────── QC-10: the cloud bootstrap seed ───────────────────────

/// The service label the bootstrap-seed errors carry in the [`RenderError`].
const BOOTSTRAP_SERVICE: &str = "cloud-bootstrap";

/// The rendered bootstrap seed's filename under `<config_root>/bootstrap/`.
const BOOTSTRAP_SEED_NAME: &str = "cloud-bootstrap.sh";

/// Render the QC-10 **cloud bootstrap seed** from this node's real `capacity`.
///
/// The two open-cloud guardrails (design Q29/39/89) as an idempotent
/// `openstack`-CLI script the leader's cloud bootstrap applies once (Q27 — the
/// CLI rides the host image).
///
/// It carries both guardrails, each *derived from real capacity*, never fixed
/// `OpenStack` defaults:
/// - **Capacity-derived flavors** ([`super::capacity::derive_flavors`], Q39): a
///   tiny/small/medium/large ladder scaled to the node's shape, created via
///   `openstack flavor create` — the set regenerates as the fleet's capacity
///   changes.
/// - **Hard per-user quotas** ([`super::capacity::derive_quotas`], Q89): a
///   per-member ceiling that is a *fraction* of the node (so several members
///   coexist and none can claim the fleet — the ENT-12 blast-radius guardrail),
///   registered as **Keystone unified limits** — the default every project/user
///   inherits, the mesh's first hard authorization boundary. Nova enforces them
///   via the `[quota] UnifiedLimitsDriver` the [`nova`] render sets; QC-11/12
///   layer explicit per-member overrides at enrollment.
///
/// Written atomically to `<config_root>/bootstrap/cloud-bootstrap.sh`; each
/// create/register is guarded (show/list first), so re-applying on a later
/// converge is a no-op. Returns the seed path.
///
/// # Errors
/// A [`RenderError::Io`] if the seed (or its parent dir) can't be written.
pub fn render_cloud_bootstrap(
    config_root: &Path,
    release: &str,
    capacity: &NodeCapacity,
) -> Result<std::path::PathBuf, RenderError> {
    use std::fmt::Write as _;

    let flavors = derive_flavors(capacity);
    let quota = derive_quotas(capacity);

    let mut body = String::new();
    body.push_str("#!/bin/sh\n");
    // Writing to a String is infallible — the discarded fmt::Result never errors.
    let _ = writeln!(
        body,
        "# rendered by mackesd openstack worker (QC-10) — kolla release {release}"
    );
    body.push_str(
        "# Capacity-derived flavors + hard per-user quotas (design Q29/39/89).\n\
         # Applied once by the leader's cloud bootstrap over the openstack CLI\n\
         # (Q27, in the host image). Idempotent: every create/register is guarded,\n\
         # so re-running on a later converge is a no-op.\n\
         set -eu\n\n",
    );

    // ── Capacity-derived flavors (Q39) — sized from this node's real shape,
    //    NOT fixed OpenStack defaults. ──
    let _ = writeln!(
        body,
        "# Capacity-derived flavors (Q39) — this node: {} vCPU / {} MiB / {} GiB.",
        capacity.vcpus, capacity.ram_mib, capacity.disk_gib
    );
    body.push_str(
        "ensure_flavor() {  # <name> <vcpus> <ram-mib> <disk-gib>\n  \
         openstack flavor show \"$1\" >/dev/null 2>&1 && return 0\n  \
         openstack flavor create --vcpus \"$2\" --ram \"$3\" --disk \"$4\" --public \"$1\"\n}\n",
    );
    for f in &flavors {
        let _ = writeln!(
            body,
            "ensure_flavor {} {} {} {}",
            f.name, f.vcpus, f.ram_mib, f.disk_gib
        );
    }

    // ── Hard per-user quotas (Q89) — a fraction of the node, registered as
    //    Keystone unified limits (the default every project/user inherits; the
    //    mesh's first hard authz boundary). ──
    body.push_str(
        "\n# Hard per-user quotas (Q89 — the ENT-12 blast-radius guardrail: one\n\
         # member may claim at most a fraction of the fleet, HARD). Registered as\n\
         # Keystone unified limits — the default every project/user inherits; Nova\n\
         # enforces them via the UnifiedLimitsDriver in nova.conf. QC-11/12 layer\n\
         # explicit per-member overrides at enrollment.\n",
    );
    body.push_str(
        "ensure_limit() {  # <service> <resource-name> <default-limit>\n  \
         [ -n \"$(openstack registered limit list --service \"$1\" \
         --resource-name \"$2\" -f value -c ID 2>/dev/null)\" ] && return 0\n  \
         openstack registered limit create --service \"$1\" --default-limit \"$3\" \"$2\"\n}\n",
    );
    // Nova compute caps, then Cinder block-storage caps, then the Neutron FIP cap
    // — the five caps the design names (instances/vCPU/RAM/volumes/floating-IPs)
    // plus the gigabytes ceiling volumes are bounded by.
    for (service, resource, limit) in [
        ("nova", "servers", u64::from(quota.instances)),
        ("nova", "class:VCPU", u64::from(quota.vcpus)),
        ("nova", "class:MEMORY_MB", quota.ram_mib),
        ("cinder", "volumes", u64::from(quota.volumes)),
        ("cinder", "gigabytes", quota.gigabytes),
        ("neutron", "floatingip", u64::from(quota.floating_ips)),
    ] {
        let _ = writeln!(body, "ensure_limit {service} {resource} {limit}");
    }

    // ── QC-19: the fleet Heat stack (Q61 — fleet renders Heat, Heat executes) ──
    // Idempotently create the fleet-inventory stack from the template the worker
    // renders beside this seed ([`render_fleet_heat_stack`] → fleet-stack.yaml),
    // so `openstack stack list` shows one authoritative, fleet-derived stack.
    // Guarded (show-first), so re-applying on a later converge is a no-op; skips
    // silently if Heat isn't in this fleet's converged set (no template written).
    body.push_str(
        "\n# QC-19 — the fleet Heat stack (Q61): the worker renders the stack from\n\
         # fleet state, Heat executes it. Idempotent; skipped if the wave-2 Heat\n\
         # service (and so its rendered template) isn't present on this fleet.\n",
    );
    let _ = writeln!(
        body,
        "FLEET_STACK_TEMPLATE=\"$(dirname \"$0\")/{FLEET_HEAT_STACK_FILE}\"\n\
         if [ -f \"$FLEET_STACK_TEMPLATE\" ]; then\n  \
         openstack stack show {FLEET_HEAT_STACK_NAME} >/dev/null 2>&1 \\\n    \
         || openstack stack create -t \"$FLEET_STACK_TEMPLATE\" {FLEET_HEAT_STACK_NAME}\n\
         fi"
    );

    let dir = config_root.join("bootstrap");
    let path = dir.join(BOOTSTRAP_SEED_NAME);
    write_atomic(&path, &body).map_err(|source| RenderError::Io {
        service: BOOTSTRAP_SERVICE.to_string(),
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

/// Atomic tmp-write + rename, creating the parent dir (the mesh convention —
/// mirrors the `chat`/`app_sync` workers' `write_atomic`).
fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, path)
}

// ─────────────────────────── per-service configs ───────────────────────────

/// The mesh-DNS service-catalog endpoint URL an API `kind` advertises (QC-6,
/// Q22/23) — plaintext HTTP over the overlay to the service's Nebula-DNS name.
/// Only the API kinds carry one; the renderer only asks for API kinds, so the
/// empty fallback is unreachable.
fn mesh_endpoint(kind: ServiceKind) -> String {
    kind.endpoint_url().unwrap_or_default()
}

/// The DB connection URL for `svc`: the leader reaches its own local `MariaDB`
/// on its overlay IP, every other node reaches it over mesh-DNS (Q15/Q46).
/// Carries the sealed per-service DB password (QC-5).
fn db_url(svc: &str, overlay: &str, ctx: &RenderCtx, secrets: &Secrets) -> String {
    let host = if ctx.leader { overlay } else { DB_MESH_NAME };
    let pw = secrets.db_password(svc);
    format!("mysql+pymysql://{svc}:{pw}@{host}/{svc}")
}

/// The host half of an OVN NB/SB OVSDB connection string (QC-7): the leader
/// reaches its own local OVN databases on its overlay IP, every other node
/// reaches them over mesh-DNS — the OVN control plane is leader-hosted like
/// `MariaDB` (Q15/Q46), so a failover moves the name, not the config.
const fn ovn_db_host<'a>(ctx: &RenderCtx, overlay: &'a str, mesh_name: &'a str) -> &'a str {
    if ctx.leader {
        overlay
    } else {
        mesh_name
    }
}

/// The oslo.messaging transport URL (Q16 — internal RPC on `RabbitMQ`, strictly
/// separate from mde-bus per Q67). Carries the sealed `RabbitMQ` password (QC-5).
fn transport_url(secrets: &Secrets) -> String {
    let pw = secrets.rabbitmq_password();
    format!("rabbit://openstack:{pw}@{RABBIT_MESH_NAME}:{RABBIT_PORT}//")
}

/// The `[keystone_authtoken]` block every keystone-authenticated API shares
/// (Q21 — the mesh account is the cloud account). The Keystone endpoint it
/// authenticates against is a mesh-DNS URL (reached over the overlay); the
/// per-node memcache it caches tokens in is this node's overlay IP. Carries the
/// sealed service-user password (QC-5).
fn authtoken(svc: &str, overlay: &str, secrets: &Secrets) -> String {
    format!(
        "[keystone_authtoken]\n\
         www_authenticate_uri = http://{KEYSTONE_MESH_NAME}:{KEYSTONE_PORT}\n\
         auth_url = http://{KEYSTONE_MESH_NAME}:{KEYSTONE_PORT}\n\
         memcached_servers = {host}:{MEMCACHE_PORT}\n\
         auth_type = password\n\
         project_domain_name = Default\n\
         user_domain_name = Default\n\
         project_name = service\n\
         username = {svc}\n\
         password = {pw}\n",
        host = overlay,
        pw = secrets.service_user_password(svc),
    )
}

/// One `[DEFAULT]` binding the Nova API listener to the overlay (QC-6, Q22/23)
/// and wiring RPC. The listen host is the resolved overlay IP and the port is
/// the catalogued Nova API port, so the compute API answers on the mesh only.
fn api_default(overlay: &str, secrets: &Secrets) -> String {
    format!(
        "[DEFAULT]\n\
         debug = False\n\
         my_ip = {host}\n\
         osapi_compute_listen = {host}\n\
         osapi_compute_listen_port = {port}\n\
         transport_url = {rpc}\n",
        host = overlay,
        port = ServiceKind::NovaApi.api_port().unwrap_or_default(),
        rpc = transport_url(secrets),
    )
}

/// Build `kind`'s launch command + config files from the doctrine (design Q24
/// MVP service set). Real but minimal — enough for the container to come up on
/// the pinned release; QC-5 layers identity/bootstrap on top.
#[allow(clippy::too_many_lines)] // one arm per service reads best kept together
fn service_plan(
    kind: ServiceKind,
    overlay: &str,
    ctx: &RenderCtx,
    secrets: &Secrets,
) -> ServicePlan {
    let one = |command: &str,
               name: &'static str,
               dest: &'static str,
               owner: &'static str,
               body: String| {
        ServicePlan {
            command: command.to_string(),
            files: vec![ConfFile {
                name,
                dest,
                owner,
                body,
            }],
        }
    };
    match kind {
        // ── Foundation trio (Q15/16/17) ──
        ServiceKind::Mariadb => one(
            "mysqld_safe",
            "galera.cnf",
            "/etc/my.cnf.d/galera.cnf",
            "mysql:mysql",
            format!(
                "[mysqld]\n\
                 bind_address = {overlay}\n\
                 wsrep_on = OFF\n\
                 default_storage_engine = InnoDB\n\
                 max_connections = 4096\n\
                 [client]\n\
                 default_character_set = utf8\n"
            ),
        ),
        ServiceKind::Rabbitmq => one(
            "rabbitmq-server",
            "rabbitmq.conf",
            "/etc/rabbitmq/rabbitmq.conf",
            "rabbitmq:rabbitmq",
            format!(
                "listeners.tcp.default = {host}:{RABBIT_PORT}\n\
                 loopback_users = none\n\
                 default_user = openstack\n\
                 default_pass = {pw}\n",
                host = overlay,
                pw = secrets.rabbitmq_password(),
            ),
        ),
        // memcached is command-only (no config file) — bound to the overlay IP.
        ServiceKind::Memcached => ServicePlan {
            command: format!("/usr/bin/memcached -vv -l {overlay}"),
            files: Vec::new(),
        },
        // ── Identity + core APIs (Q21/24), on every node (Q22) ──
        ServiceKind::Keystone => one(
            "/usr/sbin/httpd -DFOREGROUND",
            "keystone.conf",
            "/etc/keystone/keystone.conf",
            "keystone:keystone",
            format!(
                "[DEFAULT]\ndebug = False\n\
                 public_endpoint = {endpoint}\nadmin_endpoint = {endpoint}\n\
                 [database]\nconnection = {db}\n\
                 [cache]\nbackend = oslo_cache.memcache_pool\nenabled = True\n\
                 memcache_servers = {host}:{MEMCACHE_PORT}\n\
                 [token]\nprovider = fernet\n",
                endpoint = mesh_endpoint(ServiceKind::Keystone),
                db = db_url("keystone", overlay, ctx, secrets),
                host = overlay,
            ),
        ),
        ServiceKind::GlanceApi => glance(overlay, ctx, secrets),
        ServiceKind::PlacementApi => one(
            "/usr/sbin/httpd -DFOREGROUND",
            "placement.conf",
            "/etc/placement/placement.conf",
            "placement:placement",
            format!(
                "[DEFAULT]\ndebug = False\n\
                 [placement_database]\nconnection = {db}\n{authtoken}",
                db = db_url("placement", overlay, ctx, secrets),
                authtoken = authtoken("placement", overlay, secrets),
            ),
        ),
        ServiceKind::NovaApi => nova("nova-api", overlay, ctx, secrets),
        ServiceKind::NovaScheduler => nova("nova-scheduler", overlay, ctx, secrets),
        ServiceKind::NovaConductor => nova("nova-conductor", overlay, ctx, secrets),
        ServiceKind::NovaCompute => nova("nova-compute", overlay, ctx, secrets),
        ServiceKind::NeutronServer => neutron(overlay, ctx, secrets),
        ServiceKind::OvnNbDb => ovn_nb_db(overlay),
        ServiceKind::OvnSbDb => ovn_sb_db(overlay),
        ServiceKind::OvnNorthd => ovn_northd(overlay, ctx),
        ServiceKind::OvnController => ovn_controller(overlay, ctx),
        ServiceKind::CinderApi => cinder("cinder-api", overlay, ctx, secrets),
        ServiceKind::CinderScheduler => cinder("cinder-scheduler", overlay, ctx, secrets),
        ServiceKind::CinderVolume => cinder("cinder-volume", overlay, ctx, secrets),
        ServiceKind::CinderBackup => cinder("cinder-backup", overlay, ctx, secrets),
        // ── Wave-2 (QC-19, Q25/47/61) ──
        ServiceKind::HeatApi => heat("heat-api", overlay, ctx, secrets),
        ServiceKind::HeatApiCfn => heat("heat-api-cfn", overlay, ctx, secrets),
        ServiceKind::HeatEngine => heat("heat-engine", overlay, ctx, secrets),
        ServiceKind::OctaviaApi => octavia("octavia-api", overlay, ctx, secrets),
        ServiceKind::OctaviaWorker => octavia("octavia-worker", overlay, ctx, secrets),
        ServiceKind::OctaviaHealthManager => {
            octavia("octavia-health-manager", overlay, ctx, secrets)
        }
        ServiceKind::OctaviaHousekeeping => octavia("octavia-housekeeping", overlay, ctx, secrets),
        ServiceKind::Horizon => horizon(overlay, secrets),
    }
}

/// The Glance plan (Q36/53 — the image service on every API node, QC-6/Q22): a
/// node-local **file store** with **replication + caching between API nodes**.
///
/// - `[DEFAULT]`: the overlay-bound API listener (QC-6, Q22/23), the mesh
///   service-catalog `public_endpoint`, and this node's
///   `worker_self_reference_url` (its own mesh endpoint) — the node identity
///   Glance stamps on an image's store location so a peer's `copy-image` import
///   knows which node's store a location lives in. The **image cache** (Q53 —
///   caching): a cache dir on the writable partition, a real byte ceiling the
///   pruner trims to, and the stall time the cleaner reaps a half-fetched entry
///   after. `enabled_import_methods` carries **`copy-image`** — the
///   interoperable-import verb that pulls an image into THIS node's local store
///   from a peer (one half of the QC-9 replication; the other is
///   `glance-replicator livecopy`, [`super::image_pipeline`]).
/// - `[glance_store]`: the node-local **file** store (Q53/Q59) at
///   [`GLANCE_STORE_DATADIR`], `0640` file perms.
/// - `[paste_deploy]`: `flavor = keystone+cachemanagement` — wires the
///   image-cache + cache-management middleware into the API pipeline so the
///   cache directory above is actually consulted (without it the cache is inert).
/// - `[database]` + `[keystone_authtoken]`: the sealed DB (QC-5/Q15) and the
///   shared Keystone authtoken (QC-5/Q21) — the same seams every API shares.
fn glance(overlay: &str, ctx: &RenderCtx, secrets: &Secrets) -> ServicePlan {
    let endpoint = mesh_endpoint(ServiceKind::GlanceApi);
    ServicePlan {
        command: "glance-api".to_string(),
        files: vec![ConfFile {
            name: "glance-api.conf",
            dest: "/etc/glance/glance-api.conf",
            owner: "glance:glance",
            body: format!(
                "[DEFAULT]\ndebug = False\nbind_host = {host}\nbind_port = {port}\n\
                 public_endpoint = {endpoint}\n\
                 worker_self_reference_url = {endpoint}\n\
                 enabled_import_methods = [glance-direct,web-download,copy-image]\n\
                 image_cache_dir = {cache_dir}\n\
                 image_cache_max_size = {cache_max}\n\
                 image_cache_stall_time = {cache_stall}\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [glance_store]\nstores = {store}\ndefault_store = {store}\n\
                 filesystem_store_datadir = {datadir}\n\
                 filesystem_store_file_perm = 0640\n\
                 [paste_deploy]\nflavor = keystone+cachemanagement\n",
                host = overlay,
                port = ServiceKind::GlanceApi.api_port().unwrap_or_default(),
                cache_dir = GLANCE_IMAGE_CACHE_DIR,
                cache_max = GLANCE_IMAGE_CACHE_MAX_BYTES,
                cache_stall = GLANCE_IMAGE_CACHE_STALL_SECS,
                db = db_url("glance", overlay, ctx, secrets),
                authtoken = authtoken("glance", overlay, secrets),
                store = GLANCE_FILE_STORE,
                datadir = GLANCE_STORE_DATADIR,
            ),
        }],
    }
}

/// The shared Nova plan (all four nova services read one `nova.conf`; only the
/// command differs — Q31 Nova+Placement own VM lifecycle). The API listener
/// binds to the overlay ([`api_default`]); the cross-service references
/// (Glance images, Placement, Keystone) are mesh-DNS endpoints reached over the
/// overlay (QC-6, Q22). The `[quota]` block selects the **`UnifiedLimitsDriver`**
/// (QC-10, Q89) so Nova enforces the Keystone unified limits the
/// [`render_cloud_bootstrap`] seed registers — the hard per-user boundary is a
/// real enforcement path, not just a declared number.
fn nova(command: &str, overlay: &str, ctx: &RenderCtx, secrets: &Secrets) -> ServicePlan {
    ServicePlan {
        command: command.to_string(),
        files: vec![ConfFile {
            name: "nova.conf",
            dest: "/etc/nova/nova.conf",
            owner: "nova:nova",
            body: format!(
                "{default}\
                 [api_database]\nconnection = {api_db}\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [glance]\napi_servers = {glance_ep}\n\
                 [placement]\nauth_type = password\nauth_url = http://{ks}:{ksp}\n\
                 username = placement\npassword = {placement_pw}\n\
                 user_domain_name = Default\nproject_domain_name = Default\n\
                 project_name = service\n\
                 [libvirt]\nvirt_type = kvm\n\
                 [quota]\ndriver = nova.quota.UnifiedLimitsDriver\n",
                default = api_default(overlay, secrets),
                api_db = db_url("nova_api", overlay, ctx, secrets),
                db = db_url("nova", overlay, ctx, secrets),
                authtoken = authtoken("nova", overlay, secrets),
                glance_ep = mesh_endpoint(ServiceKind::GlanceApi),
                placement_pw = secrets.service_user_password("placement"),
                ks = KEYSTONE_MESH_NAME,
                ksp = KEYSTONE_PORT,
            ),
        }],
    }
}

/// The shared Cinder plan (QC-8 — LVM per node + cinder-backup to the object
/// tier; Q51/56/57/59). All four cinder services (api/scheduler/volume/backup)
/// read this one `cinder.conf`; only the launch command differs.
///
/// - `[DEFAULT]`: the overlay-bound volume-API listener (QC-6, Q22/23), the RPC
///   transport (Q16), `enabled_backends = lvm`, and the **cinder-backup** config
///   (Q57) — the Keystone-native **Swift** hot object tier (Q55) as the
///   `backup_driver`, reached over the mesh by its Nebula-DNS name (`swift.mesh`;
///   Swift replicates the ring off-site to DO Spaces, Q54 — the two-tier store's
///   off-site leg rides Swift's own replication, not a second cinder driver).
/// - `[lvm]`: the node-local LVM backend (Q51) — the volume group carved on this
///   node's writable partition (Q59), **thin**-provisioned for efficient
///   snapshots (Q56), served over an iSCSI/LIO target whose portal binds to this
///   node's overlay IP (QC-6, Q23 — a peer attaches the volume over the mesh,
///   never the public underlay).
fn cinder(command: &str, overlay: &str, ctx: &RenderCtx, secrets: &Secrets) -> ServicePlan {
    ServicePlan {
        command: command.to_string(),
        files: vec![ConfFile {
            name: "cinder.conf",
            dest: "/etc/cinder/cinder.conf",
            owner: "cinder:cinder",
            body: format!(
                "[DEFAULT]\ndebug = False\nmy_ip = {host}\n\
                 osapi_volume_listen = {host}\nosapi_volume_listen_port = {port}\n\
                 transport_url = {rpc}\nenabled_backends = lvm\n\
                 backup_driver = cinder.backup.drivers.swift.SwiftBackupDriver\n\
                 backup_swift_url = http://{swift}:{swift_port}/v1/AUTH_\n\
                 backup_swift_auth = per_user\nbackup_swift_auth_version = 3\n\
                 backup_swift_container = {backup_container}\n\
                 backup_compression_algorithm = zstd\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [lvm]\nvolume_backend_name = lvm\n\
                 volume_driver = cinder.volume.drivers.lvm.LVMVolumeDriver\n\
                 volume_group = {vg}\nlvm_type = thin\n\
                 target_protocol = iscsi\ntarget_helper = lioadm\n\
                 target_ip_address = {host}\n",
                host = overlay,
                port = ServiceKind::CinderApi.api_port().unwrap_or_default(),
                rpc = transport_url(secrets),
                swift = SWIFT_MESH_NAME,
                swift_port = SWIFT_PORT,
                backup_container = CINDER_BACKUP_CONTAINER,
                vg = CINDER_VOLUME_GROUP,
                db = db_url("cinder", overlay, ctx, secrets),
                authtoken = authtoken("cinder", overlay, secrets),
            ),
        }],
    }
}

// ─────────────────────── QC-7: Neutron ML2/OVN flat mesh ───────────────────────

/// The Neutron server plan (Q42/43/44/49): ML2 with the **OVN** mechanism over
/// **one flat provider network** bridged into the mesh — deliberately **no
/// tenant overlay** (`tenant_network_types` is empty; `type_drivers` is `flat`),
/// so an instance attaches to the flat net and gets a mesh-reachable address, a
/// peer-equivalent "inside" the mesh with no per-instance cert (Q44).
///
/// - `neutron.conf`: the overlay-bound API listener (QC-6, Q22/23), the RPC
///   transport (Q16), the DB connection (Q15), the shared Keystone authtoken,
///   and `global_physnet_mtu` set for Geneve-over-Nebula double encap (Q49).
/// - `ml2_conf.ini`: the flat-only ML2 config, the `mesh:br-mesh` provider
///   bridge mapping, and the `[ovn]` section pointing at the leader-hosted OVN
///   NB/SB OVSDBs (reached locally on the leader, over mesh-DNS elsewhere).
fn neutron(overlay: &str, ctx: &RenderCtx, secrets: &Secrets) -> ServicePlan {
    let nb_host = ovn_db_host(ctx, overlay, OVN_NB_MESH_NAME);
    let sb_host = ovn_db_host(ctx, overlay, OVN_SB_MESH_NAME);
    ServicePlan {
        command: "neutron-server --config-file /etc/neutron/neutron.conf \
                  --config-file /etc/neutron/plugins/ml2/ml2_conf.ini"
            .to_string(),
        files: vec![
            ConfFile {
                name: "neutron.conf",
                dest: "/etc/neutron/neutron.conf",
                owner: "neutron:neutron",
                // Pure-L2 flat net: no L3/router service plugin (instances reach
                // the mesh directly on the flat net — Q43/44). MTU set for
                // Geneve-over-Nebula (Q49).
                body: format!(
                    "[DEFAULT]\ndebug = False\nbind_host = {host}\nbind_port = {port}\n\
                     core_plugin = ml2\nservice_plugins =\n\
                     global_physnet_mtu = {mtu}\n\
                     transport_url = {rpc}\n\
                     [database]\nconnection = {db}\n{authtoken}",
                    host = overlay,
                    port = ServiceKind::NeutronServer.api_port().unwrap_or_default(),
                    mtu = MESH_NET_MTU,
                    rpc = transport_url(secrets),
                    db = db_url("neutron", overlay, ctx, secrets),
                    authtoken = authtoken("neutron", overlay, secrets),
                ),
            },
            ConfFile {
                name: "ml2_conf.ini",
                dest: "/etc/neutron/plugins/ml2/ml2_conf.ini",
                owner: "neutron:neutron",
                // ML2/OVN, ONE flat provider net (Q42/43): mechanism_drivers =
                // ovn, type_drivers = flat, and `tenant_network_types` is empty —
                // the flat-over-mesh posture, NOT a geneve tenant overlay (Q44).
                // The `[ovn]` section binds to the leader-hosted NB/SB OVSDBs.
                body: format!(
                    "[ml2]\ntype_drivers = flat\ntenant_network_types =\n\
                     mechanism_drivers = ovn\nextension_drivers = port_security\n\
                     [ml2_type_flat]\nflat_networks = {MESH_PHYSNET}\n\
                     [securitygroup]\nenable_security_group = True\n\
                     [ovs]\nbridge_mappings = {MESH_PHYSNET}:{MESH_PROVIDER_BRIDGE}\n\
                     [ovn]\novn_nb_connection = tcp:{nb_host}:{OVN_NB_PORT}\n\
                     ovn_sb_connection = tcp:{sb_host}:{OVN_SB_PORT}\n\
                     ovn_metadata_enabled = False\n",
                ),
            },
        ],
    }
}

/// The OVN northbound OVSDB plan (QC-7, leader-only) — binds the NB DB to this
/// node's overlay IP on [`OVN_NB_PORT`], plaintext (the overlay IS the transport
/// security, Q23; `--db-nb-create-insecure-remote=yes`). Command-only: the
/// ovsdb daemon carries its whole config on the argv (like memcached).
fn ovn_nb_db(overlay: &str) -> ServicePlan {
    ServicePlan {
        command: format!(
            "/usr/share/ovn/scripts/ovn-ctl --db-nb-addr={overlay} --db-nb-port={OVN_NB_PORT} \
             --db-nb-create-insecure-remote=yes run_nb_ovsdb"
        ),
        files: Vec::new(),
    }
}

/// The OVN southbound OVSDB plan (QC-7, leader-only) — binds the SB DB to the
/// overlay IP on [`OVN_SB_PORT`], plaintext over the mesh (Q23).
fn ovn_sb_db(overlay: &str) -> ServicePlan {
    ServicePlan {
        command: format!(
            "/usr/share/ovn/scripts/ovn-ctl --db-sb-addr={overlay} --db-sb-port={OVN_SB_PORT} \
             --db-sb-create-insecure-remote=yes run_sb_ovsdb"
        ),
        files: Vec::new(),
    }
}

/// The `ovn-northd` plan (QC-7, leader-only) — translates NB → SB. It runs only
/// on the leader, where both OVSDBs are local, so it wires each to the leader's
/// overlay IP (the [`ovn_db_host`] leader branch). Command-only.
fn ovn_northd(overlay: &str, ctx: &RenderCtx) -> ServicePlan {
    let nb_host = ovn_db_host(ctx, overlay, OVN_NB_MESH_NAME);
    let sb_host = ovn_db_host(ctx, overlay, OVN_SB_MESH_NAME);
    ServicePlan {
        command: format!(
            "/usr/share/ovn/scripts/ovn-ctl --ovn-nb-db=tcp:{nb_host}:{OVN_NB_PORT} \
             --ovn-sb-db=tcp:{sb_host}:{OVN_SB_PORT} run_northd"
        ),
        files: Vec::new(),
    }
}

/// The per-chassis `ovn-controller` plan (QC-7, every node) — programs the host
/// Open vSwitch (the OVS datapath rides the image, Q12) for the flat provider
/// net. The rendered file carries the chassis external-ids the container's
/// entrypoint applies via `ovs-vsctl`: the SB DB remote (leader-hosted, reached
/// over mesh-DNS off the leader), the **geneve** inter-chassis encap on this
/// node's overlay IP (Q49 — tunnels ride the Nebula overlay), and the
/// `mesh:br-mesh` provider bridge mapping that puts an instance on the mesh.
fn ovn_controller(overlay: &str, ctx: &RenderCtx) -> ServicePlan {
    let sb_host = ovn_db_host(ctx, overlay, OVN_SB_MESH_NAME);
    ServicePlan {
        command: "/usr/bin/ovn-controller unix:/run/openvswitch/db.sock".to_string(),
        files: vec![ConfFile {
            name: "ovn-controller.conf",
            dest: "/etc/ovn/ovn-controller.conf",
            owner: "root:root",
            body: format!(
                "# QC-7 chassis external-ids for the host Open vSwitch (Q12 — the\n\
                 # OVS datapath rides the image; ovn-controller programs it).\n\
                 # Applied by the container entrypoint via:\n\
                 #   ovs-vsctl set open . external_ids:<key>=<value>\n\
                 external_ids:ovn-remote = tcp:{sb_host}:{OVN_SB_PORT}\n\
                 external_ids:ovn-encap-type = geneve\n\
                 external_ids:ovn-encap-ip = {overlay}\n\
                 external_ids:ovn-bridge-mappings = {MESH_PHYSNET}:{MESH_PROVIDER_BRIDGE}\n",
            ),
        }],
    }
}

// ─────────────────── QC-19: wave-2 services (Q25/47/61) ───────────────────

/// The shared Heat plan (Q61 — orchestration). All three Heat services (api /
/// api-cfn / engine) read one `heat.conf`; only the launch command differs.
///
/// The fleet is authoritative: [`render_fleet_heat_stack`] renders the actual
/// stack from fleet state, and Heat (configured here) executes it. Both API
/// endpoints bind to the overlay (QC-6, Q22/23); the DB (Q15), RPC (Q16), and
/// Keystone authtoken (Q21) are the same sealed seams every API shares. The
/// `[trustee]`/`stack_domain_admin` credentials are the sealed Heat service
/// secret (QC-5) — never a blank or fabricated password.
fn heat(command: &str, overlay: &str, ctx: &RenderCtx, secrets: &Secrets) -> ServicePlan {
    let heat_pw = secrets.service_user_password("heat");
    ServicePlan {
        command: command.to_string(),
        files: vec![ConfFile {
            name: "heat.conf",
            dest: "/etc/heat/heat.conf",
            owner: "heat:heat",
            body: format!(
                "[DEFAULT]\ndebug = False\ntransport_url = {rpc}\n\
                 num_engine_workers = 4\n\
                 stack_domain_admin = heat_domain_admin\n\
                 stack_domain_admin_password = {heat_pw}\n\
                 stack_user_domain_name = heat_user_domain\n\
                 heat_metadata_server_url = http://{host}:{cfn_port}\n\
                 heat_waitcondition_server_url = http://{host}:{cfn_port}/v1/waitcondition\n\
                 [heat_api]\nbind_host = {host}\nbind_port = {api_port}\n\
                 [heat_api_cfn]\nbind_host = {host}\nbind_port = {cfn_port}\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [trustee]\nauth_type = password\nauth_url = http://{ks}:{ksp}\n\
                 username = heat\npassword = {heat_pw}\n\
                 user_domain_name = Default\n\
                 [clients_keystone]\nauth_uri = http://{ks}:{ksp}\n",
                rpc = transport_url(secrets),
                host = overlay,
                api_port = ServiceKind::HeatApi.api_port().unwrap_or_default(),
                cfn_port = ServiceKind::HeatApiCfn.api_port().unwrap_or_default(),
                db = db_url("heat", overlay, ctx, secrets),
                authtoken = authtoken("heat", overlay, secrets),
                ks = KEYSTONE_MESH_NAME,
                ksp = KEYSTONE_PORT,
            ),
        }],
    }
}

/// The shared Octavia plan (Q47 — instance-workload load-balancing). All four
/// Octavia services (api / worker / health-manager / housekeeping) read one
/// `octavia.conf`; only the launch command differs.
///
/// This wires Octavia to Keystone (Q21), the sealed DB (Q15), and RPC (Q16),
/// and binds the LB API + the health-manager heartbeat listener to the overlay
/// (QC-6, Q23). Honesty (§7): actually *serving* a load balancer additionally
/// requires the operator to provision the Octavia **amphora image**, the
/// **management network**, and the amphora **client certificates** —
/// deployment-specific IDs that do not exist until provisioned. Those are left
/// **UNSET** here (a documented precondition) rather than fabricated with
/// placeholder UUIDs; an LB create gates until the operator sets them, exactly
/// like the QC-3 archive lane gates a start on a not-yet-mirrored image.
fn octavia(command: &str, overlay: &str, ctx: &RenderCtx, secrets: &Secrets) -> ServicePlan {
    ServicePlan {
        command: command.to_string(),
        files: vec![ConfFile {
            name: "octavia.conf",
            dest: "/etc/octavia/octavia.conf",
            owner: "octavia:octavia",
            body: format!(
                "# QC-19 Octavia (Q47). The API/worker/health-manager/housekeeping are\n\
                 # wired to Keystone, the DB, and RPC below. Serving a load balancer\n\
                 # additionally needs the operator-provisioned amphora image + management\n\
                 # network + amphora client certs (deployment-specific IDs that do NOT\n\
                 # exist until provisioned) — left UNSET here rather than fabricated (§7);\n\
                 # an LB create gates until they are set.\n\
                 [DEFAULT]\ndebug = False\ntransport_url = {rpc}\n\
                 [api_settings]\nbind_host = {host}\nbind_port = {api_port}\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [service_auth]\nauth_type = password\nauth_url = http://{ks}:{ksp}\n\
                 username = octavia\npassword = {octavia_pw}\n\
                 user_domain_name = Default\nproject_domain_name = Default\n\
                 project_name = service\n\
                 [health_manager]\nbind_ip = {host}\nbind_port = {hm_port}\n\
                 [controller_worker]\nloadbalancer_topology = SINGLE\n",
                rpc = transport_url(secrets),
                host = overlay,
                api_port = ServiceKind::OctaviaApi.api_port().unwrap_or_default(),
                hm_port = OCTAVIA_HEALTH_MANAGER_PORT,
                db = db_url("octavia", overlay, ctx, secrets),
                authtoken = authtoken("octavia", overlay, secrets),
                octavia_pw = secrets.service_user_password("octavia"),
                ks = KEYSTONE_MESH_NAME,
                ksp = KEYSTONE_PORT,
            ),
        }],
    }
}

/// The Horizon plan (Q25/26/66 — the OPTIONAL dashboard). Renders the Django
/// `local_settings` bound to the Nebula overlay only (Q23), Keystone-backed
/// (Q21), with the sealed session `SECRET_KEY` (QC-5) and this node's memcached
/// as the session cache (Q17). Served by Apache in the foreground like Keystone.
///
/// Horizon is desired only when the doctrine opts in
/// ([`super::fleet::desired_services`]); when it isn't, no Horizon container is
/// converged and no mirror row appears (honest-absent — Workbench is the primary
/// Cloud UI, Q26). No DB, no Keystone service-user: Horizon acts as the logged-in
/// member over their own token.
fn horizon(overlay: &str, secrets: &Secrets) -> ServicePlan {
    ServicePlan {
        command: "/usr/sbin/httpd -DFOREGROUND".to_string(),
        files: vec![ConfFile {
            name: "local_settings",
            dest: "/etc/openstack-dashboard/local_settings",
            owner: "horizon:horizon",
            body: format!(
                "# QC-19 Horizon local_settings (Q25/26) — the OPTIONAL dashboard,\n\
                 # bound to the Nebula overlay only (Q23), Keystone-backed (Q21).\n\
                 DEBUG = False\n\
                 ALLOWED_HOSTS = ['{host}', 'horizon.mesh']\n\
                 SECRET_KEY = '{secret_key}'\n\
                 OPENSTACK_HOST = \"{ks}\"\n\
                 OPENSTACK_KEYSTONE_URL = \"http://{ks}:{ksp}/v3\"\n\
                 OPENSTACK_KEYSTONE_DEFAULT_ROLE = \"member\"\n\
                 OPENSTACK_API_VERSIONS = {{'identity': 3}}\n\
                 OPENSTACK_KEYSTONE_MULTIDOMAIN_SUPPORT = True\n\
                 WEBROOT = '/'\n\
                 SESSION_ENGINE = 'django.contrib.sessions.backends.cache'\n\
                 CACHES = {{\n    \
                 'default': {{\n        \
                 'BACKEND': 'django.core.cache.backends.memcached.PyMemcacheCache',\n        \
                 'LOCATION': '{host}:{memcache}',\n    }}\n}}\n\
                 # Apache serves the dashboard on the overlay only.\n\
                 # Listen {host}:{port}\n",
                host = overlay,
                secret_key = secrets.horizon_secret_key(),
                ks = KEYSTONE_MESH_NAME,
                ksp = KEYSTONE_PORT,
                memcache = MEMCACHE_PORT,
                port = HORIZON_PORT,
            ),
        }],
    }
}

/// Render the QC-19 **fleet Heat stack** (design Q61 — the fleet is
/// authoritative; the worker renders stacks from fleet state, Heat executes
/// them) from real fleet state.
///
/// `services` is the container-name set this node converged this tick — the
/// desired set folded from the one-state doctrine ([`super::fleet`]), a genuine
/// union of catalogued services, never hand-authored. The rendered HOT is an
/// **inert fleet-inventory stack**: it provisions nothing (`resources: {}`) and
/// records the fleet's pinned `release` + converged service set as stack
/// **outputs**, so `openstack stack {create,list}` reflect the *declared* fleet
/// without Heat ever fabricating infrastructure the doctrine didn't declare
/// (§7). Node scoping stays authoritative in the doctrine (empty ⇒ every
/// enrolled node, Q71) and is documented, not enumerated, here.
///
/// Written atomically to `<config_root>/bootstrap/fleet-stack.yaml` beside the
/// QC-10 cloud bootstrap seed (which creates the stack idempotently). Returns
/// the written path.
///
/// # Errors
/// A [`RenderError::Io`] if the template (or its parent dir) can't be written.
pub fn render_fleet_heat_stack(
    config_root: &Path,
    release: &str,
    services: &[String],
) -> Result<std::path::PathBuf, RenderError> {
    use std::fmt::Write as _;

    let mut body = String::new();
    body.push_str("heat_template_version: 2021-04-16\n");
    let _ = write!(
        body,
        "description: >\n  \
         MCNF fleet-rendered orchestration stack (QUASAR-CLOUD QC-19, lock Q61).\n  \
         Rendered by the mackesd openstack worker from the one-state fleet doctrine\n  \
         (etcd + TOML-on-Syncthing, Q30) — the fleet is authoritative and this\n  \
         template is DERIVED from it, never hand-authored. It provisions nothing\n  \
         (an inert fleet inventory); `openstack stack list` reflects the declared\n  \
         fleet. Kolla release: {release}.\n"
    );
    let _ = write!(
        body,
        "parameters:\n  \
         kolla_release:\n    type: string\n    default: {release}\n    \
         description: the pinned Kolla release the fleet converges on (Q69).\n"
    );
    // No resources — an inventory stack never fabricates infrastructure the
    // doctrine didn't declare (§7).
    body.push_str("resources: {}\n");
    body.push_str("outputs:\n");
    body.push_str(
        "  fleet_nodes:\n    \
         description: >\n      \
         node scoping is authoritative in the doctrine record\n      \
         (cloud/doctrine.toml `nodes`); empty there ⇒ every enrolled node (Q71).\n    \
         value: from-doctrine\n",
    );
    body.push_str(
        "  fleet_services:\n    \
         description: the catalogued OpenStack services the fleet converges (Q22/24/25).\n    \
         value:\n",
    );
    if services.is_empty() {
        // Honest: an enabled-but-empty desired set (never a fabricated list).
        body.push_str("      []\n");
    } else {
        for svc in services {
            let _ = writeln!(body, "      - {svc}");
        }
    }
    body.push_str(
        "  kolla_release:\n    \
         description: provenance — the release this stack was rendered against.\n    \
         value: {get_param: kolla_release}\n",
    );

    let path = config_root.join("bootstrap").join(FLEET_HEAT_STACK_FILE);
    write_atomic(&path, &body).map_err(|source| RenderError::Io {
        service: "fleet-heat-stack".to_string(),
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::openstack::podman::config_rendered;

    /// A fixture overlay IP the render binds to (QC-6) — a node "on the mesh".
    const OVERLAY: &str = "10.42.0.9";

    fn ctx(leader: bool) -> RenderCtx {
        RenderCtx {
            release: "2024.1".into(),
            leader,
            overlay: OverlayBind::Resolved(OVERLAY.into()),
            secrets: SecretView::Sealed(Secrets::generate()),
        }
    }

    /// A ctx carrying a specific sealed set (so a test can assert the exact
    /// password the renderer substitutes).
    fn ctx_with(leader: bool, secrets: Secrets) -> RenderCtx {
        RenderCtx {
            release: "2024.1".into(),
            leader,
            overlay: OverlayBind::Resolved(OVERLAY.into()),
            secrets: SecretView::Sealed(secrets),
        }
    }

    #[test]
    fn render_writes_config_json_and_the_referenced_conf() {
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::Keystone, &ctx(false)).unwrap();
        // The gate marker exists (the reconcile start gate now flips true).
        assert!(config_rendered(dir.path(), ServiceKind::Keystone));
        let svc_dir = dir.path().join("keystone");
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(svc_dir.join("config.json")).unwrap())
                .unwrap();
        // config.json is a valid Kolla descriptor pointing at the mounted conf.
        assert_eq!(cfg["command"], "/usr/sbin/httpd -DFOREGROUND");
        assert_eq!(
            cfg["config_files"][0]["source"],
            "/var/lib/kolla/config_files/keystone.conf"
        );
        assert_eq!(
            cfg["config_files"][0]["dest"],
            "/etc/keystone/keystone.conf"
        );
        // The referenced conf actually rendered.
        assert!(svc_dir.join("keystone.conf").is_file());
        // No tmp file left behind (atomic).
        assert!(!svc_dir.join("config.json.tmp").exists());
    }

    #[test]
    fn render_is_parameterized_by_the_doctrine() {
        let dir = tempfile::tempdir().unwrap();
        // Non-leader: the API binds to the resolved overlay IP; the DB target
        // is the mesh-DNS name.
        render_service_config(dir.path(), ServiceKind::GlanceApi, &ctx(false)).unwrap();
        let body =
            std::fs::read_to_string(dir.path().join("glance_api").join("glance-api.conf")).unwrap();
        assert!(body.contains(&format!("bind_host = {OVERLAY}")), "{body}");
        assert!(body.contains("@mariadb.mesh/glance"), "{body}");
        // The pinned release is stamped in (release-parameterized render).
        assert!(body.contains("kolla release 2024.1"), "{body}");

        // Leader: its own services reach the local MariaDB on the overlay IP (Q15).
        let dir2 = tempfile::tempdir().unwrap();
        render_service_config(dir2.path(), ServiceKind::GlanceApi, &ctx(true)).unwrap();
        let body2 = std::fs::read_to_string(dir2.path().join("glance_api").join("glance-api.conf"))
            .unwrap();
        assert!(body2.contains(&format!("@{OVERLAY}/glance")), "{body2}");
    }

    #[test]
    fn every_service_renders_a_valid_config_json() {
        // The whole MVP catalog renders — config.json is valid JSON for each,
        // and each referenced source file exists on disk.
        let dir = tempfile::tempdir().unwrap();
        for kind in ServiceKind::ALL {
            render_service_config(dir.path(), kind, &ctx(true)).unwrap();
            assert!(config_rendered(dir.path(), kind), "{kind:?}");
            let svc_dir = dir.path().join(kind.container_name());
            let cfg: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(svc_dir.join("config.json")).unwrap(),
            )
            .unwrap();
            assert!(cfg["command"].as_str().is_some_and(|c| !c.is_empty()));
            for entry in cfg["config_files"].as_array().unwrap() {
                let src = entry["source"].as_str().unwrap();
                let name = src.rsplit('/').next().unwrap();
                assert!(
                    svc_dir.join(name).is_file(),
                    "{kind:?}: {name} referenced but not written"
                );
            }
        }
    }

    #[test]
    fn memcached_is_command_only() {
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::Memcached, &ctx(false)).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("memcached").join("config.json")).unwrap(),
        )
        .unwrap();
        assert!(cfg["command"].as_str().unwrap().contains("memcached"));
        assert!(cfg["config_files"].as_array().unwrap().is_empty());
    }

    #[test]
    fn render_failure_leaves_no_partial_config() {
        let dir = tempfile::tempdir().unwrap();
        // A plain file where keystone's config dir must be created → the atomic
        // render can't mkdir the service dir → a typed Io error, and the gate
        // marker is never written (never a half-written config).
        std::fs::write(dir.path().join("keystone"), b"blocker").unwrap();
        let err = render_service_config(dir.path(), ServiceKind::Keystone, &ctx(false))
            .expect_err("render must fail against a blocked dir");
        assert!(matches!(err, RenderError::Io { .. }), "{err:?}");
        assert!(err.to_string().contains("keystone"), "{err}");
        assert!(!config_rendered(dir.path(), ServiceKind::Keystone));
    }

    // ── QC-5: the sealed secrets are substituted, never the placeholder ──

    /// Read a rendered service config body off the config root.
    fn read_conf(root: &Path, kind: ServiceKind, name: &str) -> String {
        std::fs::read_to_string(root.join(kind.container_name()).join(name)).unwrap()
    }

    #[test]
    fn sealed_secrets_are_substituted_into_the_connection_strings() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Secrets::generate();
        let nova_db_pw = secrets.db_password("nova").to_string();
        let nova_svc_pw = secrets.service_user_password("nova").to_string();
        let placement_pw = secrets.service_user_password("placement").to_string();
        let rabbit_pw = secrets.rabbitmq_password().to_string();
        let context = ctx_with(false, secrets);

        render_service_config(dir.path(), ServiceKind::NovaApi, &context).unwrap();
        let body = read_conf(dir.path(), ServiceKind::NovaApi, "nova.conf");

        // The real sealed secrets land in the DB URL, the authtoken block, the
        // placement auth, and the RPC transport — each in place of the QC-4
        // placeholder.
        assert!(
            body.contains(&format!("nova:{nova_db_pw}@")),
            "db password: {body}"
        );
        assert!(
            body.contains(&format!("password = {nova_svc_pw}")),
            "authtoken: {body}"
        );
        assert!(
            body.contains(&format!("username = placement\npassword = {placement_pw}")),
            "placement auth: {body}"
        );
        assert!(
            body.contains(&format!("rabbit://openstack:{rabbit_pw}@")),
            "rpc: {body}"
        );
        // The QC-4 placeholder token is gone from the rendered config entirely.
        assert!(
            !body.contains("__mcnf_qc5_secret__"),
            "placeholder leaked: {body}"
        );
        assert!(!body.contains("SECRET_PLACEHOLDER"), "{body}");
    }

    #[test]
    fn the_same_sealed_set_renders_the_same_password_on_every_node() {
        // §7 — a non-leader reads the leader's set; the same input renders the
        // same password (leader/non-leader differ only in the DB *host*).
        let secrets = Secrets::generate();
        let expected = secrets.db_password("glance").to_string();

        let d1 = tempfile::tempdir().unwrap();
        render_service_config(
            d1.path(),
            ServiceKind::GlanceApi,
            &ctx_with(false, secrets.clone()),
        )
        .unwrap();
        let leader_view = read_conf(d1.path(), ServiceKind::GlanceApi, "glance-api.conf");

        let d2 = tempfile::tempdir().unwrap();
        render_service_config(d2.path(), ServiceKind::GlanceApi, &ctx_with(true, secrets)).unwrap();
        let non_leader_view = read_conf(d2.path(), ServiceKind::GlanceApi, "glance-api.conf");

        assert!(
            leader_view.contains(&format!("glance:{expected}@")),
            "{leader_view}"
        );
        assert!(
            non_leader_view.contains(&format!("glance:{expected}@")),
            "{non_leader_view}"
        );
    }

    #[test]
    fn an_unsealed_ctx_gates_the_render() {
        let dir = tempfile::tempdir().unwrap();
        let context = RenderCtx {
            release: "2024.1".into(),
            leader: false,
            // Overlay resolved so the render reaches (and gates on) the secrets.
            overlay: OverlayBind::Resolved(OVERLAY.into()),
            secrets: SecretView::Unsealed("awaiting sealed secrets from leader".to_string()),
        };
        let err = render_service_config(dir.path(), ServiceKind::Keystone, &context)
            .expect_err("an unsealed ctx must gate the render");
        let RenderError::SecretsUnsealed { reason } = &err else {
            unreachable!("wrong variant: {err:?}");
        };
        assert!(
            reason.contains("awaiting sealed secrets from leader"),
            "{reason}"
        );
        // Nothing was written — no blank-credential config left behind.
        assert!(!config_rendered(dir.path(), ServiceKind::Keystone));
    }

    #[test]
    fn an_incomplete_sealed_set_gates_rather_than_rendering_a_blank() {
        let dir = tempfile::tempdir().unwrap();
        // A sealed set that dropped nova's DB password → the completeness gate
        // fires; the renderer never substitutes a blank password.
        let secrets = Secrets::generate().dropping_for_test("db_nova");
        let err =
            render_service_config(dir.path(), ServiceKind::NovaApi, &ctx_with(false, secrets))
                .expect_err("an incomplete set must gate");
        let RenderError::SecretsUnsealed { reason } = &err else {
            unreachable!("wrong variant: {err:?}");
        };
        assert!(reason.contains("db_nova"), "{reason}");
        assert!(!config_rendered(dir.path(), ServiceKind::NovaApi));
    }

    // ── QC-6: the Nebula-overlay bind + mesh endpoint URLs ──

    #[test]
    fn every_bind_directive_is_the_resolved_overlay_never_zero_or_localhost() {
        // §7 — each service that controls its own listener binds to the
        // resolved overlay IP; never 0.0.0.0/localhost (which would expose a
        // control-plane API on the public underlay, design Q23).
        let dir = tempfile::tempdir().unwrap();
        let cases: &[(ServiceKind, &str, &str)] = &[
            (ServiceKind::Mariadb, "galera.cnf", "bind_address"),
            (
                ServiceKind::Rabbitmq,
                "rabbitmq.conf",
                "listeners.tcp.default",
            ),
            (ServiceKind::GlanceApi, "glance-api.conf", "bind_host"),
            (ServiceKind::NovaApi, "nova.conf", "osapi_compute_listen"),
            (ServiceKind::NeutronServer, "neutron.conf", "bind_host"),
            (ServiceKind::CinderApi, "cinder.conf", "osapi_volume_listen"),
        ];
        for (kind, file, directive) in cases {
            render_service_config(dir.path(), *kind, &ctx(false)).unwrap();
            let body = read_conf(dir.path(), *kind, file);
            // The bind directive names the overlay IP.
            assert!(
                body.contains(&format!("{directive} = {OVERLAY}")),
                "{kind:?} {directive} must bind the overlay: {body}"
            );
            // And nothing binds to a wildcard/loopback address.
            assert!(!body.contains("0.0.0.0"), "{kind:?} binds 0.0.0.0: {body}");
            assert!(
                !body.contains("127.0.0.1"),
                "{kind:?} binds loopback: {body}"
            );
        }
        // memcached is command-only — its listener flag is the overlay too.
        render_service_config(dir.path(), ServiceKind::Memcached, &ctx(false)).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("memcached").join("config.json")).unwrap(),
        )
        .unwrap();
        let cmd = cfg["command"].as_str().unwrap();
        assert!(cmd.contains(&format!("-l {OVERLAY}")), "{cmd}");
        assert!(!cmd.contains("0.0.0.0"), "{cmd}");
    }

    #[test]
    fn api_endpoint_urls_advertise_the_mesh_address() {
        // The service-catalog endpoint URLs carry the Nebula-DNS mesh address
        // (Q22), so tenants reach every API over the overlay.
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::Keystone, &ctx(false)).unwrap();
        let keystone = read_conf(dir.path(), ServiceKind::Keystone, "keystone.conf");
        assert!(
            keystone.contains("public_endpoint = http://keystone.mesh:5000"),
            "{keystone}"
        );
        assert!(
            keystone.contains("admin_endpoint = http://keystone.mesh:5000"),
            "{keystone}"
        );

        render_service_config(dir.path(), ServiceKind::GlanceApi, &ctx(false)).unwrap();
        let glance = read_conf(dir.path(), ServiceKind::GlanceApi, "glance-api.conf");
        assert!(
            glance.contains("public_endpoint = http://glance.mesh:9292"),
            "{glance}"
        );

        // Nova reaches Glance's image API over the mesh endpoint too.
        render_service_config(dir.path(), ServiceKind::NovaApi, &ctx(false)).unwrap();
        let nova = read_conf(dir.path(), ServiceKind::NovaApi, "nova.conf");
        assert!(
            nova.contains("api_servers = http://glance.mesh:9292"),
            "{nova}"
        );
        // Keystone auth is a mesh endpoint on every authenticated API.
        assert!(
            nova.contains("auth_url = http://keystone.mesh:5000"),
            "{nova}"
        );
    }

    #[test]
    fn an_unresolved_overlay_gates_the_render() {
        // §7 / Q23 — a node not on the mesh has no overlay IP; the render gates
        // the service with the sharp reason rather than bind 0.0.0.0. Sealed
        // secrets prove the overlay gate fires FIRST (independent of secrets).
        let dir = tempfile::tempdir().unwrap();
        let context = RenderCtx {
            release: "2024.1".into(),
            leader: false,
            overlay: OverlayBind::Unresolved(
                "overlay address unresolved — node not on the mesh".to_string(),
            ),
            secrets: SecretView::Sealed(Secrets::generate()),
        };
        let err = render_service_config(dir.path(), ServiceKind::Keystone, &context)
            .expect_err("an unresolved overlay must gate the render");
        let RenderError::OverlayUnresolved { reason } = &err else {
            unreachable!("wrong variant: {err:?}");
        };
        assert!(reason.contains("overlay address unresolved"), "{reason}");
        assert!(reason.contains("not on the mesh"), "{reason}");
        // Nothing was written — no 0.0.0.0-bound config left behind.
        assert!(!config_rendered(dir.path(), ServiceKind::Keystone));
    }

    // ── QC-7: the Neutron ML2/OVN flat mesh network ──

    /// Read a command-only service's launch command out of its `config.json`
    /// (the OVN OVSDB daemons + northd carry their whole config on the argv).
    fn read_command(root: &Path, kind: ServiceKind) -> String {
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(kind.container_name()).join("config.json")).unwrap(),
        )
        .unwrap();
        cfg["command"].as_str().unwrap().to_string()
    }

    #[test]
    fn neutron_is_flat_over_mesh_not_a_tenant_overlay() {
        // Q42/43/44 — the ML2 config is OVN + ONE flat provider net; there is NO
        // tenant overlay (empty tenant_network_types, flat type_driver only), so
        // an instance is a mesh peer-equivalent on the flat net, not a geneve
        // tenant-network guest.
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::NeutronServer, &ctx(false)).unwrap();
        let ml2 = read_conf(dir.path(), ServiceKind::NeutronServer, "ml2_conf.ini");
        assert!(ml2.contains("mechanism_drivers = ovn"), "{ml2}");
        assert!(ml2.contains("type_drivers = flat"), "{ml2}");
        // The flat-over-mesh signal: no tenant overlay networks.
        assert!(ml2.contains("tenant_network_types =\n"), "{ml2}");
        assert!(
            !ml2.contains("tenant_network_types = geneve"),
            "must NOT be a geneve tenant overlay: {ml2}"
        );
        assert!(!ml2.contains("type_drivers = flat,geneve"), "{ml2}");
        // The single flat provider net + its provider bridge mapping into the
        // mesh.
        assert!(ml2.contains("flat_networks = mesh"), "{ml2}");
        assert!(ml2.contains("bridge_mappings = mesh:br-mesh"), "{ml2}");
    }

    #[test]
    fn neutron_ovn_section_binds_the_leader_hosted_dbs() {
        // QC-7/Q15 — the [ovn] section points Neutron's ML2/OVN driver at the
        // leader-hosted NB/SB OVSDBs: over mesh-DNS from a non-leader, on the
        // local overlay IP on the leader (a failover moves the name, not config).
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::NeutronServer, &ctx(false)).unwrap();
        let ml2 = read_conf(dir.path(), ServiceKind::NeutronServer, "ml2_conf.ini");
        assert!(
            ml2.contains("ovn_nb_connection = tcp:ovn-nb.mesh:6641"),
            "{ml2}"
        );
        assert!(
            ml2.contains("ovn_sb_connection = tcp:ovn-sb.mesh:6642"),
            "{ml2}"
        );

        let dir2 = tempfile::tempdir().unwrap();
        render_service_config(dir2.path(), ServiceKind::NeutronServer, &ctx(true)).unwrap();
        let ml2_leader = read_conf(dir2.path(), ServiceKind::NeutronServer, "ml2_conf.ini");
        assert!(
            ml2_leader.contains(&format!("ovn_nb_connection = tcp:{OVERLAY}:6641")),
            "{ml2_leader}"
        );
        assert!(
            ml2_leader.contains(&format!("ovn_sb_connection = tcp:{OVERLAY}:6642")),
            "{ml2_leader}"
        );
    }

    #[test]
    fn neutron_sets_the_geneve_over_nebula_mtu() {
        // Q49 — the tenant/instance MTU is set for Geneve-over-Nebula double
        // encap (OVN tunnels flat-net east-west over geneve on the overlay).
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::NeutronServer, &ctx(false)).unwrap();
        let neutron = read_conf(dir.path(), ServiceKind::NeutronServer, "neutron.conf");
        assert!(neutron.contains("global_physnet_mtu = 1342"), "{neutron}");
        // Pure-L2 flat net — no L3/router service plugin in the path.
        assert!(neutron.contains("service_plugins =\n"), "{neutron}");
        // The API still binds the overlay only (QC-6 preserved).
        assert!(
            neutron.contains(&format!("bind_host = {OVERLAY}")),
            "{neutron}"
        );
    }

    #[test]
    fn ovn_dbs_bind_the_overlay_and_northd_wires_them() {
        // QC-7 — the leader-hosted OVN OVSDBs bind their listeners to the overlay
        // IP (Q23; plaintext, the overlay is the security), and northd (leader-
        // local) wires both. Never 0.0.0.0/localhost.
        let dir = tempfile::tempdir().unwrap();
        let leader = ctx(true);
        for (kind, addr, port) in [
            (ServiceKind::OvnNbDb, "--db-nb-addr", 6641),
            (ServiceKind::OvnSbDb, "--db-sb-addr", 6642),
        ] {
            render_service_config(dir.path(), kind, &leader).unwrap();
            let cmd = read_command(dir.path(), kind);
            assert!(
                cmd.contains(&format!("{addr}={OVERLAY}")),
                "{kind:?}: {cmd}"
            );
            assert!(cmd.contains(&port.to_string()), "{kind:?}: {cmd}");
            assert!(cmd.contains("insecure-remote=yes"), "{kind:?}: {cmd}");
            assert!(!cmd.contains("0.0.0.0"), "{kind:?}: {cmd}");
            assert!(!cmd.contains("127.0.0.1"), "{kind:?}: {cmd}");
        }
        render_service_config(dir.path(), ServiceKind::OvnNorthd, &leader).unwrap();
        let northd = read_command(dir.path(), ServiceKind::OvnNorthd);
        // northd runs only on the leader, where both DBs are local.
        assert!(
            northd.contains(&format!("--ovn-nb-db=tcp:{OVERLAY}:6641")),
            "{northd}"
        );
        assert!(
            northd.contains(&format!("--ovn-sb-db=tcp:{OVERLAY}:6642")),
            "{northd}"
        );
    }

    #[test]
    fn ovn_controller_maps_the_flat_bridge_on_every_chassis() {
        // QC-7/Q43/49 — the per-chassis controller programs the host OVS: the SB
        // remote (leader-hosted, reached over mesh-DNS off the leader), geneve
        // inter-chassis encap on THIS node's overlay IP, and the mesh:br-mesh
        // provider bridge mapping that puts an instance on the mesh.
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::OvnController, &ctx(false)).unwrap();
        let chassis = read_conf(
            dir.path(),
            ServiceKind::OvnController,
            "ovn-controller.conf",
        );
        assert!(
            chassis.contains("external_ids:ovn-remote = tcp:ovn-sb.mesh:6642"),
            "{chassis}"
        );
        assert!(
            chassis.contains("external_ids:ovn-encap-type = geneve"),
            "{chassis}"
        );
        assert!(
            chassis.contains(&format!("external_ids:ovn-encap-ip = {OVERLAY}")),
            "{chassis}"
        );
        assert!(
            chassis.contains("external_ids:ovn-bridge-mappings = mesh:br-mesh"),
            "{chassis}"
        );
        // On the leader the SB DB is local (overlay IP).
        let dir2 = tempfile::tempdir().unwrap();
        render_service_config(dir2.path(), ServiceKind::OvnController, &ctx(true)).unwrap();
        let chassis_leader = read_conf(
            dir2.path(),
            ServiceKind::OvnController,
            "ovn-controller.conf",
        );
        assert!(
            chassis_leader.contains(&format!("external_ids:ovn-remote = tcp:{OVERLAY}:6642")),
            "{chassis_leader}"
        );
    }

    // ── QC-8: the Cinder LVM backend + cinder-backup to the object tier ──

    #[test]
    fn cinder_renders_the_node_local_lvm_backend() {
        // Q51/59 — the [lvm] backend: the volume group carved on the writable
        // partition, thin-provisioned for snapshots (Q56), served over an
        // iSCSI/LIO target whose portal binds to THIS node's overlay IP (QC-6,
        // Q23 — a peer attaches the volume over the mesh, never the underlay).
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::CinderVolume, &ctx(false)).unwrap();
        let conf = read_conf(dir.path(), ServiceKind::CinderVolume, "cinder.conf");
        assert!(conf.contains("enabled_backends = lvm"), "{conf}");
        assert!(
            conf.contains("volume_driver = cinder.volume.drivers.lvm.LVMVolumeDriver"),
            "{conf}"
        );
        assert!(conf.contains("volume_group = cinder-volumes"), "{conf}");
        assert!(conf.contains("lvm_type = thin"), "{conf}");
        assert!(conf.contains("target_protocol = iscsi"), "{conf}");
        assert!(conf.contains("target_helper = lioadm"), "{conf}");
        // The iSCSI portal is the overlay IP — a volume is attachable over the
        // mesh only, never 0.0.0.0/the public underlay.
        assert!(
            conf.contains(&format!("target_ip_address = {OVERLAY}")),
            "{conf}"
        );
        assert!(!conf.contains("0.0.0.0"), "{conf}");
        assert!(!conf.contains("127.0.0.1"), "{conf}");
    }

    #[test]
    fn cinder_renders_backup_to_the_swift_object_tier() {
        // Q55/57 — cinder-backup streams volumes to the Keystone-native Swift hot
        // object tier, reached over the mesh by its Nebula-DNS name (Swift
        // replicates off-site to DO Spaces per Q54 — the off-site leg rides
        // Swift's own replication, not a second cinder driver).
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::CinderVolume, &ctx(false)).unwrap();
        let conf = read_conf(dir.path(), ServiceKind::CinderVolume, "cinder.conf");
        assert!(
            conf.contains("backup_driver = cinder.backup.drivers.swift.SwiftBackupDriver"),
            "{conf}"
        );
        assert!(
            conf.contains("backup_swift_url = http://swift.mesh:8080/v1/AUTH_"),
            "{conf}"
        );
        assert!(
            conf.contains("backup_swift_container = volumebackups"),
            "{conf}"
        );
        // Keystone-native per-user auth (the mesh account IS the cloud account,
        // Q21) — never a hardcoded service credential in the rendered config.
        assert!(conf.contains("backup_swift_auth = per_user"), "{conf}");
    }

    #[test]
    fn cinder_backup_service_renders_the_shared_cinder_conf() {
        // QC-8 — the new cinder-backup agent reads the same one cinder.conf as
        // the api/scheduler/volume services (LVM backend + backup config in one
        // file); its config.json launches the backup command.
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::CinderBackup, &ctx(false)).unwrap();
        assert!(config_rendered(dir.path(), ServiceKind::CinderBackup));
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("cinder_backup").join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["command"], "cinder-backup");
        assert_eq!(cfg["config_files"][0]["dest"], "/etc/cinder/cinder.conf");
        // The same complete backend + backup config lands for the backup agent.
        let conf = read_conf(dir.path(), ServiceKind::CinderBackup, "cinder.conf");
        assert!(conf.contains("volume_group = cinder-volumes"), "{conf}");
        assert!(
            conf.contains("backup_driver = cinder.backup.drivers.swift.SwiftBackupDriver"),
            "{conf}"
        );
    }

    // ── QC-9: the Glance local-file store + replication/caching (Q36/53) ──

    #[test]
    fn glance_renders_the_node_local_file_store() {
        // Q53/Q59 — the [glance_store] file store on THIS node's writable
        // partition: `stores`/`default_store` are `file`, the datadir is the
        // canonical images path, and rendered files are group-readable only.
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::GlanceApi, &ctx(false)).unwrap();
        let conf = read_conf(dir.path(), ServiceKind::GlanceApi, "glance-api.conf");
        assert!(conf.contains("[glance_store]"), "{conf}");
        assert!(conf.contains("stores = file"), "{conf}");
        assert!(conf.contains("default_store = file"), "{conf}");
        assert!(
            conf.contains("filesystem_store_datadir = /var/lib/glance/images/"),
            "{conf}"
        );
        assert!(conf.contains("filesystem_store_file_perm = 0640"), "{conf}");
    }

    #[test]
    fn glance_renders_the_image_cache_with_a_real_bound_and_cachemanagement() {
        // Q53 — the "caching between API nodes" half: a cache dir on the writable
        // partition, a REAL byte ceiling (20 GiB) the pruner trims to (never an
        // unbounded cache, §7), the stall time, and the paste flavor that
        // actually wires the cache middleware into the API pipeline (without it
        // the cache dir is inert — a config that looks cached but isn't).
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::GlanceApi, &ctx(false)).unwrap();
        let conf = read_conf(dir.path(), ServiceKind::GlanceApi, "glance-api.conf");
        assert!(
            conf.contains("image_cache_dir = /var/lib/glance/image-cache/"),
            "{conf}"
        );
        // 20 GiB, spelled in bytes so the pruner has a concrete ceiling.
        assert!(
            conf.contains(&format!(
                "image_cache_max_size = {}",
                20u64 * 1024 * 1024 * 1024
            )),
            "{conf}"
        );
        assert!(conf.contains("image_cache_stall_time = 86400"), "{conf}");
        assert!(
            conf.contains("flavor = keystone+cachemanagement"),
            "the cache middleware must be wired into the paste pipeline: {conf}"
        );
    }

    #[test]
    fn glance_renders_the_cross_node_replication_wiring() {
        // Q53 — the "replication between API nodes" half: the node advertises its
        // own mesh endpoint as `worker_self_reference_url` (the identity a peer's
        // copy-image import resolves a store location against), and `copy-image`
        // is an enabled import method (the verb that pulls an image into THIS
        // node's local store from a peer). Both reference the mesh endpoint, never
        // a public-underlay address (QC-6/Q23).
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::GlanceApi, &ctx(false)).unwrap();
        let conf = read_conf(dir.path(), ServiceKind::GlanceApi, "glance-api.conf");
        assert!(
            conf.contains("worker_self_reference_url = http://glance.mesh:9292"),
            "{conf}"
        );
        assert!(
            conf.contains("enabled_import_methods = [glance-direct,web-download,copy-image]"),
            "copy-image is the replication verb: {conf}"
        );
        // Still overlay-only — no wildcard/loopback leaked by the QC-9 additions.
        assert!(!conf.contains("0.0.0.0"), "{conf}");
        assert!(!conf.contains("127.0.0.1"), "{conf}");
    }

    // ── QC-10: capacity-derived flavors + hard per-user quotas (Q29/39/89) ──

    /// A big node and a small node — the seed's scaling is asserted across both.
    const BIG_NODE: NodeCapacity = NodeCapacity::new(32, 131_072, 2_000);
    const SMALL_NODE: NodeCapacity = NodeCapacity::new(4, 8_192, 100);

    /// Read the rendered bootstrap seed body off the config root.
    fn read_seed(root: &Path) -> String {
        std::fs::read_to_string(root.join("bootstrap").join("cloud-bootstrap.sh")).unwrap()
    }

    #[test]
    fn nova_selects_the_unified_limits_quota_driver() {
        // QC-10/Q89 — nova.conf carries the [quota] UnifiedLimitsDriver so the
        // Keystone unified limits the seed registers are actually ENFORCED (a
        // hard boundary, not a declared-but-inert number).
        let dir = tempfile::tempdir().unwrap();
        render_service_config(dir.path(), ServiceKind::NovaApi, &ctx(false)).unwrap();
        let nova = read_conf(dir.path(), ServiceKind::NovaApi, "nova.conf");
        assert!(nova.contains("[quota]"), "{nova}");
        assert!(
            nova.contains("driver = nova.quota.UnifiedLimitsDriver"),
            "{nova}"
        );
    }

    #[test]
    fn bootstrap_seed_renders_capacity_derived_flavors() {
        // Q39 — the seed creates a tiny/small/medium/large ladder sized from the
        // node's real shape (the largest flavor is half a 32-vCPU node), via
        // idempotent `openstack flavor create` calls.
        let dir = tempfile::tempdir().unwrap();
        let path = render_cloud_bootstrap(dir.path(), "2024.1", &BIG_NODE).unwrap();
        assert!(path.ends_with("bootstrap/cloud-bootstrap.sh"));
        let seed = read_seed(dir.path());
        // Provenance + the honest capacity comment.
        assert!(seed.contains("QC-10"), "{seed}");
        assert!(seed.contains("kolla release 2024.1"), "{seed}");
        assert!(seed.contains("32 vCPU / 131072 MiB / 2000 GiB"), "{seed}");
        // All four rungs, capacity-derived — on a 32-vCPU node the fractions
        // dominate the floors: tiny = vcpus/16, ram/16, disk/32; large = vcpus/2,
        // ram/2, disk/4.
        assert!(seed.contains("ensure_flavor m1.tiny 2 8192 62"), "{seed}");
        assert!(
            seed.contains("ensure_flavor m1.large 16 65536 500"),
            "{seed}"
        );
        // Idempotent create guard.
        assert!(seed.contains("openstack flavor show"), "{seed}");
        assert!(seed.contains("openstack flavor create --vcpus"), "{seed}");
    }

    #[test]
    fn bootstrap_flavors_scale_with_capacity() {
        // §7 — the flavor set regenerates larger for a bigger node.
        let big_dir = tempfile::tempdir().unwrap();
        render_cloud_bootstrap(big_dir.path(), "2024.1", &BIG_NODE).unwrap();
        let small_dir = tempfile::tempdir().unwrap();
        render_cloud_bootstrap(small_dir.path(), "2024.1", &SMALL_NODE).unwrap();
        // The big node's large flavor (16 vCPU) is bigger than the small node's
        // (2→floored 4 vCPU) — the same rung, a different size.
        assert!(read_seed(big_dir.path()).contains("m1.large 16 65536 500"));
        assert!(!read_seed(small_dir.path()).contains("m1.large 16 65536 500"));
        assert!(read_seed(small_dir.path()).contains("ensure_flavor m1.large 4 4096 "));
    }

    #[test]
    fn bootstrap_seed_renders_hard_per_user_quota_caps() {
        // Q89 — the seed registers the five hard per-user caps the design names
        // (instances/vCPU/RAM/volumes/floating-IPs, + the gigabytes ceiling) as
        // Keystone unified limits, each a fraction of the node (a quarter here).
        let dir = tempfile::tempdir().unwrap();
        render_cloud_bootstrap(dir.path(), "2024.1", &BIG_NODE).unwrap();
        let seed = read_seed(dir.path());
        assert!(seed.contains("Hard per-user quotas (Q89"), "{seed}");
        // The config literally contains the caps (design §7), each derived.
        assert!(seed.contains("ensure_limit nova servers 8"), "{seed}");
        assert!(seed.contains("ensure_limit nova class:VCPU 8"), "{seed}");
        assert!(
            seed.contains("ensure_limit nova class:MEMORY_MB 32768"),
            "{seed}"
        );
        assert!(seed.contains("ensure_limit cinder volumes 16"), "{seed}");
        assert!(seed.contains("ensure_limit cinder gigabytes 500"), "{seed}");
        assert!(seed.contains("ensure_limit neutron floatingip 8"), "{seed}");
        // Registered as Keystone unified limits (the hard default every user
        // inherits), idempotently.
        assert!(
            seed.contains("openstack registered limit create --service"),
            "{seed}"
        );
        assert!(
            seed.contains("openstack registered limit list --service"),
            "{seed}"
        );
    }

    #[test]
    fn bootstrap_quota_caps_are_a_hard_fraction_that_scales() {
        // §7 — the per-user cap is strictly a fraction of the node (never the
        // whole node) and grows with capacity: the big node's vCPU cap (8) beats
        // the small node's (1).
        let big_dir = tempfile::tempdir().unwrap();
        render_cloud_bootstrap(big_dir.path(), "2024.1", &BIG_NODE).unwrap();
        let small_dir = tempfile::tempdir().unwrap();
        render_cloud_bootstrap(small_dir.path(), "2024.1", &SMALL_NODE).unwrap();
        // 8 vCPU per user on a 32-vCPU node → a quarter, so ≥4 members coexist;
        // 1 vCPU on the 4-vCPU node — the cap tracks capacity, always a fraction.
        assert!(read_seed(big_dir.path()).contains("ensure_limit nova class:VCPU 8"));
        assert!(read_seed(small_dir.path()).contains("ensure_limit nova class:VCPU 1"));
    }

    // ─────────────────── QC-19: wave-2 services (Q25/47/61) ───────────────────

    #[test]
    fn heat_shares_one_conf_and_binds_both_apis_to_the_overlay() {
        // Q61 — the three Heat services read one heat.conf; only the command
        // differs. Both API endpoints bind to the overlay (QC-6/Q23), and the DB +
        // authtoken are the shared sealed seams.
        let dir = tempfile::tempdir().unwrap();
        for (kind, command) in [
            (ServiceKind::HeatApi, "heat-api"),
            (ServiceKind::HeatApiCfn, "heat-api-cfn"),
            (ServiceKind::HeatEngine, "heat-engine"),
        ] {
            render_service_config(dir.path(), kind, &ctx(false)).unwrap();
            assert_eq!(read_command(dir.path(), kind), command);
            let conf = read_conf(dir.path(), kind, "heat.conf");
            assert!(conf.contains(&format!("bind_host = {OVERLAY}")), "{conf}");
            assert!(conf.contains("bind_port = 8004"), "{conf}"); // heat_api
            assert!(conf.contains("bind_port = 8000"), "{conf}"); // heat_api_cfn
            assert!(conf.contains("@mariadb.mesh/heat"), "{conf}");
            assert!(conf.contains("[keystone_authtoken]"), "{conf}");
            assert!(conf.contains("[trustee]"), "{conf}");
            // Never falls back off the overlay (Q23).
            assert!(!conf.contains("0.0.0.0"), "{conf}");
        }
    }

    #[test]
    fn octavia_wires_keystone_but_leaves_amphora_unfabricated() {
        // Q47 — the four Octavia services read one octavia.conf (Keystone + DB +
        // RPC + overlay-bound API/health-manager). §7 — the amphora image /
        // management-network / cert IDs are NOT fabricated: an honest precondition
        // documented in the conf, not a placeholder UUID.
        let dir = tempfile::tempdir().unwrap();
        for (kind, command) in [
            (ServiceKind::OctaviaApi, "octavia-api"),
            (ServiceKind::OctaviaWorker, "octavia-worker"),
            (ServiceKind::OctaviaHealthManager, "octavia-health-manager"),
            (ServiceKind::OctaviaHousekeeping, "octavia-housekeeping"),
        ] {
            render_service_config(dir.path(), kind, &ctx(false)).unwrap();
            assert_eq!(read_command(dir.path(), kind), command);
            let conf = read_conf(dir.path(), kind, "octavia.conf");
            assert!(conf.contains(&format!("bind_host = {OVERLAY}")), "{conf}");
            assert!(conf.contains("bind_port = 9876"), "{conf}");
            assert!(conf.contains(&format!("bind_ip = {OVERLAY}")), "{conf}"); // health-manager
            assert!(conf.contains("@mariadb.mesh/octavia"), "{conf}");
            assert!(conf.contains("[service_auth]"), "{conf}");
            // The honest precondition, never a fabricated amphora UUID (§7).
            assert!(conf.contains("provisioned"), "{conf}");
            assert!(
                !conf.to_lowercase().contains("amp_image_owner_id ="),
                "{conf}"
            );
        }
    }

    #[test]
    fn horizon_binds_the_overlay_keystone_and_the_sealed_session_key() {
        // Q25/26 — the OPTIONAL dashboard: Django local_settings bound to the
        // overlay only (Q23), Keystone-backed (Q21), with the real sealed
        // SECRET_KEY (QC-5) — never blank.
        let dir = tempfile::tempdir().unwrap();
        let secrets = Secrets::generate();
        let session_key = secrets.horizon_secret_key().to_string();
        render_service_config(dir.path(), ServiceKind::Horizon, &ctx_with(false, secrets)).unwrap();
        assert_eq!(
            read_command(dir.path(), ServiceKind::Horizon),
            "/usr/sbin/httpd -DFOREGROUND"
        );
        let conf = read_conf(dir.path(), ServiceKind::Horizon, "local_settings");
        assert!(
            conf.contains(&format!("ALLOWED_HOSTS = ['{OVERLAY}'")),
            "{conf}"
        );
        assert!(conf.contains("OPENSTACK_KEYSTONE_URL"), "{conf}");
        assert!(
            conf.contains(&format!("SECRET_KEY = '{session_key}'")),
            "{conf}"
        );
        assert!(!session_key.is_empty(), "the sealed key is real");
    }

    #[test]
    fn fleet_heat_stack_is_derived_from_the_service_set_no_fabrication() {
        // Q61 — the worker renders the stack from real fleet state (the desired
        // service set), inert (provisions nothing), no fabricated infrastructure.
        let dir = tempfile::tempdir().unwrap();
        let services = vec![
            "keystone".to_string(),
            "nova_api".to_string(),
            "heat_api".to_string(),
        ];
        let path = render_fleet_heat_stack(dir.path(), "2024.1", &services).unwrap();
        assert!(path.ends_with("bootstrap/fleet-stack.yaml"));
        let hot = std::fs::read_to_string(&path).unwrap();
        // A valid HOT header + the pinned release stamped from fleet state.
        assert!(hot.contains("heat_template_version:"), "{hot}");
        assert!(hot.contains("default: 2024.1"), "{hot}");
        // Inert: it declares NO resources (never fabricates infra the doctrine
        // didn't declare, §7).
        assert!(hot.contains("resources: {}"), "{hot}");
        // Every converged service appears, verbatim from the passed set.
        for svc in &services {
            assert!(hot.contains(&format!("- {svc}")), "{svc} missing: {hot}");
        }
        // An enabled-but-empty desired set renders an honest empty list, never a
        // fabricated one.
        let empty_dir = tempfile::tempdir().unwrap();
        let empty = render_fleet_heat_stack(empty_dir.path(), "2024.1", &[]).unwrap();
        let hot_empty = std::fs::read_to_string(empty).unwrap();
        assert!(hot_empty.contains("value:\n      []"), "{hot_empty}");
    }

    #[test]
    fn bootstrap_seed_creates_the_fleet_stack_idempotently() {
        // Q61 — the leader's bootstrap seed guards a create of the fleet stack
        // (show-first), and only when the worker actually rendered the template.
        let dir = tempfile::tempdir().unwrap();
        render_cloud_bootstrap(dir.path(), "2024.1", &BIG_NODE).unwrap();
        let seed = read_seed(dir.path());
        assert!(seed.contains("openstack stack show mcnf-fleet"), "{seed}");
        assert!(
            seed.contains("openstack stack create -t \"$FLEET_STACK_TEMPLATE\" mcnf-fleet"),
            "{seed}"
        );
        // Guarded on the template's presence (skips honestly when Heat/the stack
        // isn't part of this fleet).
        assert!(
            seed.contains("if [ -f \"$FLEET_STACK_TEMPLATE\" ]"),
            "{seed}"
        );
    }
}
