//! OW-11 — `mackesd onboard service-add`: the post-onboarding **Services** flow.
//!
//! Onboarding ends with a working network; the *Services* flow (design lock #20)
//! is a **separate, day-2 step that never blocks it**. This verb adds one of the
//! three curated services (#17) to the running mesh:
//! * **Music** — retained as a non-lighthouse service record, but the historical
//!   media-lighthouse provisioning path is retired. DigitalOcean lighthouses are
//!   thin control-plane nodes and are never promoted to media/file-sharing hosts.
//! * **Files** (#37) — **peer-to-peer only**: the shipped `mde-files` Send-To over
//!   the Bus. There is **no VM/container to stand up** (services never block the
//!   working network) — the honest outcome is "already P2P", not a faked service.
//! * **Voice** (#36) — **register to an external SIP provider** (never a PBX VM we
//!   spawn); the plan captures the SIP account, the live registration is gated on
//!   the operator's SIP creds.
//!
//! The shape mirrors the sibling onboard verbs
//! ([`crate::onboard::spawn_lighthouse`] / [`crate::onboard::first_desktop`]): a
//! pure planning core the unit tests pin, plus a thin **injectable apply seam** so
//! the live side effects are faked in tests and honestly integration-gated in
//! production.
//! * [`gather`] — impure probe: the mesh's lighthouse set (from the replicated
//!   peer roster), retained for read-only diagnostics.
//! * [`plan_service_add`] — pure fold: `[ServiceAddRequest] + [ServiceAddFacts] →
//!   [ServiceAddPlan]`, branching per [`ServiceKind`] and refusing the retired
//!   lighthouse-media path / capturing the SIP account / resolving the honest
//!   P2P no-op.
//! * [`ServiceApply`] — the injectable side-effect seam ([`ServiceApply::provision_music`]
//!   / [`ServiceApply::register_voice`]). Production [`LiveServiceApply`] returns a
//!   typed [`ServiceError::IntegrationGated`] naming exactly what the live call
//!   needs (the container spawn, the DO Spaces bucket, the SIP registration); tests
//!   drive a recording fake. **Files needs no apply method** — there is genuinely
//!   nothing to spawn.
//! * [`execute`] — pure orchestration over the seam, fully unit-tested through the
//!   fake.
//!
//! # Historical media primitives
//! The old media-lighthouse glue remains represented for backwards-compatible
//! decoding and diagnostics, but the planner and live apply path refuse it. No
//! command in this flow can create or promote a media/file-sharing lighthouse.
//! * The **DO Spaces creds** are named by
//!   [`crate::ipc::secret_store::media_spaces_creds_ref`] (`"media-spaces"`, the
//!   leader-managed S3 + Navidrome shared-account secret MEDIA-2 seals) — reused
//!   verbatim via [`media_spaces_creds_ref`], never re-derived.
//! * The historical [`mde_role::Capability::Media`] marker is ignored for
//!   persisted lighthouse state under the thin-lighthouse policy.
//! * The **published endpoint** is [`crate::mesh_media`]'s
//!   [`music_mesh_server_url`](crate::mesh_media::music_mesh_server_url)
//!   (`http://music.mesh:4533`) — reused so the plan and the registry agree.
//!
//! # This slice (OW-11): the pure core + the injectable seam — NOT the live infra
//! The live Navidrome container spawn (`install-helpers/setup-media-navidrome.sh`:
//! the rclone bucket mount + rootless-podman Navidrome), the real SIP `REGISTER`,
//! and the DO Spaces provisioning land behind [`ServiceApply`], exactly as OW-7's
//! live provision sits behind [`crate::onboard::spawn_lighthouse::Provisioner`].
//! [`LiveServiceApply`] returning a typed `IntegrationGated` error (never a fake
//! success) is §7-legal.

use crate::onboard::spawn_lighthouse::{self, ProvisionSpec, SpawnTarget};

/// The `mde-files` P2P transport the Files service uses (design lock #37): the
/// shipped Send-To over the Bus — there is no central VM/container.
pub const FILES_TRANSPORT: &str = "mde-files Send-To (peer-to-peer over the Bus)";

/// The mesh secret-store key naming the media (DO Spaces S3 + Navidrome shared
/// account) credential the Music service reads.
///
/// Reused verbatim from [`crate::ipc::secret_store::media_spaces_creds_ref`] when
/// the IPC surface is compiled in (the `async-services` build the daemon + tests
/// use); a byte-identical literal keeps the lean library build (no `async-services`)
/// compiling without pulling the IPC surface in. A test under `async-services`
/// pins the two equal (single source of truth).
#[cfg(feature = "async-services")]
#[must_use]
pub fn media_spaces_creds_ref() -> String {
    crate::ipc::secret_store::media_spaces_creds_ref()
}

/// Lean-build twin of [`media_spaces_creds_ref`] (byte-identical to
/// [`crate::ipc::secret_store::media_spaces_creds_ref`]'s `"media-spaces"`).
#[cfg(not(feature = "async-services"))]
#[must_use]
pub fn media_spaces_creds_ref() -> String {
    "media-spaces".to_string()
}

/// The published server URL the Music service is reached at.
///
/// Reused verbatim from [`crate::mesh_media::music_mesh_server_url`]
/// (`http://music.mesh:4533`) so the plan and the media registry agree.
#[must_use]
pub fn music_mesh_server_url() -> String {
    crate::mesh_media::music_mesh_server_url()
}

/// Which curated service (#17) to add. `Copy` — a tiny tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    /// Navidrome on a media-lighthouse reading DO Spaces (#18,#19,#35,#41).
    Music,
    /// P2P `mde-files` Send-To — no VM/container (#37).
    Files,
    /// Register to an external SIP provider — no PBX VM (#36).
    Voice,
}

impl ServiceKind {
    /// Parse the CLI `--kind` value (`music` | `files` | `voice`), case-insensitive.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "music" => Some(Self::Music),
            "files" => Some(Self::Files),
            "voice" => Some(Self::Voice),
            _ => None,
        }
    }

    /// The lowercase wire tag.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Music => "music",
            Self::Files => "files",
            Self::Voice => "voice",
        }
    }
}

