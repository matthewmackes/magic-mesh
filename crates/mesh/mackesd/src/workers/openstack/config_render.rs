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
        ServiceKind::GlanceApi => one(
            "glance-api",
            "glance-api.conf",
            "/etc/glance/glance-api.conf",
            "glance:glance",
            format!(
                "[DEFAULT]\ndebug = False\nbind_host = {host}\nbind_port = {port}\n\
                 public_endpoint = {endpoint}\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [glance_store]\nstores = file\ndefault_store = file\n\
                 filesystem_store_datadir = /var/lib/glance/images/\n",
                host = overlay,
                port = ServiceKind::GlanceApi.api_port().unwrap_or_default(),
                endpoint = mesh_endpoint(ServiceKind::GlanceApi),
                db = db_url("glance", overlay, ctx, secrets),
                authtoken = authtoken("glance", overlay, secrets),
            ),
        ),
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
        ServiceKind::NeutronServer => ServicePlan {
            command: "neutron-server --config-file /etc/neutron/neutron.conf \
                      --config-file /etc/neutron/plugins/ml2/ml2_conf.ini"
                .to_string(),
            files: vec![
                ConfFile {
                    name: "neutron.conf",
                    dest: "/etc/neutron/neutron.conf",
                    owner: "neutron:neutron",
                    body: format!(
                        "[DEFAULT]\ndebug = False\nbind_host = {host}\nbind_port = {port}\n\
                         core_plugin = ml2\nservice_plugins = router\n\
                         transport_url = {rpc}\n\
                         [database]\nconnection = {db}\n{authtoken}",
                        host = overlay,
                        port = ServiceKind::NeutronServer.api_port().unwrap_or_default(),
                        rpc = transport_url(secrets),
                        db = db_url("neutron", overlay, ctx, secrets),
                        authtoken = authtoken("neutron", overlay, secrets),
                    ),
                },
                ConfFile {
                    name: "ml2_conf.ini",
                    dest: "/etc/neutron/plugins/ml2/ml2_conf.ini",
                    owner: "neutron:neutron",
                    // ML2/OVN, one flat provider net (Q42/43).
                    body: "[ml2]\ntype_drivers = flat,geneve\n\
                           tenant_network_types = geneve\nmechanism_drivers = ovn\n\
                           [ml2_type_flat]\nflat_networks = mesh\n\
                           [ml2_type_geneve]\nmax_header_size = 38\n"
                        .to_string(),
                },
            ],
        },
        ServiceKind::CinderApi => cinder("cinder-api", overlay, ctx, secrets),
        ServiceKind::CinderScheduler => cinder("cinder-scheduler", overlay, ctx, secrets),
        ServiceKind::CinderVolume => cinder("cinder-volume", overlay, ctx, secrets),
    }
}

/// The shared Nova plan (all four nova services read one `nova.conf`; only the
/// command differs — Q31 Nova+Placement own VM lifecycle). The API listener
/// binds to the overlay ([`api_default`]); the cross-service references
/// (Glance images, Placement, Keystone) are mesh-DNS endpoints reached over the
/// overlay (QC-6, Q22).
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
                 [libvirt]\nvirt_type = kvm\n",
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

/// The shared Cinder plan (LVM per node — Q51/59). The volume API listener
/// binds to the overlay (QC-6, Q22/23).
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
                 [database]\nconnection = {db}\n{authtoken}\
                 [lvm]\nvolume_driver = cinder.volume.drivers.lvm.LVMVolumeDriver\n\
                 volume_group = cinder-volumes\ntarget_protocol = iscsi\n",
                host = overlay,
                port = ServiceKind::CinderApi.api_port().unwrap_or_default(),
                rpc = transport_url(secrets),
                db = db_url("cinder", overlay, ctx, secrets),
                authtoken = authtoken("cinder", overlay, secrets),
            ),
        }],
    }
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
}
