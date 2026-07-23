//! OW-7 — `mackesd onboard spawn-lighthouse`: promote a lone Workstation's
//! LAN-only mesh by standing up its first real **lighthouse** and migrating the CA
//! to it.
//!
//! A founded Workstation (OW-3 `mesh-create`) holds its own CA but has no
//! always-on lighthouse — the overlay is LAN-only and there is no durable CA home
//! off the desktop. This verb provisions one cloud droplet (`DigitalOcean` / the
//! `zone1-do` `IaC`), push-provisions it over SSH (RPM install + a
//! lighthouse-scoped enroll), then **migrates the CA** to it so the lighthouse
//! becomes the mesh's durable signer + etcd voter.
//!
//! The shape mirrors the sibling onboard verbs ([`crate::onboard::network`] /
//! [`crate::onboard::mesh_dns`]): a pure planning core the unit tests pin, plus a
//! thin **injectable apply seam** so the live side effects are faked in tests and
//! honestly integration-gated in production.
//! * [`gather`] — impure probe: reads the mesh-id, the CA-holder overlay IP, and
//!   whether a cloud token / operator SSH key are present.
//! * [`plan_spawn`] — pure fold: `[SpawnRequest] + [SpawnFacts] → [SpawnPlan]`,
//!   rendering the cloud-init provision spec, the enroll bootstrap, and the
//!   **ordered, idempotent CA-migration steps** — or the [`SpawnPlan::LanOnly`]
//!   outcome when it cannot provision yet.
//! * [`Provisioner`] — the injectable side-effect seam ([`provision`] →
//!   [`push_enroll`] → [`migrate_ca`]). Production [`LiveProvisioner`] returns a
//!   typed [`ProvisionError::IntegrationGated`] naming exactly what the live call
//!   needs (cloud token / live SSH / the CA signer); tests drive a recording fake.
//! * [`execute`] — pure orchestration over the seam (provision → push-enroll →
//!   migrate-CA, in that order), fully unit-tested through the fake.
//!
//! # Reuse, not reimplementation (§6)
//! This verb is glue over the mechanisms the mesh already has:
//! * The **CA-key delivery** is #12's existing contract — a **lighthouse-scoped**
//!   join bearer ([`crate::bearer_ledger::LIGHTHOUSE_ROLE_NOTE`]) authorizes the
//!   enroll signer to hand the new box the CA private key
//!   ([`crate::nebula_enroll`] `ca_key_pem`). We do not re-derive any crypto: the
//!   ordered [`CaMigrationStep`]s *describe* that flow so the real [`migrate_ca`]
//!   drives it (via `mint_join_token` / the bundle) idempotently.
//! * The live cloud path is the existing `mackesd lighthouse add` executor
//!   (`do-lighthouse-join.sh` + `doctl`), which the real [`Provisioner`] shells to.
//! * The mesh-id + CA-holder come from the founding bundle
//!   ([`crate::onboard::invite::resolve_mesh_id`] / [`crate::ca::bundle`]).
//!
//! # This slice (QC-15): cloud-only, no local cloud-hypervisor spawn
//! The older local cloud-hypervisor lighthouse path is deleted. The live DO API
//! call / SSH push / real CA move land behind [`Provisioner`], exactly as OW-5's
//! live `nmcli` apply sits behind [`crate::onboard::network::KeyfileSink`].
//! [`LiveProvisioner`] returning a typed `IntegrationGated` error (never a fake
//! success) is §7-legal — the same way OW-5's apply returns a typed error when
//! `NetworkManager` is not reachable.

use std::fmt::Write as _;

/// The dnf channel the spawned lighthouse installs the `mde` RPM from (the same
/// base URL `do-lighthouse-join.sh` defaults to — reused, not re-invented).
pub const REPO_BASEURL: &str = "https://matthewmackes.github.io/magic-mesh";

/// Default cloud region for a spawned lighthouse (matches `do-lighthouse-join.sh`).
pub const DEFAULT_CLOUD_REGION: &str = "nyc3";
/// Default cloud droplet size (the smallest DO Basic Droplet: 1 shared vCPU,
/// 512 MiB RAM and 10 GiB SSD). The cloud-init profile is the only supported
/// lighthouse shape: a thin relay/control-plane node with no media or
/// file-sharing subclass.
pub const DEFAULT_CLOUD_SIZE: &str = "s-1vcpu-512mb-10gb";
/// Default cloud image (matches `do-lighthouse-join.sh`).
pub const DEFAULT_CLOUD_IMAGE: &str = "fedora-43-x64";
/// The join-token placeholder the rendered provision spec carries.
///
/// The impure [`migrate_ca`] step mints a fresh lighthouse-scoped token and
/// substitutes it — the pure plan never embeds a secret (mirrors
/// `do-lighthouse-join.sh`'s `@JOIN_TOKEN@` sed seam).
pub const JOIN_TOKEN_PLACEHOLDER: &str = "{{JOIN_TOKEN}}";