/// Derive the secret-store key for a SIP account's password from its username,
/// namespaced under `sip/`.
///
/// Mirrors the `vpn/` / `xcp/` namespacing in [`crate::ipc::secret_store`] (a
/// leader-managed, age-sealed secret — the SIP password is never embedded in the
/// plan). The username is sanitized to the SIP-userinfo charset so a stray
/// separator can't widen the key namespace. Pure + stable: this string IS the
/// on-disk / etcd key.
#[must_use]
pub fn sip_creds_ref(username: &str) -> String {
    let safe: String = username
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@'))
        .collect();
    format!("sip/{safe}")
}

/// An external SIP account the Voice service registers to (never spawned by us,
/// #36). The password is held in the secret store under [`SipAccount::creds_ref`];
/// this struct carries only the non-secret registration parameters.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SipAccount {
    /// The SIP registrar host the node sends `REGISTER` to (e.g. `sip.provider.net`).
    pub registrar: String,
    /// The SIP domain (the address-of-record domain; often the same as the registrar).
    pub domain: String,
    /// The SIP account username (the user part of the AOR).
    pub username: String,
    /// The secret-store key ([`sip_creds_ref`]) holding this account's password.
    pub creds_ref: String,
}

impl SipAccount {
    /// Build a SIP account for `username` on `registrar`/`domain`, deriving the
    /// [`sip_creds_ref`] for its password (never embedded here).
    #[must_use]
    pub fn new(registrar: &str, domain: &str, username: &str) -> Self {
        Self {
            registrar: registrar.to_string(),
            domain: domain.to_string(),
            username: username.to_string(),
            creds_ref: sip_creds_ref(username),
        }
    }
}

/// The request the front-ends pass: which service, plus the operator-supplied
/// external SIP account (Voice only).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceAddRequest {
    /// Which curated service to add.
    pub kind: ServiceKind,
    /// The external SIP account to register (Voice only). `None` for Music/Files,
    /// and for a Voice request with no account yet (→ the retryable
    /// [`ServiceAddPlan::VoiceNeedsAccount`] outcome).
    pub sip: Option<SipAccount>,
}

/// One lighthouse in the mesh, from the replicated peer roster, with its media
/// capability tag — the fact the Music planner selects a target from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LighthouseFact {
    /// The lighthouse's hostname (roster row key).
    pub hostname: String,
    /// Its Nebula overlay IP (`None` until it enrolls).
    pub overlay_ip: Option<String>,
    /// Whether it already carries [`mde_role::Capability::Media`] (a
    /// `Lighthouse_Media` node). `false` ⇒ a plain lighthouse the plan would
    /// **promote** to host the media server.
    pub media: bool,
}

/// The live facts [`gather`] reads off the mesh — the seam between the impure
/// roster read and the pure [`plan_service_add`] fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAddFacts {
    /// The mesh's lighthouses (media servers live on lighthouses, #19). Empty ⇒
    /// the Music branch resolves to the honest "no lighthouse yet" outcome.
    pub lighthouses: Vec<LighthouseFact>,
}

/// The selected media lighthouse the Music service is provisioned on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MediaLighthouseTarget {
    /// The target lighthouse's hostname.
    pub hostname: String,
    /// Its overlay IP (the `music.mesh` A-record this instance contributes).
    pub overlay_ip: Option<String>,
    /// `true` when it is already a `Lighthouse_Media`; `false` ⇒ this add
    /// **promotes** a plain lighthouse (tags it [`mde_role::Capability::Media`]).
    pub already_media: bool,
}

/// One ordered step of provisioning the Music service on the target lighthouse.
///
/// The steps *describe* the flow [`execute`] drives over the [`ServiceApply`] seam,
/// mirroring `install-helpers/setup-media-navidrome.sh`'s real boot-durable units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MusicStep {
    /// 1. Ensure the leader-managed `media-spaces` secret (DO Spaces S3 keys +
    ///    the Navidrome shared account) is sealed in the mesh secret store.
    SealSpacesCreds,
    /// 2. rclone-mount the shared DO Spaces bucket at the POSIX music path
    ///    (`mcnf-music-store.service`) — S3 presented as a filesystem (#35).
    MountBucket,
    /// 3. Run the rootless-podman Navidrome reading that path, Subsonic API on the
    ///    overlay `:4533` (`mcnf-navidrome.service`), hard-capped.
    SpawnNavidrome,
    /// 4. Publish the instance into the media registry + mesh-DNS `music.mesh`
    ///    (active-active A-records with failover, #21/#5).
    PublishMusicMesh,
}

impl MusicStep {
    /// The canonical ordered provisioning sequence.
    #[must_use]
    pub fn ordered() -> Vec<Self> {
        vec![
            Self::SealSpacesCreds,
            Self::MountBucket,
            Self::SpawnNavidrome,
            Self::PublishMusicMesh,
        ]
    }

    /// A one-line human description of the step.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::SealSpacesCreds => {
                "seal the DO Spaces S3 + Navidrome shared-account creds in the mesh secret store"
            }
            Self::MountBucket => "rclone-mount the shared DO Spaces bucket at the POSIX music path",
            Self::SpawnNavidrome => {
                "run the rootless-podman Navidrome (Subsonic API on the overlay :4533)"
            }
            Self::PublishMusicMesh => {
                "publish the instance into the registry + mesh-DNS music.mesh (active-active)"
            }
        }
    }
}

/// One ordered step of registering the Voice service to an external SIP provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum VoiceStep {
    /// 1. Resolve the `sip/<user>` secret (the external provider's password) from
    ///    the mesh secret store.
    LoadSipCreds,
    /// 2. Send a SIP `REGISTER` to the external registrar (never a PBX we run).
    RegisterToProvider,
    /// 3. Publish the registered voice line into the mesh so peers can reach it.
    PublishVoicePresence,
}

