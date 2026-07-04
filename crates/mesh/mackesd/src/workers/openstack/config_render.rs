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
//! mesh-DNS), and the node's mesh name (Q22/23 — APIs bind plaintext to the
//! overlay only; the mesh name resolves to the Nebula address).
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
//! service needs to *start*. Real service credentials are seeded by the QC-5
//! identity work; until then the connection strings carry the
//! [`SECRET_PLACEHOLDER`] token (an honest "not yet sealed", not a fake
//! secret), so the config is structurally complete and the container comes up.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use super::catalog::ServiceKind;
use super::podman::kolla_config_dir;

/// Placeholder for a service credential the QC-5 identity work seals + injects.
/// Rendered into the connection strings so the config is structurally complete
/// without fabricating a real secret (§7).
pub const SECRET_PLACEHOLDER: &str = "__mcnf_qc5_secret__";

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

/// The node-local render inputs folded from the doctrine (design Q30).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderCtx {
    /// The pinned Kolla release (Q69) — echoed into the config for provenance.
    pub release: String,
    /// This node holds the etcd leader lease → it hosts `MariaDB` (Q15) and its
    /// own services reach the DB locally; a non-leader reaches it via mesh-DNS.
    pub leader: bool,
    /// This node's mesh name — the API bind host (Q22/23) and the local cache /
    /// DB target on the leader.
    pub host: String,
}

impl RenderCtx {
    /// A context with no live doctrine (a `Disabled`/`Gated` tick converges no
    /// starts, so the release is unused — only `host` is meaningful).
    #[must_use]
    pub fn empty(host: &str) -> Self {
        Self {
            release: String::new(),
            leader: false,
            host: host.to_string(),
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
    let dir = kolla_config_dir(config_root, kind);
    let service = kind.container_name();
    let plan = service_plan(kind, ctx);

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

/// The DB connection URL for `svc`: the leader reaches its own local `MariaDB`,
/// every other node reaches it over mesh-DNS (Q15/Q46).
fn db_url(svc: &str, ctx: &RenderCtx) -> String {
    let host = if ctx.leader {
        ctx.host.as_str()
    } else {
        DB_MESH_NAME
    };
    format!("mysql+pymysql://{svc}:{SECRET_PLACEHOLDER}@{host}/{svc}")
}

/// The oslo.messaging transport URL (Q16 — internal RPC on `RabbitMQ`, strictly
/// separate from mde-bus per Q67).
fn transport_url() -> String {
    format!("rabbit://openstack:{SECRET_PLACEHOLDER}@{RABBIT_MESH_NAME}:{RABBIT_PORT}//")
}

/// The `[keystone_authtoken]` block every keystone-authenticated API shares
/// (Q21 — the mesh account is the cloud account; QC-5 seals the real password).
fn authtoken(svc: &str, ctx: &RenderCtx) -> String {
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
         password = {SECRET_PLACEHOLDER}\n",
        host = ctx.host,
    )
}

/// One `[DEFAULT]` binding an API to the overlay (Q22/23) + wiring RPC.
fn api_default(ctx: &RenderCtx) -> String {
    format!(
        "[DEFAULT]\n\
         debug = False\n\
         my_ip = {host}\n\
         osapi_compute_listen = {host}\n\
         transport_url = {rpc}\n",
        host = ctx.host,
        rpc = transport_url(),
    )
}

/// Build `kind`'s launch command + config files from the doctrine (design Q24
/// MVP service set). Real but minimal — enough for the container to come up on
/// the pinned release; QC-5 layers identity/bootstrap on top.
#[allow(clippy::too_many_lines)] // one arm per service reads best kept together
fn service_plan(kind: ServiceKind, ctx: &RenderCtx) -> ServicePlan {
    let one =
        |command: &str, name: &'static str, dest: &'static str, owner: &'static str, body: String| {
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
                 bind_address = {host}\n\
                 wsrep_on = OFF\n\
                 default_storage_engine = InnoDB\n\
                 max_connections = 4096\n\
                 [client]\n\
                 default_character_set = utf8\n",
                host = ctx.host,
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
                 default_pass = {SECRET_PLACEHOLDER}\n",
                host = ctx.host,
            ),
        ),
        // memcached is command-only (no config file).
        ServiceKind::Memcached => ServicePlan {
            command: format!("/usr/bin/memcached -vv -l {host}", host = ctx.host),
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
                 [database]\nconnection = {db}\n\
                 [cache]\nbackend = oslo_cache.memcache_pool\nenabled = True\n\
                 memcache_servers = {host}:{MEMCACHE_PORT}\n\
                 [token]\nprovider = fernet\n",
                db = db_url("keystone", ctx),
                host = ctx.host,
            ),
        ),
        ServiceKind::GlanceApi => one(
            "glance-api",
            "glance-api.conf",
            "/etc/glance/glance-api.conf",
            "glance:glance",
            format!(
                "[DEFAULT]\ndebug = False\nbind_host = {host}\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [glance_store]\nstores = file\ndefault_store = file\n\
                 filesystem_store_datadir = /var/lib/glance/images/\n",
                host = ctx.host,
                db = db_url("glance", ctx),
                authtoken = authtoken("glance", ctx),
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
                db = db_url("placement", ctx),
                authtoken = authtoken("placement", ctx),
            ),
        ),
        ServiceKind::NovaApi => nova("nova-api", ctx),
        ServiceKind::NovaScheduler => nova("nova-scheduler", ctx),
        ServiceKind::NovaConductor => nova("nova-conductor", ctx),
        ServiceKind::NovaCompute => nova("nova-compute", ctx),
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
                        "[DEFAULT]\ndebug = False\nbind_host = {host}\n\
                         core_plugin = ml2\nservice_plugins = router\n\
                         transport_url = {rpc}\n\
                         [database]\nconnection = {db}\n{authtoken}",
                        host = ctx.host,
                        rpc = transport_url(),
                        db = db_url("neutron", ctx),
                        authtoken = authtoken("neutron", ctx),
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
        ServiceKind::CinderApi => cinder("cinder-api", ctx),
        ServiceKind::CinderScheduler => cinder("cinder-scheduler", ctx),
        ServiceKind::CinderVolume => cinder("cinder-volume", ctx),
    }
}