/// Where a spawned lighthouse is provisioned.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpawnTarget {
    /// A cloud droplet (`DigitalOcean`, the `zone1-do` `IaC`).
    Cloud {
        /// DO region slug (e.g. `nyc3`).
        region: String,
        /// DO droplet size slug (e.g. `s-1vcpu-512mb-10gb`).
        size: String,
    },
}

impl SpawnTarget {
    /// A cloud target with the shared `do-lighthouse-join` defaults.
    #[must_use]
    pub fn default_cloud() -> Self {
        Self::Cloud {
            region: DEFAULT_CLOUD_REGION.to_string(),
            size: DEFAULT_CLOUD_SIZE.to_string(),
        }
    }
}

/// The spawn request — the [`SpawnTarget`] plus whether a **pair** (two
/// lighthouses, for an HA / two-voter etcd quorum) is requested rather than a lone
/// one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnRequest {
    /// Where to provision.
    pub target: SpawnTarget,
    /// Provision two lighthouses for quorum/HA (`false` ⇒ a single lighthouse).
    pub pair: bool,
}

/// The live facts [`gather`] reads off this node — the seam between the impure
/// probes and the pure [`plan_spawn`] fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnFacts {
    /// This mesh's id (from the founding bundle) — the lighthouse joins THIS mesh.
    pub mesh_id: String,
    /// Whether a cloud API token is present (`doctl` /
    /// `DIGITALOCEAN_ACCESS_TOKEN`). `false` on a Cloud target ⇒ the LAN-only +
    /// retry branch.
    pub cloud_token_present: bool,
    /// This node's overlay IP — the current CA holder, i.e. the migration source.
    /// `None` when this box has not founded a mesh (nothing to migrate).
    pub ca_holder_overlay_ip: Option<String>,
}

/// Why a spawn cannot provision right now — a real, retryable outcome the plan
/// carries (the mesh keeps running LAN-only; the operator retries once fixed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum LanOnlyReason {
    /// A cloud target was asked for but no cloud API token is present.
    NoCloudToken,
    /// This node has not founded a mesh, so it holds no CA to migrate.
    NotFounded,
}

impl LanOnlyReason {
    /// What the operator must fix before a retry succeeds.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::NoCloudToken => {
                "set a cloud token (DIGITALOCEAN_ACCESS_TOKEN / `doctl auth init`), then retry"
            }
            Self::NotFounded => "found this mesh first (`mackesd onboard mesh-create`), then retry",
        }
    }
}

impl std::fmt::Display for LanOnlyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NoCloudToken => "no cloud token",
            Self::NotFounded => "no founded mesh / CA on this node",
        };
        f.write_str(s)
    }
}

/// One ordered, idempotent step of migrating the CA from the current holder (this
/// Workstation) to the freshly spawned lighthouse.
///
/// The steps *describe* the flow the real [`migrate_ca`] drives over the existing
/// #12 mechanism; each is phrased so a re-run on an already-migrated mesh is a
/// no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum CaMigrationStep {
    /// 1. Mint a **lighthouse-scoped** join bearer on the CA holder — the
    ///    `role:lighthouse` note authorizes the enroll signer to deliver the CA
    ///    key (#12). Idempotent: reuse an outstanding unredeemed lighthouse bearer.
    MintLighthouseToken,
    /// 2. The lighthouse-scoped enroll hands the new box the CA cert **and CA
    ///    private key** (the CA is mirrored to it) plus a Host cert — so it can
    ///    itself sign/enroll. No-op if the box already carries the CA key.
    DeliverCaKey,
    /// 3. Record the box as a lighthouse: the founding bundle's `lighthouses` list
    ///    + a roster row (role=lighthouse, its `external_addr`) so peers dial it.
    RegisterLighthouse,
    /// 4. Admit it to the etcd quorum as a voter (#11) — a durable CA home.
    ///    No-op if it is already a member.
    AdmitToQuorum,
    /// 5. Step this Workstation down as the *sole* CA holder: the always-on
    ///    lighthouse is now the canonical signer. Idempotent; the WS keeps its copy
    ///    (it does not destroy CA material — only relinquishes primacy).
    StepDownHolder,
}

impl CaMigrationStep {
    /// The canonical, ordered CA-migration sequence for a spawn.
    #[must_use]
    pub fn ordered() -> Vec<Self> {
        vec![
            Self::MintLighthouseToken,
            Self::DeliverCaKey,
            Self::RegisterLighthouse,
            Self::AdmitToQuorum,
            Self::StepDownHolder,
        ]
    }