impl VoiceStep {
    /// The canonical ordered registration sequence.
    #[must_use]
    pub fn ordered() -> Vec<Self> {
        vec![
            Self::LoadSipCreds,
            Self::RegisterToProvider,
            Self::PublishVoicePresence,
        ]
    }

    /// A one-line human description of the step.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::LoadSipCreds => {
                "resolve the sip/<user> secret (the external provider's password)"
            }
            Self::RegisterToProvider => {
                "REGISTER to the external SIP registrar (no PBX is spawned)"
            }
            Self::PublishVoicePresence => "publish the registered voice line into the mesh",
        }
    }
}

/// Why the Music branch cannot provision right now — a real, retryable outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MusicBlock {
    /// The mesh has no lighthouse to host the media server (media servers live on
    /// lighthouses, #19) — spawn one first.
    NoLighthouse,
    /// Media/file-sharing lighthouse support is retired by the thin-lighthouse
    /// policy; the service must be hosted on a non-lighthouse node.
    LighthouseMediaRetired,
}

impl MusicBlock {
    /// What the operator does to unblock a retry.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::NoLighthouse => {
                "stand up a lighthouse first (`mackesd onboard spawn-lighthouse`), then retry — \
                 lighthouses hold the media servers"
            }
            Self::LighthouseMediaRetired => {
                "media/file-sharing lighthouses are retired — keep Music on a non-lighthouse host"
            }
        }
    }
}

impl std::fmt::Display for MusicBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoLighthouse => f.write_str("no lighthouse to host the media server"),
            Self::LighthouseMediaRetired => {
                f.write_str("media/file-sharing lighthouse support is retired")
            }
        }
    }
}

/// Why the Voice branch cannot register right now — a real, retryable outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum VoiceBlock {
    /// No external SIP account was supplied.
    NoAccount,
}

impl VoiceBlock {
    /// What the operator does to unblock a retry.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::NoAccount => {
                "supply the SIP account (--sip-registrar / --sip-user), then retry — Voice \
                 connects to an EXTERNAL SIP provider (no PBX VM is spawned)"
            }
        }
    }
}

impl std::fmt::Display for VoiceBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAccount => f.write_str("no SIP account supplied"),
        }
    }
}

/// A resolved service-add plan — the headless body the CLI prints and [`execute`]
/// drives. One distinct variant per branch (Music/Files/Voice), plus the honest
/// retryable blocked outcomes for the two provisioning branches.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ServiceAddPlan {
    /// Provision Navidrome on the selected media lighthouse reading DO Spaces,
    /// published at `music.mesh`.
    Music {
        /// The media lighthouse to host it (already-media, or a plain one promoted).
        target: MediaLighthouseTarget,
        /// The `media-spaces` secret-store key ([`media_spaces_creds_ref`]).
        creds_ref: String,
        /// The published server URL ([`music_mesh_server_url`]).
        server_url: String,
        /// The ordered provisioning steps.
        steps: Vec<MusicStep>,
    },
    /// Music is blocked — the mesh has no lighthouse yet. Carries the reused
    /// [`crate::onboard::spawn_lighthouse`] provision spec so the operator sees the
    /// lighthouse `onboard spawn-lighthouse` would stand up first. Retryable.
    MusicNeedsLighthouse {
        /// The lighthouse-provision spec (reused from
        /// [`crate::onboard::spawn_lighthouse::render_spec`]). Boxed to keep this
        /// variant size-balanced against the small ones.
        spec: Box<ProvisionSpec>,
        /// Why Music is blocked (and, via [`MusicBlock::hint`], the fix).
        reason: MusicBlock,
    },
    /// The historical Music-on-a-lighthouse path is retired. No side effect or
    /// retry is offered, so a thin lighthouse can never be promoted indirectly.
    MusicLighthouseRetired {
        /// The explicit policy reason for the refusal.
        reason: MusicBlock,
    },
    /// Files — nothing to provision: `mde-files` Send-To (P2P over the Bus) is
    /// already the path. An honest no-op outcome, not a faked service (#37/#20).
    FilesP2P {
        /// The P2P transport already in place ([`FILES_TRANSPORT`]).
        transport: &'static str,
    },
    /// Register to the external SIP provider (live registration gated on the
    /// operator's SIP creds).
    Voice {
        /// The external SIP account captured from the request. Boxed to keep this
        /// variant size-balanced.
        account: Box<SipAccount>,
        /// The ordered registration steps.
        steps: Vec<VoiceStep>,
    },
    /// Voice is blocked — no SIP account was supplied. Retryable.
    VoiceNeedsAccount {
        /// Why Voice is blocked (and, via [`VoiceBlock::hint`], the fix).
        reason: VoiceBlock,
    },
}