/// The shared Nova plan (all four nova services read one `nova.conf`; only the
/// command differs — Q31 Nova+Placement own VM lifecycle).
fn nova(command: &str, ctx: &RenderCtx) -> ServicePlan {
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
                 [placement]\nauth_type = password\nauth_url = http://{ks}:{ksp}\n\
                 username = placement\npassword = {SECRET_PLACEHOLDER}\n\
                 user_domain_name = Default\nproject_domain_name = Default\n\
                 project_name = service\n\
                 [libvirt]\nvirt_type = kvm\n",
                default = api_default(ctx),
                api_db = db_url("nova_api", ctx),
                db = db_url("nova", ctx),
                authtoken = authtoken("nova", ctx),
                ks = KEYSTONE_MESH_NAME,
                ksp = KEYSTONE_PORT,
            ),
        }],
    }
}

/// The shared Cinder plan (LVM per node — Q51/59).
fn cinder(command: &str, ctx: &RenderCtx) -> ServicePlan {
    ServicePlan {
        command: command.to_string(),
        files: vec![ConfFile {
            name: "cinder.conf",
            dest: "/etc/cinder/cinder.conf",
            owner: "cinder:cinder",
            body: format!(
                "[DEFAULT]\ndebug = False\nmy_ip = {host}\n\
                 transport_url = {rpc}\nenabled_backends = lvm\n\
                 [database]\nconnection = {db}\n{authtoken}\
                 [lvm]\nvolume_driver = cinder.volume.drivers.lvm.LVMVolumeDriver\n\
                 volume_group = cinder-volumes\ntarget_protocol = iscsi\n",
                host = ctx.host,
                rpc = transport_url(),
                db = db_url("cinder", ctx),
                authtoken = authtoken("cinder", ctx),
            ),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::openstack::podman::config_rendered;

    fn ctx(leader: bool) -> RenderCtx {
        RenderCtx {
            release: "2024.1".into(),
            leader,
            host: "node-a".into(),
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
        assert_eq!(cfg["config_files"][0]["dest"], "/etc/keystone/keystone.conf");
        // The referenced conf actually rendered.
        assert!(svc_dir.join("keystone.conf").is_file());
        // No tmp file left behind (atomic).
        assert!(!svc_dir.join("config.json.tmp").exists());
    }

    #[test]
    fn render_is_parameterized_by_the_doctrine() {
        let dir = tempfile::tempdir().unwrap();
        // Non-leader: the DB target is the mesh-DNS name.
        render_service_config(dir.path(), ServiceKind::GlanceApi, &ctx(false)).unwrap();
        let body =
            std::fs::read_to_string(dir.path().join("glance_api").join("glance-api.conf")).unwrap();
        assert!(body.contains("bind_host = node-a"), "{body}");
        assert!(body.contains("@mariadb.mesh/glance"), "{body}");
        // The pinned release is stamped in (release-parameterized render).
        assert!(body.contains("kolla release 2024.1"), "{body}");

        // Leader: its own services reach the local MariaDB directly (Q15).
        let dir2 = tempfile::tempdir().unwrap();
        render_service_config(dir2.path(), ServiceKind::GlanceApi, &ctx(true)).unwrap();
        let body2 =
            std::fs::read_to_string(dir2.path().join("glance_api").join("glance-api.conf"))
                .unwrap();
        assert!(body2.contains("@node-a/glance"), "{body2}");
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
            let cfg: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(svc_dir.join("config.json")).unwrap())
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
}