    /// A one-line human description of the step.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::MintLighthouseToken => {
                "mint a lighthouse-scoped join token (authorizes CA-key delivery, #12)"
            }
            Self::DeliverCaKey => {
                "deliver the CA cert+key to the new lighthouse via the scoped enroll"
            }
            Self::RegisterLighthouse => {
                "register it as a lighthouse (bundle + roster) so peers dial it"
            }
            Self::AdmitToQuorum => "admit it to the etcd quorum as a voter (durable CA home)",
            Self::StepDownHolder => "step this Workstation down as the sole CA holder",
        }
    }
}

/// The rendered provisioning spec — the cloud-init user-data for the cloud
/// droplet.
///
/// Deterministic given the target, so it round-trips in tests. Carries a
/// [`JOIN_TOKEN_PLACEHOLDER`] the enroll step substitutes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ProvisionSpec {
    /// A cloud droplet: its region/size/image + the rendered cloud-init user-data.
    CloudInit {
        /// DO region slug.
        region: String,
        /// DO droplet size slug.
        size: String,
        /// DO image slug.
        image: String,
        /// The rendered `#cloud-config` user-data (installs the RPM + joins).
        user_data: String,
    },
}

impl ProvisionSpec {
    /// The rendered provisioning document for the dry-run print + the real
    /// provisioner.
    #[must_use]
    pub fn document(&self) -> &str {
        match self {
            Self::CloudInit { user_data, .. } => user_data,
        }
    }
}

/// The first-boot enroll contract the spawned box runs to join as a lighthouse.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EnrollBootstrap {
    /// The role the box takes — always `lighthouse` for a spawn.
    pub role: String,
    /// The command the box runs on first boot (the token is substituted for
    /// [`JOIN_TOKEN_PLACEHOLDER`] at apply time).
    pub command: String,
    /// Whether the join token must be lighthouse-scoped so the enroll delivers the
    /// CA key (#12). Always `true` for a spawn — that IS the CA migration's vehicle.
    pub lighthouse_scoped: bool,
}

/// A resolved spawn plan — the headless body the CLI prints and [`execute`] drives.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SpawnPlan {
    /// The target can be provisioned: the spec, the enroll bootstrap, and the
    /// ordered CA-migration steps.
    Provision {
        /// The mesh the lighthouse joins.
        mesh_id: String,
        /// Whether a pair (two lighthouses) is provisioned.
        pair: bool,
        /// The rendered provisioning spec.
        spec: ProvisionSpec,
        /// The first-boot enroll bootstrap.
        enroll: EnrollBootstrap,
        /// The ordered, idempotent CA-migration steps.
        ca_migration: Vec<CaMigrationStep>,
    },
    /// The spawn cannot provision right now → the mesh stays LAN-only and the
    /// operator can retry once the [`LanOnlyReason`]'s blocker clears.
    LanOnly {
        /// Why provisioning is blocked (and, via [`LanOnlyReason::hint`], the fix).
        reason: LanOnlyReason,
    },
}

impl SpawnPlan {
    /// Whether a retry is available (always true for the LAN-only outcome — the
    /// mesh keeps running and the operator retries after fixing the blocker).
    #[must_use]
    pub const fn retry_available(&self) -> bool {
        matches!(self, Self::LanOnly { .. })
    }

    /// The rendered provisioning spec, when this plan provisions.
    #[must_use]
    pub const fn provision_spec(&self) -> Option<&ProvisionSpec> {
        match self {
            Self::Provision { spec, .. } => Some(spec),
            Self::LanOnly { .. } => None,
        }
    }

    /// How many lighthouses this plan stands up (0 for LAN-only, 2 for a pair).
    #[must_use]
    pub const fn lighthouse_count(&self) -> usize {
        match self {
            Self::LanOnly { .. } => 0,
            Self::Provision { pair, .. } => {
                if *pair {
                    2
                } else {
                    1
                }
            }
        }
    }

    /// A one-line human summary (no trailing newline — the CLI wraps it in
    /// `println!`, mirroring the sibling verbs).
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Self::LanOnly { reason } => {
                format!(
                    "stays LAN-only ({reason}) — retry available once you {}",
                    reason.hint()
                )
            }
            Self::Provision {
                mesh_id,
                pair,
                spec,
                ca_migration,
                ..
            } => {
                let where_ = match spec {
                    ProvisionSpec::CloudInit { region, size, .. } => {
                        format!("cloud droplet ({size} in {region})")
                    }
                };
                let count = if *pair {
                    "a pair of lighthouses"
                } else {
                    "a lighthouse"
                };
                format!(
                    "spawn {count} for mesh `{mesh_id}` as {where_}, \
                     then migrate the CA in {} step(s)",
                    ca_migration.len()
                )
            }
        }
    }
}