impl ServiceAddPlan {
    /// The ordered step descriptions this plan drives (empty for the no-op /
    /// blocked outcomes — nothing is spawned).
    #[must_use]
    pub fn steps(&self) -> Vec<&'static str> {
        match self {
            Self::Music { steps, .. } => steps.iter().map(|s| s.describe()).collect(),
            Self::Voice { steps, .. } => steps.iter().map(|s| s.describe()).collect(),
            Self::MusicNeedsLighthouse { .. }
            | Self::MusicLighthouseRetired { .. }
            | Self::FilesP2P { .. }
            | Self::VoiceNeedsAccount { .. } => Vec::new(),
        }
    }

    /// The reused lighthouse-provision spec, when this plan is Music-blocked on a
    /// missing lighthouse (so the CLI can print it in `--dry-run`).
    #[must_use]
    pub fn provision_spec(&self) -> Option<&ProvisionSpec> {
        match self {
            Self::MusicNeedsLighthouse { spec, .. } => Some(spec),
            _ => None,
        }
    }

    /// Whether a retry is available (the two blocked outcomes — the mesh keeps
    /// running and the operator retries after clearing the blocker).
    #[must_use]
    pub const fn retry_available(&self) -> bool {
        matches!(
            self,
            Self::MusicNeedsLighthouse { .. } | Self::VoiceNeedsAccount { .. }
        )
    }

    /// A one-line human summary (no trailing newline — the CLI wraps it in
    /// `println!`, mirroring the sibling verbs).
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Self::Music {
                target,
                server_url,
                steps,
                ..
            } => {
                let promote = if target.already_media {
                    "media-lighthouse"
                } else {
                    "lighthouse (promoted to media)"
                };
                format!(
                    "provision Navidrome on {} `{}` reading DO Spaces, published at {server_url}, \
                     in {} step(s)",
                    promote,
                    target.hostname,
                    steps.len()
                )
            }
            Self::MusicNeedsLighthouse { reason, .. } => format!(
                "Music unavailable ({reason}) — the mesh keeps running; {}",
                reason.hint()
            ),
            Self::MusicLighthouseRetired { reason } => {
                format!("Music unavailable ({reason}) — the mesh keeps running; {}", reason.hint())
            }
            Self::FilesP2P { transport } => format!(
                "Files is already peer-to-peer ({transport}) — nothing to provision (no VM/container)"
            ),
            Self::Voice { account, steps } => format!(
                "register voice account `{}@{}` to external SIP registrar `{}` in {} step(s)",
                account.username,
                account.domain,
                account.registrar,
                steps.len()
            ),
            Self::VoiceNeedsAccount { reason } => format!(
                "Voice unavailable ({reason}) — the mesh keeps running; {}",
                reason.hint()
            ),
        }
    }
}

/// Pure: select the media lighthouse to host the Music service.
///
/// Prefers a lighthouse **already** carrying [`mde_role::Capability::Media`] (an
/// existing `Lighthouse_Media`); otherwise falls back to the first lighthouse,
/// which the add would **promote** to media. `None` when the mesh has no
/// lighthouse at all (→ [`MusicBlock::NoLighthouse`]).
#[must_use]
pub fn select_media_lighthouse(lighthouses: &[LighthouseFact]) -> Option<&LighthouseFact> {
    lighthouses
        .iter()
        .find(|l| l.media)
        .or_else(|| lighthouses.first())
}

/// Pure: render the lighthouse-provision spec `onboard spawn-lighthouse` would
/// apply to stand up a media lighthouse — reused verbatim from
/// [`crate::onboard::spawn_lighthouse::render_spec`] (§6: Music does not re-invent
/// lighthouse/droplet provisioning). Cloud by default (a media lighthouse is a DO
/// droplet holding the Spaces-backed media volume, #27).
#[must_use]
pub fn render_media_lighthouse_spec() -> ProvisionSpec {
    spawn_lighthouse::render_spec(&SpawnTarget::default_cloud())
}

/// Pure fold: turn a [`ServiceAddRequest`] + gathered [`ServiceAddFacts`] into a
/// [`ServiceAddPlan`]. No I/O — fully unit-testable.
#[must_use]
pub fn plan_service_add(req: &ServiceAddRequest, facts: &ServiceAddFacts) -> ServiceAddPlan {
    match req.kind {
        ServiceKind::Music => plan_music(facts),
        ServiceKind::Files => ServiceAddPlan::FilesP2P {
            transport: FILES_TRANSPORT,
        },
        ServiceKind::Voice => plan_voice(req),
    }
}

/// Pure: the Music branch — select a media lighthouse (or resolve the honest
/// no-lighthouse outcome), naming the reused creds ref + published endpoint.
fn plan_music(_facts: &ServiceAddFacts) -> ServiceAddPlan {
    ServiceAddPlan::MusicLighthouseRetired {
        reason: MusicBlock::LighthouseMediaRetired,
    }
}

/// Pure: the Voice branch — capture the request's SIP account (or resolve the
/// honest no-account outcome).
fn plan_voice(req: &ServiceAddRequest) -> ServiceAddPlan {
    match &req.sip {
        Some(account) => ServiceAddPlan::Voice {
            account: Box::new(account.clone()),
            steps: VoiceStep::ordered(),
        },
        None => ServiceAddPlan::VoiceNeedsAccount {
            reason: VoiceBlock::NoAccount,
        },
    }
}

/// The reachable Navidrome instance a [`ServiceApply::provision_music`] stood up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicEndpoint {
    /// The lighthouse hosting the instance.
    pub host: String,
    /// The published server URL clients connect to (`music.mesh`).
    pub server_url: String,
}

/// A typed failure from the injectable [`ServiceApply`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// The live path is not runnable in this build/environment yet — it needs a
    /// real prerequisite (the container spawn + DO Spaces bucket, or the external
    /// SIP registration). Names the step + what is missing. §7-legal: a real method
    /// returning a real typed error, exactly as OW-7's / OW-8's seams do.
    IntegrationGated {
        /// Which seam step (`provision-music` / `register-voice`).
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

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { step, reason } => {
                write!(f, "{step}: integration-gated — {reason}")
            }
            Self::Failed { step, reason } => write!(f, "{step}: {reason}"),
        }
    }
}

impl std::error::Error for ServiceError {}

/// The injectable side-effect seam. Production is [`LiveServiceApply`]; tests use a
/// recording fake so the pure orchestration is exercised without a real container
/// spawn / SIP registration.
///
/// Files is deliberately absent — it is a pure P2P no-op ([`ServiceAddPlan::FilesP2P`]),
/// so [`execute`] resolves it without ever touching the seam.
pub trait ServiceApply {
    /// Provision the Navidrome media service on `target` reading the `media-spaces`
    /// creds (`creds_ref`), published at `server_url`.
    ///
    /// # Errors
    /// A [`ServiceError`] — `IntegrationGated` when the live provisioner can't run
    /// yet (the container spawn / DO Spaces bucket), else `Failed`.
    fn provision_music(
        &self,
        target: &MediaLighthouseTarget,
        creds_ref: &str,
        server_url: &str,
    ) -> Result<MusicEndpoint, ServiceError>;

    /// Register `account` to its external SIP provider.
    ///
    /// # Errors
    /// A [`ServiceError`] — `IntegrationGated` without the operator's SIP creds +
    /// live registrar reachability, else `Failed`.
    fn register_voice(&self, account: &SipAccount) -> Result<(), ServiceError>;
}