/// Pure fold: turn a [`SpawnRequest`] + gathered [`SpawnFacts`] into a
/// [`SpawnPlan`]. No I/O — fully unit-testable.
///
/// A Cloud target with no cloud token or an un-founded node (no CA to migrate)
/// resolves to the retryable [`SpawnPlan::LanOnly`] outcome. Otherwise the plan
/// renders the provision spec, the lighthouse-scoped enroll bootstrap, and the
/// ordered CA-migration steps.
#[must_use]
pub fn plan_spawn(req: &SpawnRequest, facts: &SpawnFacts) -> SpawnPlan {
    // A spawn migrates *this node's* CA, so it must hold one (be founded).
    if facts.ca_holder_overlay_ip.is_none() {
        return SpawnPlan::LanOnly {
            reason: LanOnlyReason::NotFounded,
        };
    }

    // The no-cloud-token → LAN-only + retry branch is a real code path, not a
    // comment.
    match &req.target {
        SpawnTarget::Cloud { .. } if !facts.cloud_token_present => {
            return SpawnPlan::LanOnly {
                reason: LanOnlyReason::NoCloudToken,
            };
        }
        _ => {}
    }

    let spec = render_spec(&req.target);
    SpawnPlan::Provision {
        mesh_id: facts.mesh_id.clone(),
        pair: req.pair,
        spec,
        enroll: enroll_bootstrap(&facts.mesh_id),
        ca_migration: CaMigrationStep::ordered(),
    }
}

/// Pure renderer: the provisioning spec for `target`.
///
/// Cloud → a `#cloud-config` user-data that installs the RPM off the channel and
/// runs the lighthouse join. It carries the [`JOIN_TOKEN_PLACEHOLDER`] the enroll
/// step substitutes.
#[must_use]
pub fn render_spec(target: &SpawnTarget) -> ProvisionSpec {
    match target {
        SpawnTarget::Cloud { region, size } => {
            let mut user_data = String::new();
            let _ = writeln!(user_data, "#cloud-config");
            let _ = writeln!(
                user_data,
                "# spawn-lighthouse: join THIS mesh as a lighthouse"
            );
            let _ = writeln!(user_data, "runcmd:");
            let _ = writeln!(
                user_data,
                "  - dnf -y install --repofrompath \"mde,{REPO_BASEURL}\" --nogpgcheck mde"
            );
            let _ = writeln!(
                user_data,
                "  - mackesd join --role lighthouse {JOIN_TOKEN_PLACEHOLDER}"
            );
            let _ = writeln!(
                user_data,
                "  - /usr/libexec/mackesd/configure-small-lighthouse small"
            );
            let _ = writeln!(
                user_data,
                "  - sh -c 'echo OK > /root/mesh-join-status.txt'"
            );
            ProvisionSpec::CloudInit {
                region: region.clone(),
                size: size.clone(),
                image: DEFAULT_CLOUD_IMAGE.to_string(),
                user_data,
            }
        }
    }
}

/// Pure: the lighthouse-scoped first-boot enroll bootstrap for `mesh_id`.
#[must_use]
pub fn enroll_bootstrap(mesh_id: &str) -> EnrollBootstrap {
    EnrollBootstrap {
        role: "lighthouse".to_string(),
        command: format!(
            "mackesd join --role lighthouse {JOIN_TOKEN_PLACEHOLDER}  # mesh {mesh_id}"
        ),
        lighthouse_scoped: true,
    }
}

/// The reachable box a [`Provisioner`] stood up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The public/reachable host (droplet public IPv4).
    pub host: String,
    /// The overlay IP the box takes as a lighthouse once enrolled (`None` until
    /// the enroll signs it).
    pub overlay_ip: Option<String>,
}

/// A typed failure from the injectable [`Provisioner`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionError {
    /// The live path is not runnable in this build/environment yet — it needs a
    /// real prerequisite (cloud token / live SSH / the CA signer). Names the step
    /// + what is missing. §7-legal: a real method returning a real typed error,
    ///   exactly as OW-5's apply does when `NetworkManager` is unreachable.
    IntegrationGated {
        /// Which seam step (`provision` / `push-enroll` / `migrate-ca`).
        step: &'static str,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// A step failed for a concrete runtime reason.
    Failed {
        /// Which seam step failed.
        step: &'static str,
        /// The failure detail.
        reason: String,
    },
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { step, reason } => {
                write!(f, "{step}: integration-gated — {reason}")
            }
            Self::Failed { step, reason } => write!(f, "{step}: {reason}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

/// The injectable side-effect seam. Production is [`LiveProvisioner`]; tests use a
/// recording fake so the pure orchestration is exercised without a real cloud /
/// SSH / CA move.
pub trait Provisioner {
    /// Provision the box from `spec`, returning its reachable [`Endpoint`].
    ///
    /// # Errors
    /// A [`ProvisionError`] — `IntegrationGated` when the live provisioner can't
    /// run yet, else `Failed`.
    fn provision(&self, spec: &ProvisionSpec) -> Result<Endpoint, ProvisionError>;

    /// Push-provision: SSH in and run `enroll` so the box joins as a lighthouse.
    ///
    /// # Errors
    /// A [`ProvisionError`] (`IntegrationGated` without live SSH, else `Failed`).
    fn push_enroll(
        &self,
        endpoint: &Endpoint,
        enroll: &EnrollBootstrap,
    ) -> Result<(), ProvisionError>;

    /// Execute the ordered, idempotent CA-migration `steps` against `endpoint`.
    ///
    /// # Errors
    /// A [`ProvisionError`] (`IntegrationGated` without the live CA signer, else
    /// `Failed`).
    fn migrate_ca(
        &self,
        endpoint: &Endpoint,
        steps: &[CaMigrationStep],
    ) -> Result<(), ProvisionError>;
}

/// Production [`Provisioner`] — the live cloud spawn + SSH push + CA move.
///
/// OW-7's **push-enroll** is a **bootstrap** remote push (the target box is not on
/// the mesh yet): [`push_enroll`](Self::push_enroll) drives the shared OW-15
/// [`RemotePush`](crate::onboard::remote_push::RemotePush) executor over the
/// bearer-scoped [`SshBootstrap`](crate::onboard::remote_push::SshBootstrap)
/// transport (the single-use enroll bearer, no ambient SSH key). The transport is
/// an **injectable seam** (default: the honestly-gated production `SshBootstrap`;
/// tests use a fake), and reaching the box over live SSH stays operator/live-gated
/// (§7 — a typed error, never a fake success).
///
/// `provision` (the cloud/VM spawn) and `migrate_ca` (the CA move) are not
/// remote-push concerns and stay honestly integration-gated on their own live
/// prerequisites (a cloud token / the CA signer).
pub struct LiveProvisioner {
    /// The OW-15 bootstrap remote-push transport. Default: [`SshBootstrap`]; tests
    /// inject a recording fake to prove the wiring without a live SSH round-trip.
    ///
    /// [`SshBootstrap`]: crate::onboard::remote_push::SshBootstrap
    remote_push: std::sync::Arc<dyn crate::onboard::remote_push::RemotePush + Send + Sync>,
}

impl Default for LiveProvisioner {
    fn default() -> Self {
        Self {
            remote_push: std::sync::Arc::new(crate::onboard::remote_push::SshBootstrap),
        }
    }
}

impl LiveProvisioner {
    /// Inject the bootstrap remote-push transport (tests use a recording fake).
    #[must_use]
    pub fn with_remote_push(
        mut self,
        transport: std::sync::Arc<dyn crate::onboard::remote_push::RemotePush + Send + Sync>,
    ) -> Self {
        self.remote_push = transport;
        self
    }
}

/// Map an OW-15 [`RemotePushError`](crate::onboard::remote_push::RemotePushError)
/// into the provisioner seam's typed error for the push-enroll step.
fn remote_push_to_provision_error(
    e: crate::onboard::remote_push::RemotePushError,
    endpoint: &Endpoint,
) -> ProvisionError {
    use crate::onboard::remote_push::RemotePushError as R;
    let detail = e.to_string();
    match e {
        R::NotWired { .. } | R::Unreachable { .. } => ProvisionError::IntegrationGated {
            step: "push-enroll",
            reason: format!(
                "needs live SSH to {} to run the lighthouse enroll over the single-use bearer \
                 (OW-15 SshBootstrap transport: {detail})",
                endpoint.host
            ),
        },
        R::BundleRejected { why } => ProvisionError::Failed {
            step: "push-enroll",
            reason: why,
        },
        R::ActionFailed { action, why } => ProvisionError::Failed {
            step: "push-enroll",
            reason: format!("{action}: {why}"),
        },
    }
}

impl Provisioner for LiveProvisioner {
    fn provision(&self, spec: &ProvisionSpec) -> Result<Endpoint, ProvisionError> {
        let reason = match spec {
            ProvisionSpec::CloudInit { .. } => {
                "needs a cloud token (DIGITALOCEAN_ACCESS_TOKEN) + the `do-lighthouse-join` \
                 executor (doctl)"
            }
        };
        Err(ProvisionError::IntegrationGated {
            step: "provision",
            reason: reason.to_string(),
        })
    }

    fn push_enroll(
        &self,
        endpoint: &Endpoint,
        enroll: &EnrollBootstrap,
    ) -> Result<(), ProvisionError> {
        // OW-15 bootstrap remote push: reach the fresh box over bearer-scoped SSH
        // and run ONLY the enroll step (the single-use bearer, no ambient key). The
        // enroll invocation (carrying the join-token placeholder substituted at
        // apply time) rides the RunEnroll action; SshBootstrap refuses anything else.
        let target = crate::onboard::remote_push::Target::Bootstrap {
            host: endpoint.host.clone(),
        };
        let actions = [crate::onboard::remote_push::Action::RunEnroll {
            bearer: enroll.command.clone(),
        }];
        self.remote_push
            .apply(&target, &actions)
            .map_err(|e| remote_push_to_provision_error(e, endpoint))
    }

    fn migrate_ca(
        &self,
        _endpoint: &Endpoint,
        _steps: &[CaMigrationStep],
    ) -> Result<(), ProvisionError> {
        Err(ProvisionError::IntegrationGated {
            step: "migrate-ca",
            reason: "needs the live CA signer to mint the lighthouse-scoped token + move the CA"
                .to_string(),
        })
    }
}

/// The result of a [`execute`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnOutcome {
    /// The lighthouse(s) were provisioned + enrolled + the CA migrated.
    Provisioned {
        /// The reachable endpoint the provisioner returned.
        endpoint: Endpoint,
    },
    /// The plan was LAN-only — nothing was provisioned; a retry is available.
    LanOnly {
        /// Why provisioning was blocked.
        reason: LanOnlyReason,
    },
}

/// Pure orchestration over the [`Provisioner`] seam.
///
/// For a [`SpawnPlan::Provision`] run provision → push-enroll → migrate-CA **in
/// that order**; for [`SpawnPlan::LanOnly`] short-circuit to the retryable outcome
/// (no seam calls).
///
/// This is the tested orchestration the fake pins; the real side effects live
/// entirely in the injected `prov`.
///
/// # Errors
/// Propagates the first [`ProvisionError`] any seam step returns.
pub fn execute(plan: &SpawnPlan, prov: &dyn Provisioner) -> Result<SpawnOutcome, ProvisionError> {
    match plan {
        SpawnPlan::LanOnly { reason } => Ok(SpawnOutcome::LanOnly { reason: *reason }),
        SpawnPlan::Provision {
            spec,
            enroll,
            ca_migration,
            ..
        } => {
            let endpoint = prov.provision(spec)?;
            prov.push_enroll(&endpoint, enroll)?;
            prov.migrate_ca(&endpoint, ca_migration)?;
            Ok(SpawnOutcome::Provisioned { endpoint })
        }
    }
}

/// Impure probe shell: gather the live spawn facts off this node.
///
/// Best-effort — a missing bundle / binary degrades to `None`/`false` fields
/// rather than erroring, so the pure [`plan_spawn`] fold always runs and produces
/// the real verdict (LAN-only when a prerequisite is absent). The mesh-id + CA
/// holder come from the founding bundle (reuse, not reinvention).
#[must_use]
pub fn gather(workgroup_root: &std::path::Path, node_id: &str) -> SpawnFacts {
    let mesh_id = crate::onboard::invite::resolve_mesh_id(workgroup_root, node_id);
    let ca_holder_overlay_ip =
        crate::ca::bundle::read_bundle(&crate::ca::bundle::bundle_path(workgroup_root, node_id))
            .ok()
            .map(|b| b.overlay_ip);
    SpawnFacts {
        mesh_id,
        cloud_token_present: cloud_token_present(),
        ca_holder_overlay_ip,
    }
}

/// Whether a cloud API token is present in the environment (the `doctl` /
/// `DigitalOcean` vars). Pure over the process env — the signal the no-cloud-token
/// branch keys off.
#[must_use]
fn cloud_token_present() -> bool {
    [
        "DIGITALOCEAN_ACCESS_TOKEN",
        "DIGITALOCEAN_TOKEN",
        "DO_TOKEN",
    ]
    .iter()
    .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn facts(cloud_token: bool, founded: bool) -> SpawnFacts {
        SpawnFacts {
            mesh_id: "home-deadbeef".to_string(),
            cloud_token_present: cloud_token,
            ca_holder_overlay_ip: founded.then(|| "10.42.0.1".to_string()),
        }
    }

    fn cloud_req(pair: bool) -> SpawnRequest {
        SpawnRequest {
            target: SpawnTarget::default_cloud(),
            pair,
        }
    }

    #[test]
    fn cloud_with_token_plans_a_provision() {
        let plan = plan_spawn(&cloud_req(false), &facts(true, true));
        match &plan {
            SpawnPlan::Provision {
                mesh_id,
                pair,
                spec,
                enroll,
                ca_migration,
            } => {
                assert_eq!(mesh_id, "home-deadbeef");
                assert!(!pair);
                assert!(matches!(spec, ProvisionSpec::CloudInit { .. }));
                // The enroll is lighthouse-scoped — that IS the CA migration vehicle.
                assert!(enroll.lighthouse_scoped);
                assert_eq!(enroll.role, "lighthouse");
                assert_eq!(ca_migration, &CaMigrationStep::ordered());
            }
            SpawnPlan::LanOnly { .. } => panic!("expected a provision plan"),
        }
        assert!(!plan.retry_available());
        assert_eq!(plan.lighthouse_count(), 1);
    }

    #[test]
    fn cloud_without_token_stays_lan_only_with_retry() {
        // The headline no-cloud-token → LAN-only + retry branch (a real path).
        let plan = plan_spawn(&cloud_req(false), &facts(false, true));
        assert_eq!(
            plan,
            SpawnPlan::LanOnly {
                reason: LanOnlyReason::NoCloudToken
            }
        );
        assert!(
            plan.retry_available(),
            "the operator can retry once a token exists"
        );
        assert!(plan.provision_spec().is_none());
        assert!(plan.human().contains("retry available"));
    }

    #[test]
    fn unfounded_node_cannot_migrate_a_ca() {
        // No CA holder ⇒ nothing to migrate, even with a token present.
        let plan = plan_spawn(&cloud_req(false), &facts(true, false));
        assert_eq!(
            plan,
            SpawnPlan::LanOnly {
                reason: LanOnlyReason::NotFounded
            }
        );
        assert!(plan.human().contains("found this mesh first"));
    }

    #[test]
    fn a_pair_provisions_two_lighthouses() {
        let plan = plan_spawn(&cloud_req(true), &facts(true, true));
        assert_eq!(plan.lighthouse_count(), 2);
        assert!(plan.human().contains("pair of lighthouses"));
    }

    #[test]
    fn ca_migration_is_ordered_and_stable() {
        let steps = CaMigrationStep::ordered();
        assert_eq!(
            steps,
            vec![
                CaMigrationStep::MintLighthouseToken,
                CaMigrationStep::DeliverCaKey,
                CaMigrationStep::RegisterLighthouse,
                CaMigrationStep::AdmitToQuorum,
                CaMigrationStep::StepDownHolder,
            ],
            "the migration order is fixed"
        );
        // The token mint must precede the CA-key delivery (the enroll consumes it).
        let mint = steps
            .iter()
            .position(|s| *s == CaMigrationStep::MintLighthouseToken)
            .unwrap();
        let deliver = steps
            .iter()
            .position(|s| *s == CaMigrationStep::DeliverCaKey)
            .unwrap();
        assert!(
            mint < deliver,
            "mint the token before delivering the CA key"
        );
        // Every step has a non-empty description.
        assert!(steps.iter().all(|s| !s.describe().is_empty()));
    }

    #[test]
    fn cloud_spec_carries_the_join_bootstrap_and_placeholder() {
        let spec = render_spec(&SpawnTarget::default_cloud());
        let doc = spec.document();
        assert!(doc.starts_with("#cloud-config"));
        // Installs the RPM off the shared channel + runs the lighthouse join.
        assert!(doc.contains(REPO_BASEURL));
        assert!(doc.contains("mackesd join --role lighthouse"));
        assert!(doc.contains("configure-small-lighthouse small"));
        // The secret is a placeholder the enroll step substitutes — never embedded.
        assert!(doc.contains(JOIN_TOKEN_PLACEHOLDER));
    }

    #[test]
    fn render_spec_is_deterministic() {
        let a = render_spec(&SpawnTarget::default_cloud());
        let b = render_spec(&SpawnTarget::default_cloud());
        assert_eq!(a, b, "same target ⇒ byte-identical spec");
    }

    #[test]
    fn spawn_request_round_trips_through_serde() {
        let req = SpawnRequest {
            target: SpawnTarget::Cloud {
                region: "sfo3".to_string(),
                size: "s-2vcpu-2gb".to_string(),
            },
            pair: true,
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let back: SpawnRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(req, back);
    }

    /// Recording [`Provisioner`] fake: records the ordered calls so the pure
    /// orchestration is asserted without a real cloud / SSH / CA move.
    struct RecordingProvisioner {
        calls: RefCell<Vec<String>>,
        seen_spec: RefCell<Option<ProvisionSpec>>,
        seen_steps: RefCell<Vec<CaMigrationStep>>,
    }

    impl RecordingProvisioner {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                seen_spec: RefCell::new(None),
                seen_steps: RefCell::new(Vec::new()),
            }
        }
    }

    impl Provisioner for RecordingProvisioner {
        fn provision(&self, spec: &ProvisionSpec) -> Result<Endpoint, ProvisionError> {
            self.calls.borrow_mut().push("provision".to_string());
            *self.seen_spec.borrow_mut() = Some(spec.clone());
            Ok(Endpoint {
                host: "203.0.113.7".to_string(),
                overlay_ip: None,
            })
        }
        fn push_enroll(
            &self,
            endpoint: &Endpoint,
            enroll: &EnrollBootstrap,
        ) -> Result<(), ProvisionError> {
            assert_eq!(
                endpoint.host, "203.0.113.7",
                "push-enroll sees provision's endpoint"
            );
            assert!(
                enroll.lighthouse_scoped,
                "the enroll must be lighthouse-scoped"
            );
            self.calls.borrow_mut().push("push_enroll".to_string());
            Ok(())
        }
        fn migrate_ca(
            &self,
            _endpoint: &Endpoint,
            steps: &[CaMigrationStep],
        ) -> Result<(), ProvisionError> {
            self.calls.borrow_mut().push("migrate_ca".to_string());
            *self.seen_steps.borrow_mut() = steps.to_vec();
            Ok(())
        }
    }

    #[test]
    fn execute_drives_the_seam_in_order() {
        let plan = plan_spawn(&cloud_req(false), &facts(true, true));
        let prov = RecordingProvisioner::new();
        let outcome = execute(&plan, &prov).expect("execute");
        match outcome {
            SpawnOutcome::Provisioned { endpoint } => {
                assert_eq!(endpoint.host, "203.0.113.7");
            }
            SpawnOutcome::LanOnly { .. } => panic!("expected a provisioned outcome"),
        }
        // The seam ran provision → push_enroll → migrate_ca, in that order.
        assert_eq!(
            *prov.calls.borrow(),
            vec!["provision", "push_enroll", "migrate_ca"]
        );
        // migrate_ca received the exact ordered CA-migration steps.
        assert_eq!(*prov.seen_steps.borrow(), CaMigrationStep::ordered());
        assert!(matches!(
            prov.seen_spec.borrow().as_ref(),
            Some(ProvisionSpec::CloudInit { .. })
        ));
    }

    #[test]
    fn execute_short_circuits_a_lan_only_plan() {
        // A LAN-only plan makes no seam calls — nothing to provision.
        let plan = plan_spawn(&cloud_req(false), &facts(false, true));
        let prov = RecordingProvisioner::new();
        let outcome = execute(&plan, &prov).expect("execute");
        assert_eq!(
            outcome,
            SpawnOutcome::LanOnly {
                reason: LanOnlyReason::NoCloudToken
            }
        );
        assert!(prov.calls.borrow().is_empty(), "no seam calls on LAN-only");
    }

    #[test]
    fn live_provisioner_is_integration_gated_not_fake_success() {
        let prov = LiveProvisioner::default();
        let spec = render_spec(&SpawnTarget::default_cloud());
        let err = prov
            .provision(&spec)
            .expect_err("live provision must not fake success");
        match err {
            ProvisionError::IntegrationGated { step, reason } => {
                assert_eq!(step, "provision");
                assert!(
                    reason.contains("cloud token"),
                    "reason names the missing prereq"
                );
            }
            ProvisionError::Failed { .. } => panic!("expected an integration-gated error"),
        }
        // push-enroll + migrate-ca are likewise integration-gated (typed, honest).
        let ep = Endpoint {
            host: "203.0.113.7".to_string(),
            overlay_ip: None,
        };
        assert!(matches!(
            prov.push_enroll(&ep, &enroll_bootstrap("home-deadbeef")),
            Err(ProvisionError::IntegrationGated {
                step: "push-enroll",
                ..
            })
        ));
        assert!(matches!(
            prov.migrate_ca(&ep, &CaMigrationStep::ordered()),
            Err(ProvisionError::IntegrationGated {
                step: "migrate-ca",
                ..
            })
        ));
    }

    #[test]
    fn execute_propagates_the_integration_gated_error() {
        // Through the LIVE provisioner, execute surfaces the first typed error.
        let plan = plan_spawn(&cloud_req(false), &facts(true, true));
        let err = execute(&plan, &LiveProvisioner::default()).expect_err("live path is gated");
        assert!(matches!(
            err,
            ProvisionError::IntegrationGated {
                step: "provision",
                ..
            }
        ));
    }

    // ── OW-15 wiring: push_enroll drives the bootstrap RemotePush (fake) ──

    #[test]
    fn push_enroll_drives_the_bootstrap_remote_push_with_run_enroll() {
        use crate::onboard::remote_push::{Action, RemotePush, RemotePushError, Target};
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct RecordingPush {
            seen: Mutex<Vec<(Target, Vec<Action>)>>,
        }
        impl RemotePush for RecordingPush {
            fn apply(&self, target: &Target, actions: &[Action]) -> Result<(), RemotePushError> {
                self.seen
                    .lock()
                    .expect("seen mutex")
                    .push((target.clone(), actions.to_vec()));
                Ok(())
            }
        }

        let push = Arc::new(RecordingPush::default());
        let prov = LiveProvisioner::default().with_remote_push(push.clone());
        let ep = Endpoint {
            host: "203.0.113.7".to_string(),
            overlay_ip: None,
        };
        prov.push_enroll(&ep, &enroll_bootstrap("home-deadbeef"))
            .expect("fake transport ⇒ wiring proven");

        let seen = push.seen.lock().expect("seen mutex");
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].0,
            Target::Bootstrap {
                host: "203.0.113.7".into()
            },
            "bootstrap push targets the fresh box over SSH"
        );
        assert!(
            matches!(&seen[0].1[0], Action::RunEnroll { .. }),
            "the bootstrap instant runs only enroll"
        );
    }
}