/// Production [`ServiceApply`] — the retired Navidrome-lighthouse path plus SIP
/// registration. Music provisioning always returns a typed refusal; the Voice
/// branch remains independently integration-gated.
///
/// Voice ([`register_voice`](Self::register_voice)) is an external-SIP
/// registration (not a remote push) and stays honestly integration-gated.
pub struct LiveServiceApply {
    /// The OW-15 day-2 remote-push transport. Default: [`BusApply`]; tests inject a
    /// recording fake to prove the wiring without a live round-trip.
    ///
    /// [`BusApply`]: crate::onboard::remote_push::BusApply
    remote_push: std::sync::Arc<dyn crate::onboard::remote_push::RemotePush + Send + Sync>,
    /// This authoring node's local secret store, when available — the source of the
    /// `media-spaces` plaintext that gets sealed into the target's bundle. `None`
    /// ⇒ only the Media-role pin is pushed (the live push delivers the seal).
    local_secrets: Option<crate::ipc::secret_store::SecretStore>,
}

impl Default for LiveServiceApply {
    fn default() -> Self {
        Self {
            remote_push: std::sync::Arc::new(crate::onboard::remote_push::BusApply),
            local_secrets: None,
        }
    }
}

impl LiveServiceApply {
    /// Inject the remote-push transport (tests use a recording fake). Production
    /// uses the honestly-gated [`BusApply`](crate::onboard::remote_push::BusApply).
    #[must_use]
    pub fn with_remote_push(
        mut self,
        transport: std::sync::Arc<dyn crate::onboard::remote_push::RemotePush + Send + Sync>,
    ) -> Self {
        self.remote_push = transport;
        self
    }

    /// Inject this node's local secret store (the source of the sealed
    /// `media-spaces` plaintext).
    #[must_use]
    pub fn with_local_secrets(mut self, store: crate::ipc::secret_store::SecretStore) -> Self {
        self.local_secrets = Some(store);
        self
    }
}

impl ServiceApply for LiveServiceApply {
    fn provision_music(
        &self,
        _target: &MediaLighthouseTarget,
        _creds_ref: &str,
        _server_url: &str,
    ) -> Result<MusicEndpoint, ServiceError> {
        return Err(ServiceError::Failed {
            step: "provision-music",
            reason: "media/file-sharing lighthouse support is retired; lighthouses are thin control-plane nodes".into(),
        });
    }

    fn register_voice(&self, account: &SipAccount) -> Result<(), ServiceError> {
        Err(ServiceError::IntegrationGated {
            step: "register-voice",
            reason: format!(
                "SIP `{}@{}` → needs the operator's SIP password (secret `{}`) and a live \
                 REGISTER to the external registrar `{}`; Voice connects to an EXTERNAL provider, \
                 never a PBX we spawn",
                account.username, account.domain, account.creds_ref, account.registrar
            ),
        })
    }
}

/// The result of an [`execute`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceAddOutcome {
    /// The Music service was provisioned + published.
    MusicProvisioned {
        /// The lighthouse hosting it.
        host: String,
        /// The published server URL.
        server_url: String,
    },
    /// Music was blocked (no lighthouse) — nothing provisioned; retry available.
    NoLighthouse {
        /// Why Music was blocked.
        reason: MusicBlock,
    },
    /// Music-on-lighthouse was refused by the thin-lighthouse policy.
    MusicLighthouseRetired {
        /// The explicit policy reason.
        reason: MusicBlock,
    },
    /// Files is already P2P — nothing was provisioned (the honest no-op).
    FilesAlreadyP2P {
        /// The P2P transport already in place.
        transport: &'static str,
    },
    /// The Voice account was registered to its external SIP provider.
    VoiceRegistered {
        /// The external registrar it registered to.
        registrar: String,
    },
    /// Voice was blocked (no SIP account) — nothing registered; retry available.
    NoSipAccount {
        /// Why Voice was blocked.
        reason: VoiceBlock,
    },
}

impl ServiceAddOutcome {
    /// A one-line human summary (no trailing newline).
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Self::MusicProvisioned { host, server_url } => {
                format!("Music provisioned on `{host}`, published at {server_url}")
            }
            Self::NoLighthouse { reason } => {
                format!("no-op — Music blocked ({reason}); retry available")
            }
            Self::MusicLighthouseRetired { reason } => {
                format!("no-op — Music refused ({reason}); lighthouses remain thin")
            }
            Self::FilesAlreadyP2P { transport } => {
                format!("no-op — Files is already peer-to-peer ({transport})")
            }
            Self::VoiceRegistered { registrar } => {
                format!("Voice registered to external SIP registrar `{registrar}`")
            }
            Self::NoSipAccount { reason } => {
                format!("no-op — Voice blocked ({reason}); retry available")
            }
        }
    }
}

/// Pure orchestration over the [`ServiceApply`] seam.
///
/// A provisioning plan (Music / Voice) drives the matching seam call; a blocked or
/// no-op plan (MusicNeedsLighthouse / MusicLighthouseRetired / FilesP2P /
/// VoiceNeedsAccount) short-circuits
/// to its retryable / no-op outcome **without any seam calls** — a P2P Files add
/// never touches live infra, honoring "services never block the working network".
///
/// This is the tested orchestration the fake pins; the real side effects live
/// entirely in the injected `apply`.
///
/// # Errors
/// Propagates the first [`ServiceError`] the seam returns.
pub fn execute(
    plan: &ServiceAddPlan,
    apply: &dyn ServiceApply,
) -> Result<ServiceAddOutcome, ServiceError> {
    match plan {
        ServiceAddPlan::Music { .. } => Ok(ServiceAddOutcome::MusicLighthouseRetired {
            reason: MusicBlock::LighthouseMediaRetired,
        }),
        ServiceAddPlan::MusicNeedsLighthouse { reason, .. } => {
            Ok(ServiceAddOutcome::NoLighthouse { reason: *reason })
        }
        ServiceAddPlan::MusicLighthouseRetired { reason } => {
            Ok(ServiceAddOutcome::MusicLighthouseRetired { reason: *reason })
        }
        ServiceAddPlan::FilesP2P { transport } => {
            Ok(ServiceAddOutcome::FilesAlreadyP2P { transport })
        }
        ServiceAddPlan::Voice { account, .. } => {
            apply.register_voice(account)?;
            Ok(ServiceAddOutcome::VoiceRegistered {
                registrar: account.registrar.clone(),
            })
        }
        ServiceAddPlan::VoiceNeedsAccount { reason } => {
            Ok(ServiceAddOutcome::NoSipAccount { reason: *reason })
        }
    }
}

/// Impure probe shell: gather the mesh's lighthouse set (with each one's media
/// capability tag) off the replicated peer roster.
///
/// Best-effort — a missing roster degrades to an empty lighthouse list rather than
/// erroring, so the pure [`plan_service_add`] fold always runs and produces the
/// real verdict (`MusicNeedsLighthouse` when no lighthouse exists). Reuses the
/// PEERVER-1 own-row directory ([`mackes_mesh_types::peers`]) — the same roster
/// [`crate::onboard::mesh_dns`] folds — so the media-lighthouse discovery needs no
/// separate probe (each node stamps its `role` + `media` tag on the heartbeat).
#[must_use]
pub fn gather(workgroup_root: &std::path::Path) -> ServiceAddFacts {
    use mackes_mesh_types::peers;
    let roster = peers::read_peers(&peers::peers_dir(workgroup_root));
    let lighthouses = roster
        .into_iter()
        .filter(|p| p.role.as_deref() == Some("lighthouse"))
        .map(|p| LighthouseFact {
            hostname: p.hostname,
            overlay_ip: p.overlay_ip,
            media: p.media,
        })
        .collect();
    ServiceAddFacts { lighthouses }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn lh(host: &str, ip: Option<&str>, media: bool) -> LighthouseFact {
        LighthouseFact {
            hostname: host.to_string(),
            overlay_ip: ip.map(str::to_string),
            media,
        }
    }

    fn facts(lighthouses: Vec<LighthouseFact>) -> ServiceAddFacts {
        ServiceAddFacts { lighthouses }
    }

    fn req(kind: ServiceKind, sip: Option<SipAccount>) -> ServiceAddRequest {
        ServiceAddRequest { kind, sip }
    }

    // ── ServiceKind parse ──

    #[test]
    fn service_kind_parses_the_three_curated_services() {
        assert_eq!(ServiceKind::parse("music"), Some(ServiceKind::Music));
        assert_eq!(ServiceKind::parse("FILES"), Some(ServiceKind::Files));
        assert_eq!(ServiceKind::parse("  Voice "), Some(ServiceKind::Voice));
        assert_eq!(ServiceKind::parse("desktop"), None);
        // Round-trips through as_str.
        for k in [ServiceKind::Music, ServiceKind::Files, ServiceKind::Voice] {
            assert_eq!(ServiceKind::parse(k.as_str()), Some(k));
        }
    }

    // ── Music branch ──

    #[test]
    fn music_on_any_lighthouse_is_refused_by_the_thin_policy() {
        let plan = plan_service_add(
            &req(ServiceKind::Music, None),
            &facts(vec![
                lh("lh-plain", Some("10.42.0.1"), false),
                lh("lh-media", Some("10.42.0.2"), true),
            ]),
        );
        assert!(matches!(
            plan,
            ServiceAddPlan::MusicLighthouseRetired {
                reason: MusicBlock::LighthouseMediaRetired
            }
        ));
        assert!(!plan.retry_available());
        assert!(plan.human().contains("lighthouses are retired"));
    }

    #[test]
    fn music_refusal_does_not_depend_on_lighthouse_facts() {
        let plan = plan_service_add(
            &req(ServiceKind::Music, None),
            &facts(vec![
                lh("lh-a", Some("10.42.0.1"), false),
                lh("lh-b", Some("10.42.0.2"), false),
            ]),
        );
        assert!(matches!(
            plan,
            ServiceAddPlan::MusicLighthouseRetired {
                reason: MusicBlock::LighthouseMediaRetired
            }
        ));
    }

    #[test]
    fn music_without_a_lighthouse_is_a_retryable_no_lighthouse_outcome() {
        // No lighthouse still cannot trigger provisioning or lighthouse creation.
        let plan = plan_service_add(&req(ServiceKind::Music, None), &facts(vec![]));
        assert!(matches!(
            plan,
            ServiceAddPlan::MusicLighthouseRetired {
                reason: MusicBlock::LighthouseMediaRetired
            }
        ));
        assert!(!plan.retry_available());
        assert!(plan.steps().is_empty());
        assert!(plan.provision_spec().is_none());
    }

    #[test]
    fn select_media_lighthouse_prefers_media_then_first_then_none() {
        // Media preferred even when it isn't first.
        let mixed = [lh("a", None, false), lh("b", None, true)];
        assert_eq!(select_media_lighthouse(&mixed).unwrap().hostname, "b");
        // No media → the first lighthouse.
        let plain = [lh("a", None, false), lh("b", None, false)];
        assert_eq!(select_media_lighthouse(&plain).unwrap().hostname, "a");
        // Empty → None.
        assert!(select_media_lighthouse(&[]).is_none());
    }

    #[test]
    fn render_media_lighthouse_spec_reuses_spawn_lighthouse_render_spec() {
        // §6: the media-lighthouse provisioning is the spawn-lighthouse cloud-init,
        // byte-identical to the sibling verb's renderer (no re-invention).
        let spec = render_media_lighthouse_spec();
        assert_eq!(
            spec,
            spawn_lighthouse::render_spec(&SpawnTarget::default_cloud())
        );
        assert!(spec.document().contains(spawn_lighthouse::REPO_BASEURL));
    }

    #[test]
    fn music_steps_are_ordered_and_described() {
        let steps = MusicStep::ordered();
        assert_eq!(
            steps,
            vec![
                MusicStep::SealSpacesCreds,
                MusicStep::MountBucket,
                MusicStep::SpawnNavidrome,
                MusicStep::PublishMusicMesh,
            ]
        );
        // Seal the creds before mounting the bucket that needs them.
        let seal = steps.iter().position(|s| *s == MusicStep::SealSpacesCreds);
        let mount = steps.iter().position(|s| *s == MusicStep::MountBucket);
        assert!(seal < mount, "seal creds before mounting the bucket");
        assert!(steps.iter().all(|s| !s.describe().is_empty()));
    }

    // ── Files branch ──

    #[test]
    fn files_is_an_honest_p2p_no_op_never_a_spawned_service() {
        let plan = plan_service_add(&req(ServiceKind::Files, None), &facts(vec![]));
        assert_eq!(
            plan,
            ServiceAddPlan::FilesP2P {
                transport: FILES_TRANSPORT
            }
        );
        // Nothing to spawn — no steps, no provision spec, and it names the P2P path.
        assert!(plan.steps().is_empty());
        assert!(plan.provision_spec().is_none());
        assert!(!plan.retry_available());
        assert!(plan.human().contains("peer-to-peer"));
        assert!(plan.human().contains("no VM/container"));
    }

    // ── Voice branch ──

    #[test]
    fn voice_with_an_account_plans_an_external_sip_registration() {
        let acct = SipAccount::new("sip.provider.net", "provider.net", "alice");
        let plan = plan_service_add(&req(ServiceKind::Voice, Some(acct.clone())), &facts(vec![]));
        let ServiceAddPlan::Voice { account, steps } = &plan else {
            panic!("expected a Voice plan, got {plan:?}");
        };
        assert_eq!(**account, acct);
        // The password rides a secret-store ref, never embedded in the plan.
        assert_eq!(account.creds_ref, "sip/alice");
        assert_eq!(steps, &VoiceStep::ordered());
        assert!(!plan.retry_available());
        assert!(plan.human().contains("external SIP registrar"));
        assert!(plan.human().contains("alice@provider.net"));
    }

    #[test]
    fn voice_without_an_account_is_a_retryable_no_account_outcome() {
        let plan = plan_service_add(&req(ServiceKind::Voice, None), &facts(vec![]));
        assert_eq!(
            plan,
            ServiceAddPlan::VoiceNeedsAccount {
                reason: VoiceBlock::NoAccount
            }
        );
        assert!(plan.retry_available());
        assert!(plan.steps().is_empty());
        assert!(plan.human().contains("--sip-registrar"));
    }

    #[test]
    fn sip_creds_ref_namespaces_and_sanitizes() {
        assert_eq!(sip_creds_ref("alice"), "sip/alice");
        assert_eq!(sip_creds_ref("  bob@corp "), "sip/bob@corp");
        // Stray separators that could widen the namespace (the `/` and the space)
        // are dropped; the remaining userinfo chars survive.
        assert_eq!(sip_creds_ref("a/b c"), "sip/abc");
    }

    #[test]
    fn voice_steps_are_ordered_and_described() {
        let steps = VoiceStep::ordered();
        assert_eq!(
            steps,
            vec![
                VoiceStep::LoadSipCreds,
                VoiceStep::RegisterToProvider,
                VoiceStep::PublishVoicePresence,
            ]
        );
        // Load the creds before REGISTERing with them.
        let load = steps.iter().position(|s| *s == VoiceStep::LoadSipCreds);
        let reg = steps
            .iter()
            .position(|s| *s == VoiceStep::RegisterToProvider);
        assert!(load < reg, "load creds before registering");
        assert!(steps.iter().all(|s| !s.describe().is_empty()));
    }

    // ── §6 reuse assertions: the media primitives are shared, not re-derived ──

    #[test]
    fn music_endpoint_pins_the_shared_music_mesh_url() {
        // The published server URL is mesh_media's single-sourced music.mesh:4533.
        assert_eq!(music_mesh_server_url(), "http://music.mesh:4533");
        assert_eq!(
            music_mesh_server_url(),
            crate::mesh_media::music_mesh_server_url()
        );
    }

    #[cfg(feature = "async-services")]
    #[test]
    fn media_creds_ref_is_the_shared_secret_store_key() {
        // §6: reused verbatim from ipc::secret_store (single source of truth).
        assert_eq!(
            media_spaces_creds_ref(),
            crate::ipc::secret_store::media_spaces_creds_ref()
        );
        assert_eq!(media_spaces_creds_ref(), "media-spaces");
    }

    #[test]
    fn media_capability_is_retired_for_lighthouses() {
        use mde_role::{Capability, Role};
        assert!(!Capability::Media.applies_to(Role::Lighthouse));
        assert!(!Capability::Media.applies_to(Role::Workstation));
    }

    // ── execute over the seam (recording fake) ──

    /// Recording [`ServiceApply`] fake: records the ordered calls + what it saw so
    /// the pure orchestration is asserted without a real container / SIP registration.
    struct FakeApply {
        calls: RefCell<Vec<&'static str>>,
        seen_target: RefCell<Option<MediaLighthouseTarget>>,
        seen_creds: RefCell<Option<String>>,
        seen_account: RefCell<Option<SipAccount>>,
    }

    impl FakeApply {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                seen_target: RefCell::new(None),
                seen_creds: RefCell::new(None),
                seen_account: RefCell::new(None),
            }
        }
    }

    impl ServiceApply for FakeApply {
        fn provision_music(
            &self,
            target: &MediaLighthouseTarget,
            creds_ref: &str,
            server_url: &str,
        ) -> Result<MusicEndpoint, ServiceError> {
            self.calls.borrow_mut().push("provision_music");
            *self.seen_target.borrow_mut() = Some(target.clone());
            *self.seen_creds.borrow_mut() = Some(creds_ref.to_string());
            Ok(MusicEndpoint {
                host: target.hostname.clone(),
                server_url: server_url.to_string(),
            })
        }
        fn register_voice(&self, account: &SipAccount) -> Result<(), ServiceError> {
            self.calls.borrow_mut().push("register_voice");
            *self.seen_account.borrow_mut() = Some(account.clone());
            Ok(())
        }
    }

    #[test]
    fn execute_music_refuses_lighthouse_provision_without_seam_calls() {
        let plan = plan_service_add(
            &req(ServiceKind::Music, None),
            &facts(vec![lh("lh-media", Some("10.42.0.2"), true)]),
        );
        let apply = FakeApply::new();
        let outcome = execute(&plan, &apply).expect("execute");
        assert_eq!(
            outcome,
            ServiceAddOutcome::MusicLighthouseRetired {
                reason: MusicBlock::LighthouseMediaRetired
            }
        );
        assert!(apply.calls.borrow().is_empty());
    }

    #[test]
    fn execute_voice_drives_register_voice() {
        let acct = SipAccount::new("sip.provider.net", "provider.net", "alice");
        let plan = plan_service_add(&req(ServiceKind::Voice, Some(acct)), &facts(vec![]));
        let apply = FakeApply::new();
        let outcome = execute(&plan, &apply).expect("execute");
        assert_eq!(
            outcome,
            ServiceAddOutcome::VoiceRegistered {
                registrar: "sip.provider.net".into()
            }
        );
        assert_eq!(*apply.calls.borrow(), vec!["register_voice"]);
        assert_eq!(
            apply
                .seen_account
                .borrow()
                .as_ref()
                .map(|a| a.username.clone()),
            Some("alice".into())
        );
    }

    #[test]
    fn execute_files_makes_no_seam_calls() {
        // A P2P Files add never touches live infra (never blocks the network).
        let plan = plan_service_add(&req(ServiceKind::Files, None), &facts(vec![]));
        let apply = FakeApply::new();
        let outcome = execute(&plan, &apply).expect("execute");
        assert_eq!(
            outcome,
            ServiceAddOutcome::FilesAlreadyP2P {
                transport: FILES_TRANSPORT
            }
        );
        assert!(
            apply.calls.borrow().is_empty(),
            "no seam calls for P2P files"
        );
    }

    #[test]
    fn execute_blocked_plans_short_circuit_without_seam_calls() {
        let apply = FakeApply::new();
        // Retired lighthouse Music path → no seam call.
        let m = plan_service_add(&req(ServiceKind::Music, None), &facts(vec![]));
        assert_eq!(
            execute(&m, &apply).expect("execute"),
            ServiceAddOutcome::MusicLighthouseRetired {
                reason: MusicBlock::LighthouseMediaRetired
            }
        );
        // Voice-blocked → NoSipAccount, no seam call.
        let v = plan_service_add(&req(ServiceKind::Voice, None), &facts(vec![]));
        assert_eq!(
            execute(&v, &apply).expect("execute"),
            ServiceAddOutcome::NoSipAccount {
                reason: VoiceBlock::NoAccount
            }
        );
        assert!(
            apply.calls.borrow().is_empty(),
            "no seam calls on blocked plans"
        );
    }

    // ── the production seam is integration-gated, never a fake success ──

    #[test]
    fn live_provision_music_refuses_the_retired_lighthouse_path() {
        let apply = LiveServiceApply::default();
        let target = MediaLighthouseTarget {
            hostname: "lh-media".into(),
            overlay_ip: Some("10.42.0.2".into()),
            already_media: true,
        };
        let err = apply
            .provision_music(&target, &media_spaces_creds_ref(), &music_mesh_server_url())
            .expect_err("live music provision must not fake success");
        assert!(
            matches!(err, ServiceError::Failed { step: "provision-music", ref reason }
            if reason.contains("media/file-sharing lighthouse support is retired"))
        );
    }

    #[test]
    fn live_register_voice_is_integration_gated_not_fake_success() {
        let apply = LiveServiceApply::default();
        let acct = SipAccount::new("sip.provider.net", "provider.net", "alice");
        let err = apply
            .register_voice(&acct)
            .expect_err("live SIP register must not fake success");
        match err {
            ServiceError::IntegrationGated { step, reason } => {
                assert_eq!(step, "register-voice");
                assert!(
                    reason.contains("sip/alice"),
                    "names the creds ref: {reason}"
                );
                assert!(
                    reason.contains("EXTERNAL"),
                    "external provider, not a PBX: {reason}"
                );
                assert!(
                    reason.contains("sip.provider.net"),
                    "names the registrar: {reason}"
                );
            }
            ServiceError::Failed { .. } => panic!("expected an integration-gated error"),
        }
    }

    #[test]
    fn execute_preserves_the_retired_lighthouse_refusal() {
        // Through the live seam, execute keeps the refusal side-effect free.
        let plan = plan_service_add(
            &req(ServiceKind::Music, None),
            &facts(vec![lh("lh-media", None, true)]),
        );
        assert_eq!(
            execute(&plan, &LiveServiceApply::default()).expect("retired path is a safe no-op"),
            ServiceAddOutcome::MusicLighthouseRetired {
                reason: MusicBlock::LighthouseMediaRetired
            }
        );
    }

    #[test]
    fn provision_music_refuses_the_retired_lighthouse_path() {
        let apply = LiveServiceApply::default();
        let target = MediaLighthouseTarget {
            hostname: "lh-media".into(),
            overlay_ip: Some("10.42.0.2".into()),
            already_media: false,
        };
        let err = apply
            .provision_music(&target, "media-spaces", &music_mesh_server_url())
            .expect_err("thin lighthouses must never receive a media push");
        assert!(err
            .to_string()
            .contains("media/file-sharing lighthouse support is retired"));
    }

    // ── serde ──

    #[test]
    fn service_add_request_round_trips_through_serde() {
        let request = ServiceAddRequest {
            kind: ServiceKind::Voice,
            sip: Some(SipAccount::new("sip.provider.net", "provider.net", "alice")),
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let back: ServiceAddRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(request, back);
        // ServiceKind serializes lowercase.
        assert!(json.contains("\"voice\""));
    }
}
