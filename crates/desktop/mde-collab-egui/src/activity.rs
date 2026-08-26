//! Activity mode — the action-oriented chronological feed from the
//! [`ActivityFeed`](mde_collab_types::ActivityFeed) projection, with band
//! filters. In the Activity app it prefers the cross-space feed; when routed as
//! a selected channel body it falls back to that channel's feed. There is
//! deliberately **no** competing global search box here (spec §2): the rail is
//! the space selector and the chips are the only filter.

use mde_egui::egui;
use mde_egui::style::TypographyRole;
use mde_egui::widgets;
use mde_egui::Style;

use std::ops::Range;
use std::path::{Path, PathBuf};

use mde_collab_types::{ActivityEntry, AlertInbox, Severity, SpaceId};

use crate::icons::CommsHoverExt;
use crate::{icons, relative_age, ActivityFilter, CommunicationsSurface, MeshTeamsApp};

/// Bus topic the Voice panel publishes to force a (re-)provision pass.
pub const VOICE_PROVISION_TOPIC: &str = "action/voice/provision";
/// Bus topic the Voice panel publishes to route an existing DID.
pub const VOICE_DID_ROUTE_TOPIC: &str = "action/voice/did-route";
/// Bus topic the Voice panel publishes to set a node's failover policy.
pub const VOICE_FAILOVER_TOPIC: &str = "action/voice/failover";
/// Bus topic the Voice panel publishes to apply the fleet shared-outbound.
pub const VOICE_SHARED_CONFIG_TOPIC: &str = "action/voice/shared-config";
/// Bus topic the SIP-gateway set verb is published on.
pub const VOIP_SET_GATEWAY_TOPIC: &str = "action/voip/set-gateway";
/// Bus topic the SIP-gateway get verb is published on.
pub const VOIP_GET_GATEWAY_TOPIC: &str = "action/voip/get-gateway";
/// Bus topic the SIP-gateway clear verb is published on.
pub const VOIP_CLEAR_GATEWAY_TOPIC: &str = "action/voip/clear-gateway";

/// Master-account DID inventory published by `voice_provision`. Body is a
/// JSON array of [`VoiceDid`]; the worker writes `[]` when unprovisioned.
const VOICE_DIDS_STATE_TOPIC: &str = "state/voice-dids";
/// Leader-held shared-outbound mirror. Body is [`VoiceSharedOutbound`] or `null`.
const VOICE_SHARED_STATE_TOPIC: &str = "state/voice-shared";
/// Fleet cutover status. Body is [`VoiceCutoverStatus`].
const VOICE_CUTOVER_STATE_TOPIC: &str = "state/voice-cutover";
/// Per-node fleet-board prefix. One [`VoiceNodeProjection`] per `state/voice/<node>`.
const VOICE_STATE_PREFIX: &str = "state/voice/";
/// Observational HUD snapshot suffix — never a fleet-board row.
const VOICE_HUD_STATUS_SUFFIX: &str = "status";
/// Bound the fleet board so a hostile retained prefix cannot stall refresh.
const MAX_VOICE_NODE_ROWS: usize = 512;

/// Canonical workgroup path the responder writes: `<workgroup_root>/voip/gateway.toml`.
/// Activity hydrates from this file in place; it never rewrites it.
#[must_use]
pub fn workgroup_gateway_toml_path(workgroup_root: impl AsRef<Path>) -> PathBuf {
    workgroup_root.as_ref().join("voip").join("gateway.toml")
}

/// Retained fleet-voice and SIP-gateway projections the Activity admin panels
/// render. Every method defaults to empty so existing [`CollabData`] callers of
/// [`CommunicationsSurface::activity_body`] keep compiling without implementing
/// this trait.
pub trait ActivityAdminData {
    /// Per-node `state/voice/<node>` fleet-board rows.
    fn voice_nodes(&self) -> &[VoiceNodeProjection] {
        &[]
    }

    /// Master-account DID inventory from `state/voice-dids`.
    fn voice_dids(&self) -> &[VoiceDid] {
        &[]
    }

    /// Leader-held shared-outbound mirrored on `state/voice-shared`.
    fn voice_shared(&self) -> Option<&VoiceSharedOutbound> {
        None
    }

    /// Fleet cutover status mirrored on `state/voice-cutover`.
    fn voice_cutover(&self) -> Option<&VoiceCutoverStatus> {
        None
    }

    /// Redacted `get-gateway` readout. `password` is always empty.
    fn gateway(&self) -> Option<&GatewayReadout> {
        None
    }
}

impl ActivityAdminData for () {}

/// Owned snapshot a shell (or test) can fill and pass into Activity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityAdminSnapshot {
    /// Per-node fleet-board rows.
    pub voice_nodes: Vec<VoiceNodeProjection>,
    /// Master-account DID inventory.
    pub voice_dids: Vec<VoiceDid>,
    /// Leader-held shared-outbound, when lifted.
    pub voice_shared: Option<VoiceSharedOutbound>,
    /// Fleet cutover status, when projected.
    pub voice_cutover: Option<VoiceCutoverStatus>,
    /// Redacted gateway readout, when projected.
    pub gateway: Option<GatewayReadout>,
}

impl ActivityAdminData for ActivityAdminSnapshot {
    fn voice_nodes(&self) -> &[VoiceNodeProjection] {
        &self.voice_nodes
    }

    fn voice_dids(&self) -> &[VoiceDid] {
        &self.voice_dids
    }

    fn voice_shared(&self) -> Option<&VoiceSharedOutbound> {
        self.voice_shared.as_ref()
    }

    fn voice_cutover(&self) -> Option<&VoiceCutoverStatus> {
        self.voice_cutover.as_ref()
    }

    fn gateway(&self) -> Option<&GatewayReadout> {
        self.gateway.as_ref()
    }
}

impl ActivityAdminSnapshot {
    /// Load the redacted `get-gateway` projection from an in-place workgroup
    /// `gateway.toml`, falling back to a retained get-gateway body. The on-disk
    /// file is not rewritten and the password is never kept.
    pub fn hydrate_gateway(
        &mut self,
        workgroup_root: Option<&Path>,
        retained_get: Option<&str>,
    ) -> &GatewayReadout {
        self.gateway
            .insert(hydrate_gateway_readout(workgroup_root, retained_get))
    }

    /// Fold the latest retained `state/voice-dids`, `state/voice-shared`,
    /// `state/voice-cutover`, and `state/voice/<node>` bodies into this
    /// snapshot. Call on every refresh.
    ///
    /// Missing topics, the worker's honest empty `[]` / `null` bodies, and
    /// unparseable payloads stay empty. `state/voice/status` and the
    /// fleet-wide topics never become invented node rows.
    pub fn hydrate_voice(
        &mut self,
        retained_dids: Option<&str>,
        retained_shared: Option<&str>,
        retained_cutover: Option<&str>,
        retained_nodes: &[(&str, &str)],
    ) -> &Self {
        self.voice_dids = parse_retained_voice_dids(retained_dids);
        self.voice_shared = parse_retained_voice_shared(retained_shared);
        self.voice_cutover = parse_retained_voice_cutover(retained_cutover);
        self.voice_nodes = parse_retained_voice_nodes(retained_nodes);
        self
    }
}

/// One node's `state/voice/<node>` fleet-board row. Field names match
/// `NodeVoiceState` so a shell can deserialize the topic unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceNodeProjection {
    /// Topic suffix / board key.
    pub node_id: String,
    /// Hostname the sub-account username derives from.
    pub hostname: String,
    /// Sub-account username; empty until provisioned.
    pub username: String,
    /// Callable `<username>@<realm>` SIP address.
    pub sip_uri: String,
    /// Provisioning / registration state.
    pub reg_state: VoiceRegState,
    /// Master DIDs currently routed to this node.
    pub routed_dids: Vec<String>,
    /// Applied offline-inbound failover policy, if any.
    pub failover: Option<VoiceFailoverPolicy>,
    /// When this row was produced (epoch seconds).
    pub updated_at_s: u64,
}

impl VoiceNodeProjection {
    /// Whether this node has a sealed inbound sub-account.
    ///
    /// Matches `voice_provision::is_reprovisioned`: only `Registered` or
    /// `Unregistered` count. A derived username in `Provisioning` is the
    /// worker's honest "awaiting the master key / live Vitelity" row — not a
    /// provisioned account. `Error` is a failed pass, never a fake online.
    #[must_use]
    pub fn is_provisioned(&self) -> bool {
        !self.username.trim().is_empty()
            && matches!(
                self.reg_state,
                VoiceRegState::Registered | VoiceRegState::Unregistered
            )
    }
}

/// A node's provisioning / registration state. Tagged `state` + kebab-case on
/// the wire (`RegState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceRegState {
    /// Active REGISTER.
    Registered,
    /// Provisioned, awaiting REGISTER.
    Unregistered,
    /// Provisioning action in flight.
    Provisioning,
    /// Honest failure with the provider reason.
    Error {
        /// Operator-readable failure detail.
        reason: String,
    },
}

impl VoiceRegState {
    /// Wire `state` tag (`registered`, `unregistered`, `provisioning`, `error`).
    #[must_use]
    pub const fn wire_tag(&self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Unregistered => "unregistered",
            Self::Provisioning => "provisioning",
            Self::Error { .. } => "error",
        }
    }
}

/// An existing master-account DID. Field names match the Vitelity `Did` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceDid {
    /// DID digits as the provider returns them.
    pub number: String,
    /// Current routing target (sub-account username), or `None` for the main line.
    pub routed_to: Option<String>,
}

/// Leader-held shared-outbound. Field names match `SharedOutboundConfig` /
/// `SharedConfigRequest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceSharedOutbound {
    /// Shared caller-ID all outbound PSTN presents.
    pub caller_id: String,
    /// Shared outbound trunk label / account.
    pub outbound_trunk: String,
}

/// Fleet cutover status from `state/voice-cutover`. Field names match
/// `CutoverStatus`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceCutoverStatus {
    /// Single fleet-wide migration phase.
    pub phase: VoiceCutoverPhase,
    /// Enrolled nodes total.
    pub total_nodes: usize,
    /// Nodes already on the split model.
    pub reprovisioned: usize,
    /// Hostnames (or node ids) still on the legacy model.
    pub pending_nodes: Vec<String>,
    /// Whether the shared-outbound config is lifted.
    pub shared_outbound_lifted: bool,
    /// When this status was produced (epoch seconds).
    pub updated_at_s: u64,
}

/// Fleet migration phase. Wire names are kebab-case (`CutoverPhase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceCutoverPhase {
    /// Still on the pre-split single-account model.
    Legacy,
    /// Shared-outbound lifted; no node has crossed yet.
    LiftedSharedOutbound,
    /// Some nodes have crossed; others remain.
    NodesReprovisioning,
    /// Every enrolled node is on the split model.
    CutoverComplete,
}

impl VoiceCutoverPhase {
    /// Operator headline copied from the worker's `CutoverPhase::headline`.
    #[must_use]
    pub const fn headline(self) -> &'static str {
        match self {
            Self::Legacy => {
                "Legacy single-account model — apply the fleet shared-outbound to begin"
            }
            Self::LiftedSharedOutbound => {
                "Shared-outbound lifted — outbound alive; reprovisioning nodes onto the split model"
            }
            Self::NodesReprovisioning => {
                "Cutover in progress — some nodes still on the legacy model"
            }
            Self::CutoverComplete => "Cutover complete — every node on the split model",
        }
    }

    /// Wire name (`legacy`, `lifted-shared-outbound`, …).
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::LiftedSharedOutbound => "lifted-shared-outbound",
            Self::NodesReprovisioning => "nodes-reprovisioning",
            Self::CutoverComplete => "cutover-complete",
        }
    }
}

/// Offline-inbound failover policy. Variants match `FailoverPolicy` so the
/// `action/voice/failover` body serializes unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceFailoverPolicy {
    /// Send unanswered/offline calls to voicemail.
    Voicemail,
    /// Forward to a PSTN number when the node is unreachable.
    Forward {
        /// The E.164 number to forward to.
        number: String,
    },
    /// No failover — the caller hears unavailable.
    None,
}

impl VoiceFailoverPolicy {
    /// Compact operator label for the fleet board.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Voicemail => "Voicemail".to_owned(),
            Self::Forward { number } => format!("Forward {number}"),
            Self::None => "None".to_owned(),
        }
    }
}

/// Typed Voice admin verb the shell drains onto `action/voice/*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceAdminCommand {
    /// Force an immediate provision/reconcile pass.
    Provision,
    /// Route an existing DID. `node_id == None` unroutes to the main line.
    DidRoute {
        /// Existing master-account DID.
        did: String,
        /// Target node id, or `None` for the main account.
        node_id: Option<String>,
    },
    /// Set a node's offline-inbound failover policy.
    Failover {
        /// Target node id.
        node_id: String,
        /// Desired policy.
        policy: VoiceFailoverPolicy,
    },
    /// Apply the leader-held shared-outbound config.
    SharedConfig {
        /// Shared caller-ID.
        caller_id: String,
        /// Shared outbound trunk label.
        outbound_trunk: String,
    },
    /// Continue the hard cutover (drains onto the provision topic).
    Cutover,
}

impl VoiceAdminCommand {
    /// Canonical Bus topic this command publishes on.
    #[must_use]
    pub const fn topic(&self) -> &'static str {
        match self {
            Self::Provision | Self::Cutover => VOICE_PROVISION_TOPIC,
            Self::DidRoute { .. } => VOICE_DID_ROUTE_TOPIC,
            Self::Failover { .. } => VOICE_FAILOVER_TOPIC,
            Self::SharedConfig { .. } => VOICE_SHARED_CONFIG_TOPIC,
        }
    }

    /// JSON body matching the worker request structs, ready to publish unchanged.
    #[must_use]
    pub fn json_body(&self) -> String {
        match self {
            Self::Provision | Self::Cutover => "{}".to_owned(),
            Self::DidRoute { did, node_id } => match node_id {
                Some(node) => format!(
                    "{{\"did\":{},\"node_id\":{}}}",
                    json_string(did),
                    json_string(node)
                ),
                None => format!("{{\"did\":{},\"node_id\":null}}", json_string(did)),
            },
            Self::Failover { node_id, policy } => format!(
                "{{\"node_id\":{},\"policy\":{}}}",
                json_string(node_id),
                failover_policy_json(policy)
            ),
            Self::SharedConfig {
                caller_id,
                outbound_trunk,
            } => format!(
                "{{\"caller_id\":{},\"outbound_trunk\":{}}}",
                json_string(caller_id),
                json_string(outbound_trunk)
            ),
        }
    }
}

/// Caller-drained Voice admin intent queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceAdminSink {
    queued: Vec<VoiceAdminCommand>,
}

impl VoiceAdminSink {
    /// A fresh, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `command` for the shell to publish.
    pub fn emit(&mut self, command: VoiceAdminCommand) {
        self.queued.push(command);
    }

    /// Take every queued command, leaving the sink empty.
    #[must_use = "the drained commands must be routed onto action/voice/*"]
    pub fn drain(&mut self) -> Vec<VoiceAdminCommand> {
        std::mem::take(&mut self.queued)
    }

    /// Queued commands without draining.
    #[must_use]
    pub fn queued(&self) -> &[VoiceAdminCommand] {
        &self.queued
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Pending DID routes already in this sink (`did` → `node_id`).
    #[must_use]
    pub fn pending_did_routes(&self) -> Vec<(String, Option<String>)> {
        self.queued
            .iter()
            .filter_map(|command| match command {
                VoiceAdminCommand::DidRoute { did, node_id } => {
                    Some((did.clone(), node_id.clone()))
                }
                _ => None,
            })
            .collect()
    }
}

/// Why a Voice admin verb was refused at the UI/verb boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceAdminRefuse {
    /// DID is malformed or not in the master inventory.
    InvalidDid,
    /// Target node is not in the fleet board.
    UnknownNode,
    /// The same DID is already pending toward a different node.
    ConflictingRoute,
    /// DID/failover/cutover require a provisioned sub-account.
    NoProvisionedAccount,
    /// Shared-outbound caller-ID or trunk is unusable.
    InvalidSharedConfig,
    /// Cutover cannot run until shared-outbound is lifted and nodes remain.
    CutoverNotReady,
}

impl VoiceAdminRefuse {
    /// Operator-visible refusal copy.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::InvalidDid => "Invalid DID — use an existing inventory number",
            Self::UnknownNode => "Unknown node — pick a fleet-board node id",
            Self::ConflictingRoute => "Conflicting route — that DID is already pending elsewhere",
            Self::NoProvisionedAccount => "No provisioned voice account yet",
            Self::InvalidSharedConfig => "Shared-outbound caller-ID or trunk is invalid",
            Self::CutoverNotReady => "Cutover is not ready — lift shared-outbound first",
        }
    }
}

/// Button-level intent on the fleet voice-admin form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceAdminFormIntent {
    /// Publish `action/voice/provision`.
    Provision,
    /// Route the draft DID to `route_node`.
    DidRoute,
    /// Unroute the draft DID to the main line.
    Unroute,
    /// Apply the draft failover policy.
    Failover,
    /// Apply the draft shared-outbound.
    SharedConfig,
    /// Arm cutover confirm, or publish cutover when already armed.
    Cutover,
    /// Drop a pending cutover confirm.
    CancelCutover,
}

/// Result of applying one [`VoiceAdminFormIntent`] to the local form + sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceAdminFormOutcome {
    /// A typed verb was queued for the shell.
    Published(VoiceAdminCommand),
    /// First cutover click — nothing published until confirm.
    ArmedCutover,
    /// Confirm was dismissed.
    CancelledCutover,
    /// Verb boundary refused; the sink is unchanged.
    Refused(VoiceAdminRefuse),
}

/// Redacted `get-gateway` reply shape. `password` is always the empty string;
/// `password_set` distinguishes a stored secret from an intentionally empty one.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayReadout {
    /// Whether `gateway.toml` is present.
    pub present: bool,
    /// Registrar host.
    pub host: String,
    /// Registrar port (default 5060).
    pub port: u16,
    /// SIP username.
    pub username: String,
    /// Always empty — the responder never returns the stored credential.
    pub password: String,
    /// Whether a password is stored.
    pub password_set: bool,
    /// Optional display name.
    pub display_name: String,
    /// REGISTER expiry in seconds.
    pub expires: u32,
}

impl GatewayReadout {
    /// Absent gateway — `present: false`.
    #[must_use]
    pub fn absent() -> Self {
        Self {
            present: false,
            host: String::new(),
            port: 5060,
            username: String::new(),
            password: String::new(),
            password_set: false,
            display_name: String::new(),
            expires: 3600,
        }
    }

    /// Present gateway with the password already redacted.
    #[must_use]
    pub fn present(
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        password_set: bool,
        display_name: impl Into<String>,
        expires: u32,
    ) -> Self {
        Self {
            present: true,
            host: host.into(),
            port,
            username: username.into(),
            password: String::new(),
            password_set,
            display_name: display_name.into(),
            expires,
        }
    }

    /// The password field the readout may paint — always empty.
    #[must_use]
    pub const fn redacted_password(&self) -> &'static str {
        ""
    }

    /// Bind a `get-gateway` reply. Any `password` in the JSON is dropped so a
    /// leaked credential can never become readout copy.
    #[must_use]
    pub fn from_get_reply(body: &str) -> Option<Self> {
        let present = json_flat_bool(body, "present")?;
        if !present {
            return Some(Self::absent());
        }
        let host = json_flat_string(body, "host").unwrap_or_default();
        let port = json_flat_u64(body, "port")
            .and_then(|port| u16::try_from(port).ok())
            .unwrap_or(5060);
        let username = json_flat_string(body, "username").unwrap_or_default();
        let password_set = json_flat_bool(body, "password_set").unwrap_or(false);
        let display_name = json_flat_string(body, "display_name").unwrap_or_default();
        let expires = json_flat_u64(body, "expires")
            .and_then(|expires| u32::try_from(expires).ok())
            .unwrap_or(3600);
        Some(Self::present(
            host,
            port,
            username,
            password_set,
            display_name,
            expires,
        ))
    }

    /// Bind the voice-agent `account.toml` shape written to `gateway.toml`.
    /// The password is observed only to set `password_set` and is never kept.
    #[must_use]
    pub fn from_gateway_toml(text: &str) -> Option<Self> {
        let server = toml_flat_string(text, "server")?;
        if server.is_empty() {
            return None;
        }
        let (host, port) = split_gateway_server(&server);
        let username = toml_flat_string(text, "username").unwrap_or_default();
        let password_set =
            toml_flat_string(text, "password").is_some_and(|password| !password.is_empty());
        let display_name = toml_flat_string(text, "display_name").unwrap_or_default();
        let expires = toml_flat_u32(text, "expires").unwrap_or(3600);
        Some(Self::present(
            host,
            port,
            username,
            password_set,
            display_name,
            expires,
        ))
    }

    /// Load `<path>` without rewriting it. Missing or unparseable files yield
    /// `None` so a retained get-gateway body can still hydrate.
    #[must_use]
    pub fn from_gateway_toml_path(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        Self::from_gateway_toml(&text)
    }
}

/// Hydrate the redacted get-gateway shape from an in-place workgroup
/// `gateway.toml` (account.toml shape), falling back to a retained
/// `get-gateway` body. Neither source is rewritten; any password is dropped.
#[must_use]
pub fn hydrate_gateway_readout(
    workgroup_root: Option<&Path>,
    retained_get: Option<&str>,
) -> GatewayReadout {
    if let Some(root) = workgroup_root {
        if let Some(from_file) =
            GatewayReadout::from_gateway_toml_path(&workgroup_gateway_toml_path(root))
        {
            return from_file;
        }
    }
    if let Some(body) = retained_get {
        if let Some(from_get) = GatewayReadout::from_get_reply(body) {
            return from_get;
        }
    }
    GatewayReadout::absent()
}

impl std::fmt::Debug for GatewayReadout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayReadout")
            .field("present", &self.present)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.redacted_password())
            .field("password_set", &self.password_set)
            .field("display_name", &self.display_name)
            .field("expires", &self.expires)
            .finish()
    }
}

/// Typed SIP-gateway verb the shell drains onto `action/voip/*`.
#[derive(Clone, PartialEq, Eq)]
pub enum GatewayCommand {
    /// `set-gateway` body (`host`, `port`?, `username`, `password`?,
    /// `display_name`?, `expires`?).
    Set {
        /// Registrar host.
        host: String,
        /// Optional port. Omitted from JSON when `None`.
        port: Option<u64>,
        /// SIP username.
        username: String,
        /// SIP password. Empty means "unchanged" on an existing gateway.
        password: String,
        /// Optional display name.
        display_name: String,
        /// Optional REGISTER expiry.
        expires: Option<u64>,
    },
    /// `get-gateway` — no body.
    Get,
    /// `clear-gateway` — empty JSON object; payload ignored by the responder.
    Clear,
}

impl std::fmt::Debug for GatewayCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Set {
                host,
                port,
                username,
                password,
                display_name,
                expires,
            } => f
                .debug_struct("Set")
                .field("host", host)
                .field("port", port)
                .field("username", username)
                .field(
                    "password",
                    if password.is_empty() {
                        &""
                    } else {
                        &"<redacted>"
                    },
                )
                .field("display_name", display_name)
                .field("expires", expires)
                .finish(),
            Self::Get => write!(f, "Get"),
            Self::Clear => write!(f, "Clear"),
        }
    }
}

impl GatewayCommand {
    /// Canonical Bus topic this command publishes on.
    #[must_use]
    pub const fn topic(&self) -> &'static str {
        match self {
            Self::Set { .. } => VOIP_SET_GATEWAY_TOPIC,
            Self::Get => VOIP_GET_GATEWAY_TOPIC,
            Self::Clear => VOIP_CLEAR_GATEWAY_TOPIC,
        }
    }

    /// JSON body matching the responder, or `None` for `get-gateway`.
    ///
    /// The write body includes the password so the shell can drain it unchanged;
    /// it is never used as readout copy.
    #[must_use]
    pub fn json_body(&self) -> Option<String> {
        match self {
            Self::Set {
                host,
                port,
                username,
                password,
                display_name,
                expires,
            } => {
                let mut body = format!(
                    "{{\"host\":{},\"username\":{},\"password\":{},\"display_name\":{}",
                    json_string(host),
                    json_string(username),
                    json_string(password),
                    json_string(display_name)
                );
                if let Some(port) = port {
                    body.push_str(&format!(",\"port\":{port}"));
                }
                if let Some(expires) = expires {
                    body.push_str(&format!(",\"expires\":{expires}"));
                }
                body.push('}');
                Some(body)
            }
            Self::Get => None,
            Self::Clear => Some("{}".to_owned()),
        }
    }

    /// Readout-safe JSON: password forced to `""` so a write never echoes back.
    #[must_use]
    pub fn redacted_json_body(&self) -> Option<String> {
        match self {
            Self::Set { password, .. } if !password.is_empty() => self.json_body().map(|body| {
                body.replacen(
                    &format!("\"password\":{}", json_string(password)),
                    "\"password\":\"\"",
                    1,
                )
            }),
            other => other.json_body(),
        }
    }
}

/// Caller-drained SIP-gateway intent queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GatewaySink {
    queued: Vec<GatewayCommand>,
}

impl GatewaySink {
    /// A fresh, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `command` for the shell to publish.
    pub fn emit(&mut self, command: GatewayCommand) {
        self.queued.push(command);
    }

    /// Take every queued command, leaving the sink empty.
    #[must_use = "the drained commands must be routed onto action/voip/*"]
    pub fn drain(&mut self) -> Vec<GatewayCommand> {
        std::mem::take(&mut self.queued)
    }

    /// Queued commands without draining.
    #[must_use]
    pub fn queued(&self) -> &[GatewayCommand] {
        &self.queued
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Whether this sink already carries a `clear-gateway` (a replay).
    #[must_use]
    pub fn has_clear(&self) -> bool {
        self.queued
            .iter()
            .any(|command| matches!(command, GatewayCommand::Clear))
    }
}

/// Why a gateway verb was refused at the UI/verb boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayRefuse {
    /// Host is empty, has a scheme/path, whitespace, or illegal hostname chars.
    MalformedHost,
    /// Port is zero or otherwise unusable.
    InvalidPort,
    /// `set-gateway` requires a username.
    UsernameRequired,
    /// REGISTER expiry is outside the responder's u32 contract.
    InvalidExpiry,
    /// Clear when the gateway is already absent, or a second clear in this sink.
    ReplayClear,
    /// Clear was clicked without the confirm step.
    UnconfirmedClear,
}

impl GatewayRefuse {
    /// Operator-visible refusal copy.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MalformedHost => "Malformed gateway host",
            Self::InvalidPort => "Gateway port must be 1–65535",
            Self::UsernameRequired => "Gateway username is required",
            Self::InvalidExpiry => "Gateway expiry must be between 1 and 4294967295 seconds",
            Self::ReplayClear => "Gateway is already cleared",
            Self::UnconfirmedClear => "Confirm clear gateway to remove gateway.toml",
        }
    }
}

/// Button-level intent on the SIP-gateway form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayFormIntent {
    /// Publish `set-gateway` from the draft fields.
    Set,
    /// Publish `get-gateway`.
    Get,
    /// Arm the clear confirm, or publish `clear-gateway` when already armed.
    Clear,
    /// Drop a pending clear confirm.
    CancelClear,
}

/// Result of applying one [`GatewayFormIntent`] to the local form + sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayFormOutcome {
    /// A typed verb was queued for the shell.
    Published(GatewayCommand),
    /// First clear click — nothing published until confirm.
    ArmedClear,
    /// Confirm was dismissed.
    CancelledClear,
    /// Verb boundary refused; the sink is unchanged.
    Refused(GatewayRefuse),
}

/// Admit one gateway-form intent. The password is wiped after a successful set
/// or clear and never copied into the notice. Clear is a two-step: first click
/// arms, second click publishes, a replayed or absent clear refuses.
pub fn apply_gateway_form(
    form: &mut GatewayFormState,
    readout: Option<&GatewayReadout>,
    sink: &mut GatewaySink,
    intent: GatewayFormIntent,
) -> GatewayFormOutcome {
    match intent {
        GatewayFormIntent::Set => apply_gateway_set(form, sink),
        GatewayFormIntent::Get => {
            let command = GatewayCommand::Get;
            sink.emit(command.clone());
            form.notice = Some("Gateway get published".to_owned());
            GatewayFormOutcome::Published(command)
        }
        GatewayFormIntent::Clear => apply_gateway_clear(form, readout, sink),
        GatewayFormIntent::CancelClear => {
            form.confirm_clear = false;
            GatewayFormOutcome::CancelledClear
        }
    }
}

fn apply_gateway_set(form: &mut GatewayFormState, sink: &mut GatewaySink) -> GatewayFormOutcome {
    let port = parse_optional_u64(&form.port);
    let expires = parse_optional_u64(&form.expires);
    let outcome = match (port, expires) {
        (Err(()), _) => GatewayFormOutcome::Refused(GatewayRefuse::InvalidPort),
        (_, Err(())) => GatewayFormOutcome::Refused(GatewayRefuse::InvalidExpiry),
        (Ok(port), Ok(expires)) => {
            match validate_gateway_set(
                &form.host,
                port,
                &form.username,
                &form.password,
                &form.display_name,
                expires,
            ) {
                Ok(command) => {
                    sink.emit(command.clone());
                    form.password.clear();
                    form.notice = Some("Gateway set published".to_owned());
                    return GatewayFormOutcome::Published(command);
                }
                Err(refuse) => GatewayFormOutcome::Refused(refuse),
            }
        }
    };
    if let GatewayFormOutcome::Refused(refuse) = &outcome {
        form.notice = Some(refuse.label().to_owned());
    }
    outcome
}

fn apply_gateway_clear(
    form: &mut GatewayFormState,
    readout: Option<&GatewayReadout>,
    sink: &mut GatewaySink,
) -> GatewayFormOutcome {
    if !form.confirm_clear {
        form.confirm_clear = true;
        form.notice = Some(GatewayRefuse::UnconfirmedClear.label().to_owned());
        return GatewayFormOutcome::ArmedClear;
    }
    match validate_gateway_clear(readout, sink) {
        Ok(command) => {
            sink.emit(command.clone());
            form.host.clear();
            form.port.clear();
            form.username.clear();
            form.display_name.clear();
            form.expires.clear();
            form.password.clear();
            form.confirm_clear = false;
            form.notice = Some("Gateway clear published".to_owned());
            GatewayFormOutcome::Published(command)
        }
        Err(refuse) => {
            form.confirm_clear = false;
            form.notice = Some(refuse.label().to_owned());
            GatewayFormOutcome::Refused(refuse)
        }
    }
}

fn theme_color(ui: &egui::Ui, color: egui::Color32) -> egui::Color32 {
    Style::resolve_color(ui.ctx(), color)
}

const ACTIVITY_ROW_HEIGHT: f32 = Style::SP_L;
/// Keep a burst of identical Activity notifications readable without merging
/// separate incidents that happen later. This matches the notification lane's
/// bounded five-minute coalescing window.
const ACTIVITY_COALESCE_WINDOW_MS: u64 = 5 * 60 * 1_000;

/// View-local state for one Activity source. The entries are cloned only when
/// the source is live (to establish the pause snapshot) or when Resume is
/// clicked; while paused they remain the exact projection snapshot the user
/// chose to hold.
#[derive(Clone, Default)]
struct ActivityViewState {
    source: Option<ActivitySource>,
    paused: bool,
    entries: Vec<ActivityEntry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ActivitySource {
    app: MeshTeamsApp,
    space: Option<SpaceId>,
}

/// One visible row backed by one real ActivityEntry. `count` is the number of
/// adjacent, identical projection entries represented by the row; no synthetic
/// ActivityEntry is introduced by the UI.
#[derive(Clone, Copy)]
pub(crate) struct ActivityRow<'a> {
    entry: &'a ActivityEntry,
    count: usize,
    severity: Option<Severity>,
}

impl ActivityRow<'_> {
    #[cfg(test)]
    pub(crate) fn entry(&self) -> &ActivityEntry {
        self.entry
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    #[cfg(test)]
    pub(crate) fn severity(&self) -> Option<Severity> {
        self.severity
    }
}

/// The bounded/coalesced rows supplied to the virtualized Activity painter.
pub(crate) struct CoalescedActivityRows<'a> {
    rows: Vec<ActivityRow<'a>>,
}

impl<'a> CoalescedActivityRows<'a> {
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn range(&self, row_range: Range<usize>) -> CoalescedActivityRowRange<'_, 'a> {
        CoalescedActivityRowRange {
            rows: &self.rows,
            next: row_range.start,
            end: row_range.end,
        }
    }
}

struct CoalescedActivityRowRange<'rows, 'entry> {
    rows: &'rows [ActivityRow<'entry>],
    next: usize,
    end: usize,
}

impl<'rows, 'entry> Iterator for CoalescedActivityRowRange<'rows, 'entry> {
    type Item = &'rows ActivityRow<'entry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        self.next += 1;
        self.rows.get(index)
    }
}

/// Activity feed plus the fleet voice-admin and SIP-gateway panels, reading
/// retained `state/voice/*` / `get-gateway` projections from `admin` and
/// publishing typed verbs into the surface's retained sinks (never the Bus).
pub fn activity_body_with_admin(
    surface: &mut CommunicationsSurface,
    ui: &mut egui::Ui,
    data: &dyn crate::CollabData,
    admin: &dyn ActivityAdminData,
) {
    surface.activity_filter_chips(ui);
    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_S);

    let feed = if surface.app() == MeshTeamsApp::Activity {
        data.activity(None)
            .or_else(|| data.activity(surface.selected_space()))
    } else {
        data.activity(surface.selected_space())
    };
    let live_entries: &[ActivityEntry] = feed.map_or(&[], |f| f.entries.as_slice());
    let source = ActivitySource {
        app: surface.app(),
        space: feed.map(|f| f.space).unwrap_or(surface.selected_space()),
    };
    let state_id = ui.id().with("activity-feed-state");
    let mut state = activity_view_state(ui, state_id, source, live_entries);
    activity_pause_resume_control(ui, &mut state, live_entries);
    ui.ctx()
        .data_mut(|data| data.insert_temp(state_id, state.clone()));

    let filter = surface.activity_filter();
    let now = data.now_unix_ms();

    let admitted = coalesced_activity_rows(&state.entries, filter, data.alert_inbox());
    if admitted.is_empty() {
        ui.label(
            egui::RichText::new("No activity for this filter yet")
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
    } else {
        egui::ScrollArea::vertical()
            .max_height(Style::SP_XL * 8.0)
            .auto_shrink([false, false])
            .show_rows(ui, ACTIVITY_ROW_HEIGHT, admitted.len(), |ui, row_range| {
                for row in admitted.range(row_range) {
                    activity_row(ui, row, now);
                }
            });
    }

    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.columns(2, |columns| {
        voice_admin_panel(&mut columns[0], data, admin, &mut surface.voice_admin_sink);
        gateway_admin_panel(&mut columns[1], admin, &mut surface.gateway_sink);
    });
}

impl CommunicationsSurface {
    /// The band-filter chip row (`All`, `Messages`, `Alerts`, `Calls`, `Files`,
    /// `People`). A chip carries a Carbon glyph when the band has a faithful one.
    pub(crate) fn activity_filter_chips(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for filter in ActivityFilter::ALL {
                let selected = self.activity_filter() == filter;
                if let Some(glyph) = icons::activity_filter_icon(filter) {
                    let tint = if selected {
                        Style::ACCENT
                    } else {
                        theme_color(ui, Style::TEXT_DIM)
                    };
                    icons::icon(ui, glyph, Style::SP_M, tint);
                }
                if ui.selectable_label(selected, filter.label()).clicked() {
                    self.activity_filter = filter;
                }
                ui.add_space(Style::SP_XS);
            }
        });
    }
}

/// Load or refresh the source snapshot. egui's temporary data survives frames
/// for the lifetime of this context, which is enough for a seat-local view
/// preference without adding state to the collaboration contract or surface
/// model.
fn activity_view_state(
    ui: &egui::Ui,
    state_id: egui::Id,
    source: ActivitySource,
    live_entries: &[ActivityEntry],
) -> ActivityViewState {
    let mut state = ui
        .ctx()
        .data_mut(|data| data.get_temp::<ActivityViewState>(state_id))
        .unwrap_or_default();

    if state.source != Some(source) {
        state = ActivityViewState {
            source: Some(source),
            paused: false,
            entries: live_entries.to_vec(),
        };
    } else if !state.paused {
        state.entries = live_entries.to_vec();
    }

    state
}

/// A visible, text-labelled control for holding the current feed snapshot.
/// Pausing only affects this view: the real projection keeps updating behind
/// it, and Resume takes a fresh snapshot before painting the next row count.
fn activity_pause_resume_control(
    ui: &mut egui::Ui,
    state: &mut ActivityViewState,
    live_entries: &[ActivityEntry],
) {
    ui.horizontal(|ui| {
        let (status, status_color, action) = if state.paused {
            ("Feed paused", Style::WARN, "Resume feed")
        } else {
            ("Live feed", Style::OK, "Pause feed")
        };
        ui.label(
            egui::RichText::new(status)
                .small()
                .color(theme_color(ui, status_color)),
        );
        if ui.button(action).clicked() {
            state.paused = !state.paused;
            if !state.paused {
                state.entries = live_entries.to_vec();
            }
        }
    });
}

/// Compatibility helpers for the pre-coalescing unit tests. Production
/// rendering uses [`CoalescedActivityRows`] exclusively, so this borrowed
/// source-slice assertion does not leave a second virtualized implementation in
/// the shipped Activity path.
#[cfg(test)]
pub(crate) enum ActivityRows<'a> {
    All(&'a [ActivityEntry]),
    Filtered(Vec<&'a ActivityEntry>),
}

#[cfg(test)]
impl ActivityRows<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::All(entries) => entries.len(),
            Self::Filtered(entries) => entries.len(),
        }
    }

    #[cfg(test)]
    pub(crate) fn uses_unfiltered_source(&self) -> bool {
        matches!(self, Self::All(_))
    }
}

#[cfg(test)]
pub(crate) fn activity_rows(entries: &[ActivityEntry], filter: ActivityFilter) -> ActivityRows<'_> {
    if filter == ActivityFilter::All {
        ActivityRows::All(entries)
    } else {
        ActivityRows::Filtered(filtered_activity_entries(entries, filter))
    }
}

#[cfg(test)]
pub(crate) fn filtered_activity_entries(
    entries: &[ActivityEntry],
    filter: ActivityFilter,
) -> Vec<&ActivityEntry> {
    entries
        .iter()
        .filter(|entry| filter.matches(&entry.kind_tag))
        .collect()
}

/// Admit filtered Activity entries and coalesce only adjacent repeats within
/// the bounded notification window. The severity is joined from the existing
/// AlertInbox by event id; equal summaries at different severity levels remain
/// separate rows so a Critical alert can never disappear into an Info repeat.
pub(crate) fn coalesced_activity_rows<'a>(
    entries: &'a [ActivityEntry],
    filter: ActivityFilter,
    alert_inbox: Option<&AlertInbox>,
) -> CoalescedActivityRows<'a> {
    let mut rows = Vec::new();

    for entry in entries
        .iter()
        .filter(|entry| filter.matches(&entry.kind_tag))
    {
        let severity = activity_entry_severity(entry, alert_inbox);
        let can_coalesce = rows.last().is_some_and(|last: &ActivityRow<'a>| {
            last.entry.space == entry.space
                && last.entry.actor == entry.actor
                && last.entry.kind_tag == entry.kind_tag
                && last.entry.summary == entry.summary
                && last.severity == severity
                && last.entry.created_unix_ms.abs_diff(entry.created_unix_ms)
                    < ACTIVITY_COALESCE_WINDOW_MS
        });

        if can_coalesce {
            if let Some(last) = rows.last_mut() {
                last.count = last.count.saturating_add(1);
            }
        } else {
            rows.push(ActivityRow {
                entry,
                count: 1,
                severity,
            });
        }
    }

    CoalescedActivityRows { rows }
}

fn activity_entry_severity(
    entry: &ActivityEntry,
    alert_inbox: Option<&AlertInbox>,
) -> Option<Severity> {
    alert_inbox.and_then(|inbox| {
        inbox
            .alerts
            .iter()
            .find(|view| view.event_id == entry.event_id)
            .map(|view| view.alert.severity)
    })
}

/// One Activity row: a band glyph, the actor, the projected summary line, and a
/// right-aligned relative age.
fn activity_row(ui: &mut egui::Ui, row: &ActivityRow<'_>, now_unix_ms: i64) {
    let entry = row.entry;
    let icon_color = row
        .severity
        .map(activity_severity_color)
        .unwrap_or(theme_color(ui, Style::TEXT_DIM));
    ui.horizontal(|ui| {
        icons::icon(ui, entry_icon(&entry.kind_tag), Style::SP_M, icon_color);
        ui.label(
            egui::RichText::new(entry.actor.as_str())
                .small()
                .strong()
                .color(theme_color(ui, Style::TEXT)),
        );
        ui.label(egui::RichText::new(&entry.summary).color(theme_color(ui, Style::TEXT)));
        if row.count > 1 {
            ui.label(
                egui::RichText::new(format!("×{}", row.count))
                    .small()
                    .strong()
                    .color(theme_color(ui, icon_color)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(relative_age(now_unix_ms, entry.created_unix_ms))
                    .small()
                    .color(theme_color(ui, Style::TEXT_DIM)),
            );
        });
    });
}

const fn activity_severity_color(severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Info => Style::ACCENT,
        Severity::Warning => Style::WARN,
        Severity::Critical => Style::DANGER,
    }
}

/// The Carbon glyph for an Activity row, chosen from the event-kind band the
/// same way the filter classifies it (kept within [`ALL_COLLAB_ICONS`]).
///
/// [`ALL_COLLAB_ICONS`]: crate::ALL_COLLAB_ICONS
fn entry_icon(kind_tag: &str) -> &'static str {
    if ActivityFilter::Messages.matches(kind_tag) {
        "share"
    } else if ActivityFilter::Alerts.matches(kind_tag) {
        "notification"
    } else if ActivityFilter::Calls.matches(kind_tag) {
        "audio-volume-high"
    } else if ActivityFilter::Files.matches(kind_tag) {
        "download"
    } else if ActivityFilter::People.matches(kind_tag) {
        "view-grid"
    } else {
        "view"
    }
}

/// Whether any fleet-board row carries a provisioned sub-account.
#[must_use]
pub fn has_provisioned_voice_account(nodes: &[VoiceNodeProjection]) -> bool {
    nodes.iter().any(VoiceNodeProjection::is_provisioned)
}

/// Resolve a fleet-board target from the wire `node_id`, a unique hostname,
/// or a unique sub-account username. The published verb always carries the
/// worker's `node_id`. Ambiguous hostnames/usernames refuse as unknown.
pub fn resolve_voice_node<'a>(
    nodes: &'a [VoiceNodeProjection],
    identity: &str,
) -> Result<&'a VoiceNodeProjection, VoiceAdminRefuse> {
    let identity = identity.trim();
    if identity.is_empty() {
        return Err(VoiceAdminRefuse::UnknownNode);
    }
    if let Some(node) = nodes.iter().find(|row| row.node_id == identity) {
        return Ok(node);
    }
    let by_host: Vec<&VoiceNodeProjection> = nodes
        .iter()
        .filter(|row| row.hostname == identity)
        .collect();
    if by_host.len() == 1 {
        return Ok(by_host[0]);
    }
    if by_host.len() > 1 {
        return Err(VoiceAdminRefuse::UnknownNode);
    }
    let by_user: Vec<&VoiceNodeProjection> = nodes
        .iter()
        .filter(|row| !row.username.is_empty() && row.username == identity)
        .collect();
    if by_user.len() == 1 {
        return Ok(by_user[0]);
    }
    Err(VoiceAdminRefuse::UnknownNode)
}

/// Operator copy when the fleet board has rows but none are Registered /
/// Unregistered. Distinguishes awaiting-master-key / live-provider from a
/// blank snapshot so the leftover Vitelity gate cannot look provisioned.
#[must_use]
pub fn voice_unprovisioned_headline(nodes: &[VoiceNodeProjection]) -> &'static str {
    if nodes.is_empty() {
        return "No provisioned voice account";
    }
    if nodes
        .iter()
        .any(|node| matches!(node.reg_state, VoiceRegState::Error { .. }))
    {
        return "Voice provider error";
    }
    if nodes
        .iter()
        .all(|node| matches!(node.reg_state, VoiceRegState::Provisioning))
    {
        return "Awaiting Vitelity master key";
    }
    "No provisioned voice account"
}

/// Longer empty-state body under [`voice_unprovisioned_headline`].
#[must_use]
pub fn voice_unprovisioned_detail(nodes: &[VoiceNodeProjection]) -> &'static str {
    if nodes.is_empty() {
        return "DID routing, failover, shared-outbound, and cutover stay empty until a node has a sub-account.";
    }
    if nodes
        .iter()
        .any(|node| matches!(node.reg_state, VoiceRegState::Error { .. }))
    {
        return "Provider has not sealed a sub-account. The fleet board shows the honest error.";
    }
    if nodes
        .iter()
        .all(|node| matches!(node.reg_state, VoiceRegState::Provisioning))
    {
        return "Enrolled nodes are awaiting a sealed Vitelity master key or a live provider pass. DID routing stays closed until a node is Registered or Unregistered.";
    }
    "DID routing, failover, shared-outbound, and cutover stay empty until a node is Registered or Unregistered."
}

/// Admit a DID-route verb at the UI boundary. Invalid DIDs, unknown nodes, and
/// conflicting in-flight routes refuse; a DID not in the master inventory is
/// an invalid DID (route-existing only).
pub fn validate_did_route(
    did: &str,
    node_id: Option<&str>,
    inventory: &[VoiceDid],
    nodes: &[VoiceNodeProjection],
    pending: &[(String, Option<String>)],
) -> Result<VoiceAdminCommand, VoiceAdminRefuse> {
    if !has_provisioned_voice_account(nodes) {
        return Err(VoiceAdminRefuse::NoProvisionedAccount);
    }
    let did = did.trim();
    if !is_valid_did(did) || !inventory.iter().any(|row| row.number == did) {
        return Err(VoiceAdminRefuse::InvalidDid);
    }
    let node_id = match node_id {
        None => None,
        Some(identity) => {
            let node = resolve_voice_node(nodes, identity)?;
            if !node.is_provisioned() {
                return Err(VoiceAdminRefuse::NoProvisionedAccount);
            }
            Some(node.node_id.clone())
        }
    };
    if pending.iter().any(|(pending_did, pending_node)| {
        pending_did == did && pending_node.as_deref() != node_id.as_deref()
    }) {
        return Err(VoiceAdminRefuse::ConflictingRoute);
    }
    Ok(VoiceAdminCommand::DidRoute {
        did: did.to_owned(),
        node_id,
    })
}

/// Admit a failover verb at the UI boundary.
pub fn validate_failover(
    node_id: &str,
    policy: VoiceFailoverPolicy,
    nodes: &[VoiceNodeProjection],
) -> Result<VoiceAdminCommand, VoiceAdminRefuse> {
    if !has_provisioned_voice_account(nodes) {
        return Err(VoiceAdminRefuse::NoProvisionedAccount);
    }
    let node = resolve_voice_node(nodes, node_id)?;
    if !node.is_provisioned() {
        return Err(VoiceAdminRefuse::NoProvisionedAccount);
    }
    let node_id = node.node_id.as_str();
    if let VoiceFailoverPolicy::Forward { ref number } = policy {
        if !is_valid_did(number) {
            return Err(VoiceAdminRefuse::InvalidDid);
        }
    }
    Ok(VoiceAdminCommand::Failover {
        node_id: node_id.to_owned(),
        policy,
    })
}

/// Admit a shared-outbound verb. Caller-ID must be a DID; trunk must be a
/// non-empty label.
pub fn validate_shared_config(
    caller_id: &str,
    outbound_trunk: &str,
) -> Result<VoiceAdminCommand, VoiceAdminRefuse> {
    let caller_id = caller_id.trim();
    let outbound_trunk = outbound_trunk.trim();
    if !is_valid_did(caller_id)
        || outbound_trunk.is_empty()
        || outbound_trunk.chars().any(char::is_control)
    {
        return Err(VoiceAdminRefuse::InvalidSharedConfig);
    }
    Ok(VoiceAdminCommand::SharedConfig {
        caller_id: caller_id.to_owned(),
        outbound_trunk: outbound_trunk.to_owned(),
    })
}

/// Admit a cutover control. Requires a lifted shared-outbound and remaining
/// pending nodes; complete/legacy refuse.
pub fn validate_cutover(
    cutover: Option<&VoiceCutoverStatus>,
    nodes: &[VoiceNodeProjection],
) -> Result<VoiceAdminCommand, VoiceAdminRefuse> {
    if !has_provisioned_voice_account(nodes) {
        return Err(VoiceAdminRefuse::NoProvisionedAccount);
    }
    let Some(status) = cutover else {
        return Err(VoiceAdminRefuse::CutoverNotReady);
    };
    if !status.shared_outbound_lifted
        || matches!(
            status.phase,
            VoiceCutoverPhase::Legacy | VoiceCutoverPhase::CutoverComplete
        )
        || status.pending_nodes.is_empty()
    {
        return Err(VoiceAdminRefuse::CutoverNotReady);
    }
    Ok(VoiceAdminCommand::Cutover)
}

/// Admit `set-gateway` at the UI boundary. Empty host is not a clear — that is
/// an explicit [`validate_gateway_clear`].
pub fn validate_gateway_set(
    host: &str,
    port: Option<u64>,
    username: &str,
    password: &str,
    display_name: &str,
    expires: Option<u64>,
) -> Result<GatewayCommand, GatewayRefuse> {
    let host = host.trim();
    if !is_valid_gateway_host(host) {
        return Err(GatewayRefuse::MalformedHost);
    }
    if let Some(port) = port {
        if port == 0 || port > u64::from(u16::MAX) {
            return Err(GatewayRefuse::InvalidPort);
        }
    }
    if let Some(expires) = expires {
        if expires == 0 || expires > u64::from(u32::MAX) {
            return Err(GatewayRefuse::InvalidExpiry);
        }
    }
    let username = username.trim();
    if username.is_empty() {
        return Err(GatewayRefuse::UsernameRequired);
    }
    Ok(GatewayCommand::Set {
        host: host.to_owned(),
        port,
        username: username.to_owned(),
        password: password.to_owned(),
        display_name: display_name.to_owned(),
        expires,
    })
}

/// Admit `clear-gateway`. Absent gateways and a second clear already queued in
/// `sink` are replayed clears and refuse.
pub fn validate_gateway_clear(
    readout: Option<&GatewayReadout>,
    sink: &GatewaySink,
) -> Result<GatewayCommand, GatewayRefuse> {
    let present = readout.is_some_and(|row| row.present);
    if !present || sink.has_clear() {
        return Err(GatewayRefuse::ReplayClear);
    }
    Ok(GatewayCommand::Clear)
}

/// E.164-ish DID: optional `+`, then 8–15 digits.
#[must_use]
pub fn is_valid_did(did: &str) -> bool {
    let did = did.trim();
    let digits = did.strip_prefix('+').unwrap_or(did);
    let len = digits.len();
    (8..=15).contains(&len) && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Registrar host: IPv4 or DNS label, no scheme, path, port, or whitespace.
#[must_use]
pub fn is_valid_gateway_host(host: &str) -> bool {
    let host = host.trim();
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.contains("://")
        || host.contains('/')
        || host.contains('\\')
        || host.contains('@')
        || host.contains(' ')
        || host.contains(':')
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
    {
        return false;
    }
    if is_ipv4_host(host) {
        return true;
    }
    host.split('.').all(is_dns_label)
}

fn is_ipv4_host(host: &str) -> bool {
    let mut count = 0usize;
    for part in host.split('.') {
        count += 1;
        if count > 4 {
            return false;
        }
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if part.parse::<u8>().is_err() {
            return false;
        }
    }
    count == 4
}

/// Split `host` / `host:port`, defaulting 5060 (mirrors the voip responder).
fn split_gateway_server(server: &str) -> (String, u16) {
    match server.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (server.to_string(), 5060),
        },
        _ => (server.to_string(), 5060),
    }
}

fn toml_flat_u32(body: &str, key: &str) -> Option<u32> {
    toml_flat_raw(body, key)?.parse().ok()
}

fn toml_flat_string(body: &str, key: &str) -> Option<String> {
    let raw = toml_flat_raw(body, key)?;
    if raw.starts_with('"') {
        json_unquote(raw)
    } else if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn toml_flat_raw<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        if lhs.trim() != key {
            continue;
        }
        let rhs = rhs.trim_start();
        if rhs.starts_with('"') {
            let mut end = 1;
            let bytes = rhs.as_bytes();
            while end < bytes.len() {
                match bytes[end] {
                    b'\\' if end + 1 < bytes.len() => end += 2,
                    b'"' => return Some(&rhs[..=end]),
                    _ => end += 1,
                }
            }
            return None;
        }
        let end = rhs
            .find(|c: char| c == '#' || c.is_whitespace())
            .unwrap_or(rhs.len());
        return Some(rhs[..end].trim());
    }
    None
}

fn json_flat_bool(body: &str, key: &str) -> Option<bool> {
    match json_flat_raw(body, key)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn json_flat_u64(body: &str, key: &str) -> Option<u64> {
    json_flat_raw(body, key)?.parse().ok()
}

fn json_flat_string(body: &str, key: &str) -> Option<String> {
    let raw = json_flat_raw(body, key)?;
    json_unquote(raw)
}

fn json_flat_raw<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let after = body.find(&needle)? + needle.len();
    let rest = body[after..].trim_start().strip_prefix(':')?.trim_start();
    if rest.starts_with('"') {
        let mut end = 1;
        let bytes = rest.as_bytes();
        while end < bytes.len() {
            match bytes[end] {
                b'\\' if end + 1 < bytes.len() => end += 2,
                b'"' => return Some(&rest[..=end]),
                _ => end += 1,
            }
        }
        return None;
    }
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn json_unquote(raw: &str) -> Option<String> {
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let mut hex = String::new();
                for _ in 0..4 {
                    hex.push(chars.next()?);
                }
                let code = u32::from_str_radix(&hex, 16).ok()?;
                out.push(char::from_u32(code)?);
            }
            other => out.push(other),
        }
    }
    Some(out)
}

fn is_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

fn json_string(value: &str) -> String {
    let mut out = String::from('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let code = u32::from(c);
                out.push_str(&format!("\\u{code:04x}"));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn failover_policy_json(policy: &VoiceFailoverPolicy) -> String {
    match policy {
        VoiceFailoverPolicy::Voicemail => "\"Voicemail\"".to_owned(),
        VoiceFailoverPolicy::None => "\"None\"".to_owned(),
        VoiceFailoverPolicy::Forward { number } => {
            format!("{{\"Forward\":{{\"number\":{}}}}}", json_string(number))
        }
    }
}

fn parse_retained_voice_dids(body: Option<&str>) -> Vec<VoiceDid> {
    let Some(body) = body.map(str::trim).filter(|body| !body.is_empty()) else {
        return Vec::new();
    };
    if body == "null" || body == "[]" {
        return Vec::new();
    }
    let Some(items) = json_top_array_objects(body) else {
        return Vec::new();
    };
    let mut dids = Vec::with_capacity(items.len());
    for item in items {
        let Some(number) = json_flat_string(item, "number") else {
            continue;
        };
        if number.trim().is_empty() {
            continue;
        }
        let routed_to = match json_flat_value(item, "routed_to") {
            Some("null") | None => None,
            Some(raw) => json_unquote(raw).filter(|value| !value.is_empty()),
        };
        dids.push(VoiceDid { number, routed_to });
    }
    dids
}

fn parse_retained_voice_shared(body: Option<&str>) -> Option<VoiceSharedOutbound> {
    let body = body?.trim();
    if body.is_empty() || body == "null" || !body.starts_with('{') {
        return None;
    }
    Some(VoiceSharedOutbound {
        caller_id: json_flat_string(body, "caller_id").unwrap_or_default(),
        outbound_trunk: json_flat_string(body, "outbound_trunk").unwrap_or_default(),
    })
}

fn parse_retained_voice_cutover(body: Option<&str>) -> Option<VoiceCutoverStatus> {
    let body = body?.trim();
    if body.is_empty() || body == "null" || !body.starts_with('{') {
        return None;
    }
    Some(VoiceCutoverStatus {
        phase: parse_cutover_phase(json_flat_string(body, "phase")?.as_str())?,
        total_nodes: json_flat_u64(body, "total_nodes")
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
        reprovisioned: json_flat_u64(body, "reprovisioned")
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
        pending_nodes: json_flat_string_array(body, "pending_nodes"),
        shared_outbound_lifted: json_flat_bool(body, "shared_outbound_lifted")?,
        updated_at_s: json_flat_u64(body, "updated_at_s")?,
    })
}

fn parse_cutover_phase(wire: &str) -> Option<VoiceCutoverPhase> {
    match wire {
        "legacy" => Some(VoiceCutoverPhase::Legacy),
        "lifted-shared-outbound" => Some(VoiceCutoverPhase::LiftedSharedOutbound),
        "nodes-reprovisioning" => Some(VoiceCutoverPhase::NodesReprovisioning),
        "cutover-complete" => Some(VoiceCutoverPhase::CutoverComplete),
        _ => None,
    }
}

fn parse_retained_voice_nodes(rows: &[(&str, &str)]) -> Vec<VoiceNodeProjection> {
    let mut nodes = Vec::new();
    for (topic, body) in rows {
        if nodes.len() >= MAX_VOICE_NODE_ROWS {
            break;
        }
        let Some(topic_id) = voice_node_topic_id(topic) else {
            continue;
        };
        if let Some(node) = parse_voice_node_body(topic_id, body) {
            nodes.push(node);
        }
    }
    nodes.sort_by(|left, right| {
        left.hostname
            .cmp(&right.hostname)
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    nodes
}

fn voice_node_topic_id(topic: &str) -> Option<&str> {
    let topic = topic.trim();
    if topic == VOICE_DIDS_STATE_TOPIC
        || topic == VOICE_SHARED_STATE_TOPIC
        || topic == VOICE_CUTOVER_STATE_TOPIC
    {
        return None;
    }
    if let Some(suffix) = topic.strip_prefix(VOICE_STATE_PREFIX) {
        if suffix.is_empty() || suffix == VOICE_HUD_STATUS_SUFFIX || suffix.contains('/') {
            return None;
        }
        return Some(suffix);
    }
    if topic.is_empty() || topic == VOICE_HUD_STATUS_SUFFIX || topic.contains('/') {
        return None;
    }
    Some(topic)
}

fn parse_voice_node_body(topic_id: &str, body: &str) -> Option<VoiceNodeProjection> {
    let body = body.trim();
    if body.is_empty() || body == "null" || !body.starts_with('{') {
        return None;
    }
    let node_id = json_flat_string(body, "node_id").unwrap_or_else(|| topic_id.to_owned());
    if node_id.trim().is_empty() {
        return None;
    }
    Some(VoiceNodeProjection {
        node_id,
        hostname: json_flat_string(body, "hostname").unwrap_or_default(),
        username: json_flat_string(body, "username").unwrap_or_default(),
        sip_uri: json_flat_string(body, "sip_uri").unwrap_or_default(),
        reg_state: parse_voice_reg_state(body)?,
        routed_dids: json_flat_string_array(body, "routed_dids"),
        failover: parse_voice_failover(body),
        updated_at_s: json_flat_u64(body, "updated_at_s")?,
    })
}

fn parse_voice_reg_state(body: &str) -> Option<VoiceRegState> {
    match json_flat_string(body, "state")?.as_str() {
        "registered" => Some(VoiceRegState::Registered),
        "unregistered" => Some(VoiceRegState::Unregistered),
        "provisioning" => Some(VoiceRegState::Provisioning),
        "error" => Some(VoiceRegState::Error {
            reason: json_flat_string(body, "reason").unwrap_or_default(),
        }),
        _ => None,
    }
}

fn parse_voice_failover(body: &str) -> Option<VoiceFailoverPolicy> {
    match json_flat_value(body, "failover")? {
        "null" => None,
        raw if raw.starts_with('"') => match json_unquote(raw)?.as_str() {
            "Voicemail" => Some(VoiceFailoverPolicy::Voicemail),
            "None" => Some(VoiceFailoverPolicy::None),
            _ => None,
        },
        raw if raw.starts_with('{') => {
            let number = json_flat_string(raw, "number")?;
            Some(VoiceFailoverPolicy::Forward { number })
        }
        _ => None,
    }
}

fn json_flat_string_array(body: &str, key: &str) -> Vec<String> {
    json_flat_value(body, key)
        .map(json_string_array_items)
        .unwrap_or_default()
}

fn json_string_array_items(array: &str) -> Vec<String> {
    let Some(inner) = array
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = inner.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] != b'"' {
            break;
        }
        let rest = &inner[i..];
        let mut end = 1;
        let rest_bytes = rest.as_bytes();
        while end < rest_bytes.len() {
            match rest_bytes[end] {
                b'\\' if end + 1 < rest_bytes.len() => end += 2,
                b'"' => {
                    if let Some(value) = json_unquote(&rest[..=end]) {
                        out.push(value);
                    }
                    i += end + 1;
                    break;
                }
                _ => end += 1,
            }
        }
        if end >= rest_bytes.len() {
            break;
        }
    }
    out
}

fn json_top_array_objects(body: &str) -> Option<Vec<&str>> {
    let inner = body.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = inner.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] != b'{' {
            return None;
        }
        let end = json_balanced_end(inner, i, b'{', b'}')?;
        out.push(&inner[i..=end]);
        i = end + 1;
    }
    Some(out)
}

fn json_flat_value<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let after = body.find(&needle)? + needle.len();
    let rest = body[after..].trim_start().strip_prefix(':')?.trim_start();
    if rest.starts_with('"') {
        let mut end = 1;
        let bytes = rest.as_bytes();
        while end < bytes.len() {
            match bytes[end] {
                b'\\' if end + 1 < bytes.len() => end += 2,
                b'"' => return Some(&rest[..=end]),
                _ => end += 1,
            }
        }
        return None;
    }
    if rest.starts_with('{') {
        let end = json_balanced_end(rest, 0, b'{', b'}')?;
        return Some(&rest[..=end]);
    }
    if rest.starts_with('[') {
        let end = json_balanced_end(rest, 0, b'[', b']')?;
        return Some(&rest[..=end]);
    }
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn json_balanced_end(body: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = body.as_bytes();
    if bytes.get(start) != Some(&open) {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (idx, &byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

const EMPTY_VOICE_NODES_NOTE: &str = "No state/voice nodes projected.";
const EMPTY_VOICE_DIDS_NOTE: &str = "No master-account DIDs projected.";
const EMPTY_VOICE_SHARED_NOTE: &str = "Shared-outbound is not lifted yet.";
const EMPTY_VOICE_CUTOVER_NOTE: &str = "No cutover status projected.";

/// Honest empty-state copy for missing `state/voice/*` projections. Never
/// invents demo rows — an empty snapshot yields every note.
#[must_use]
pub fn voice_projection_empty_notes(
    nodes: &[VoiceNodeProjection],
    dids: &[VoiceDid],
    shared: Option<&VoiceSharedOutbound>,
    cutover: Option<&VoiceCutoverStatus>,
) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if nodes.is_empty() {
        notes.push(EMPTY_VOICE_NODES_NOTE);
    }
    if dids.is_empty() {
        notes.push(EMPTY_VOICE_DIDS_NOTE);
    }
    if shared.is_none() {
        notes.push(EMPTY_VOICE_SHARED_NOTE);
    }
    if cutover.is_none() {
        notes.push(EMPTY_VOICE_CUTOVER_NOTE);
    }
    notes
}

/// Draft fields for the fleet voice-admin form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceAdminFormState {
    did: String,
    route_node: String,
    failover_node: String,
    failover_kind: usize,
    failover_number: String,
    caller_id: String,
    outbound_trunk: String,
    shared_seeded: bool,
    confirm_cutover: bool,
    notice: Option<String>,
}

impl VoiceAdminFormState {
    /// A write draft the apply path (and tests) can fill without painting.
    #[cfg(test)]
    #[must_use]
    pub fn draft(
        did: impl Into<String>,
        route_node: impl Into<String>,
        failover_node: impl Into<String>,
        failover_kind: usize,
        failover_number: impl Into<String>,
        caller_id: impl Into<String>,
        outbound_trunk: impl Into<String>,
    ) -> Self {
        Self {
            did: did.into(),
            route_node: route_node.into(),
            failover_node: failover_node.into(),
            failover_kind,
            failover_number: failover_number.into(),
            caller_id: caller_id.into(),
            outbound_trunk: outbound_trunk.into(),
            shared_seeded: false,
            confirm_cutover: false,
            notice: None,
        }
    }
}

/// Draft fields for the SIP-gateway form. `password` is write-only: it is
/// never seeded from a readout and is wiped after a successful set or clear.
#[derive(Clone, Default)]
pub struct GatewayFormState {
    host: String,
    port: String,
    username: String,
    password: String,
    display_name: String,
    expires: String,
    seeded: bool,
    seeded_present: bool,
    confirm_clear: bool,
    notice: Option<String>,
}

impl GatewayFormState {
    /// A write draft. `password` is accepted here so the set verb can carry it;
    /// it is never used as readout copy.
    #[cfg(test)]
    #[must_use]
    pub fn draft(
        host: impl Into<String>,
        port: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
        display_name: impl Into<String>,
        expires: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port: port.into(),
            username: username.into(),
            password: password.into(),
            display_name: display_name.into(),
            expires: expires.into(),
            seeded: true,
            seeded_present: false,
            confirm_clear: false,
            notice: None,
        }
    }
}

impl std::fmt::Debug for GatewayFormState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayFormState")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("display_name", &self.display_name)
            .field("expires", &self.expires)
            .field("seeded", &self.seeded)
            .field("confirm_clear", &self.confirm_clear)
            .field("notice", &self.notice)
            .finish()
    }
}

fn voice_admin_form_state(ui: &egui::Ui) -> VoiceAdminFormState {
    let id = ui.id().with("voice-admin-form");
    ui.ctx()
        .data_mut(|data| data.get_temp::<VoiceAdminFormState>(id))
        .unwrap_or_default()
}

fn store_voice_admin_form_state(ui: &egui::Ui, state: VoiceAdminFormState) {
    let id = ui.id().with("voice-admin-form");
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
}

fn seed_voice_shared_form_state(
    mut form: VoiceAdminFormState,
    shared: Option<&VoiceSharedOutbound>,
) -> VoiceAdminFormState {
    if !form.shared_seeded {
        if let Some(config) = shared {
            if form.caller_id.trim().is_empty() {
                form.caller_id.clone_from(&config.caller_id);
            }
            if form.outbound_trunk.trim().is_empty() {
                form.outbound_trunk.clone_from(&config.outbound_trunk);
            }
            form.shared_seeded = true;
        }
    }
    form
}

/// Admit one fleet voice-admin intent. Provision always publishes. DID route,
/// failover, shared-config, and cutover refuse honestly when `state/voice/*`
/// projections have no provisioned sub-account.
pub fn apply_voice_admin(
    form: &mut VoiceAdminFormState,
    nodes: &[VoiceNodeProjection],
    dids: &[VoiceDid],
    cutover: Option<&VoiceCutoverStatus>,
    sink: &mut VoiceAdminSink,
    intent: VoiceAdminFormIntent,
) -> VoiceAdminFormOutcome {
    match intent {
        VoiceAdminFormIntent::Provision => {
            let command = VoiceAdminCommand::Provision;
            sink.emit(command.clone());
            form.notice = Some("Provision published".to_owned());
            VoiceAdminFormOutcome::Published(command)
        }
        VoiceAdminFormIntent::DidRoute => apply_voice_did_route(form, nodes, dids, sink, false),
        VoiceAdminFormIntent::Unroute => apply_voice_did_route(form, nodes, dids, sink, true),
        VoiceAdminFormIntent::Failover => apply_voice_failover(form, nodes, sink),
        VoiceAdminFormIntent::SharedConfig => apply_voice_shared_config(form, nodes, sink),
        VoiceAdminFormIntent::Cutover => apply_voice_cutover(form, nodes, cutover, sink),
        VoiceAdminFormIntent::CancelCutover => {
            form.confirm_cutover = false;
            VoiceAdminFormOutcome::CancelledCutover
        }
    }
}

fn refuse_unprovisioned_voice(form: &mut VoiceAdminFormState) -> VoiceAdminFormOutcome {
    form.notice = Some(VoiceAdminRefuse::NoProvisionedAccount.label().to_owned());
    VoiceAdminFormOutcome::Refused(VoiceAdminRefuse::NoProvisionedAccount)
}

fn apply_voice_did_route(
    form: &mut VoiceAdminFormState,
    nodes: &[VoiceNodeProjection],
    dids: &[VoiceDid],
    sink: &mut VoiceAdminSink,
    unroute: bool,
) -> VoiceAdminFormOutcome {
    if !has_provisioned_voice_account(nodes) {
        return refuse_unprovisioned_voice(form);
    }
    // Empty node identity is an unknown node, but invalid DIDs must refuse
    // first so the operator sees the DID problem rather than a missing node.
    let target = if unroute {
        None
    } else {
        Some(form.route_node.trim())
    };
    let pending = sink.pending_did_routes();
    match validate_did_route(&form.did, target, dids, nodes, &pending) {
        Ok(command) => {
            sink.emit(command.clone());
            form.notice = Some(if unroute {
                "DID unroute published".to_owned()
            } else {
                "DID route published".to_owned()
            });
            VoiceAdminFormOutcome::Published(command)
        }
        Err(refuse) => {
            form.notice = Some(refuse.label().to_owned());
            VoiceAdminFormOutcome::Refused(refuse)
        }
    }
}

fn apply_voice_failover(
    form: &mut VoiceAdminFormState,
    nodes: &[VoiceNodeProjection],
    sink: &mut VoiceAdminSink,
) -> VoiceAdminFormOutcome {
    if !has_provisioned_voice_account(nodes) {
        return refuse_unprovisioned_voice(form);
    }
    let policy = match form.failover_kind {
        1 => VoiceFailoverPolicy::Forward {
            number: form.failover_number.clone(),
        },
        2 => VoiceFailoverPolicy::None,
        _ => VoiceFailoverPolicy::Voicemail,
    };
    match validate_failover(&form.failover_node, policy, nodes) {
        Ok(command) => {
            sink.emit(command.clone());
            form.notice = Some("Failover published".to_owned());
            VoiceAdminFormOutcome::Published(command)
        }
        Err(refuse) => {
            form.notice = Some(refuse.label().to_owned());
            VoiceAdminFormOutcome::Refused(refuse)
        }
    }
}

fn apply_voice_shared_config(
    form: &mut VoiceAdminFormState,
    nodes: &[VoiceNodeProjection],
    sink: &mut VoiceAdminSink,
) -> VoiceAdminFormOutcome {
    if !has_provisioned_voice_account(nodes) {
        return refuse_unprovisioned_voice(form);
    }
    match validate_shared_config(&form.caller_id, &form.outbound_trunk) {
        Ok(command) => {
            sink.emit(command.clone());
            form.notice = Some("Shared-outbound published".to_owned());
            VoiceAdminFormOutcome::Published(command)
        }
        Err(refuse) => {
            form.notice = Some(refuse.label().to_owned());
            VoiceAdminFormOutcome::Refused(refuse)
        }
    }
}

fn apply_voice_cutover(
    form: &mut VoiceAdminFormState,
    nodes: &[VoiceNodeProjection],
    cutover: Option<&VoiceCutoverStatus>,
    sink: &mut VoiceAdminSink,
) -> VoiceAdminFormOutcome {
    if !has_provisioned_voice_account(nodes) {
        form.confirm_cutover = false;
        return refuse_unprovisioned_voice(form);
    }
    if !form.confirm_cutover {
        form.confirm_cutover = true;
        return VoiceAdminFormOutcome::ArmedCutover;
    }
    let outcome = match validate_cutover(cutover, nodes) {
        Ok(command) => {
            sink.emit(command.clone());
            form.notice = Some("Cutover published".to_owned());
            VoiceAdminFormOutcome::Published(command)
        }
        Err(refuse) => {
            form.notice = Some(refuse.label().to_owned());
            VoiceAdminFormOutcome::Refused(refuse)
        }
    };
    form.confirm_cutover = false;
    outcome
}

fn gateway_admin_form_state(ui: &egui::Ui, readout: Option<&GatewayReadout>) -> GatewayFormState {
    let id = ui.id().with("gateway-admin-form");
    let mut state = ui
        .ctx()
        .data_mut(|data| data.get_temp::<GatewayFormState>(id))
        .unwrap_or_default();
    let present = readout.is_some_and(|row| row.present);
    if !state.seeded || (present && !state.seeded_present) {
        if let Some(row) = readout.filter(|row| row.present) {
            state.host = row.host.clone();
            state.port = row.port.to_string();
            state.username = row.username.clone();
            state.display_name = row.display_name.clone();
            state.expires = row.expires.to_string();
            // Never seed the password from a readout — it is always redacted.
            state.password.clear();
        }
        state.seeded = true;
    }
    state.seeded_present = present;
    state
}

fn store_gateway_admin_form_state(ui: &egui::Ui, state: GatewayFormState) {
    let id = ui.id().with("gateway-admin-form");
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
}

fn voice_admin_panel(
    ui: &mut egui::Ui,
    data: &dyn crate::CollabData,
    admin: &dyn ActivityAdminData,
    sink: &mut VoiceAdminSink,
) {
    let nodes = admin.voice_nodes();
    let dids = admin.voice_dids();
    let provisioned = has_provisioned_voice_account(nodes);
    let form = voice_admin_form_state(ui);
    let mut form = seed_voice_shared_form_state(form, admin.voice_shared());

    widgets::section().show(ui, |ui| {
        widgets::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(
                    ui,
                    "audio-volume-high",
                    Style::SP_M,
                    theme_color(ui, Style::ACCENT),
                );
                ui.label(
                    Style::typography_text("Fleet voice", TypographyRole::Headline)
                        .color(theme_color(ui, Style::TEXT_STRONG)),
                );
            });
            widgets::muted_note(
                ui,
                "Leader/operator console. Verbs publish locally; the shell drains them onto action/voice/*.",
            );

            voice_fleet_board(ui, data, nodes);
            ui.add_space(Style::SP_S);

            ui.add_space(Style::SP_S);
            if ui
                .button("Provision / Re-provision")
                .comms_hover_text("Force an immediate reconcile pass for every enrolled node")
                .clicked()
            {
                let _ = apply_voice_admin(
                    &mut form,
                    nodes,
                    dids,
                    admin.voice_cutover(),
                    sink,
                    VoiceAdminFormIntent::Provision,
                );
            }

            if !provisioned {
                ui.add_space(Style::SP_S);
                widgets::WorkspaceStatePanel::new(
                    widgets::WorkspaceState::Empty,
                    voice_unprovisioned_headline(nodes),
                    voice_unprovisioned_detail(nodes),
                )
                .show(ui, |_| {});
                for note in voice_projection_empty_notes(
                    nodes,
                    dids,
                    admin.voice_shared(),
                    admin.voice_cutover(),
                )
                .into_iter()
                .filter(|note| *note != EMPTY_VOICE_NODES_NOTE)
                {
                    widgets::muted_note(ui, note);
                }
                if let Some(notice) = form.notice.as_deref() {
                    widgets::muted_note(ui, notice);
                }
                return;
            }

            ui.add_space(Style::SP_S);
            // Keep the retained migration projection above the longer action
            // forms. The Activity body is hosted in a bounded shell viewport,
            // so status must be visible before the controls can extend below
            // the first frame; the controls and their verb paths stay intact.
            voice_cutover(ui, &mut form, admin.voice_cutover(), nodes, data, sink);
            ui.add_space(Style::SP_S);
            voice_did_routing(ui, &mut form, dids, nodes, sink);
            ui.add_space(Style::SP_S);
            voice_failover(ui, &mut form, nodes, sink);
            ui.add_space(Style::SP_S);
            voice_shared_outbound(ui, &mut form, admin.voice_shared(), nodes, sink);

            if let Some(notice) = form.notice.as_deref() {
                ui.add_space(Style::SP_XS);
                widgets::muted_note(ui, notice);
            }
        });
    });

    store_voice_admin_form_state(ui, form);
}

fn voice_fleet_board(
    ui: &mut egui::Ui,
    data: &dyn crate::CollabData,
    nodes: &[VoiceNodeProjection],
) {
    ui.label(
        Style::typography_text("Fleet board", TypographyRole::Title)
            .color(theme_color(ui, Style::TEXT_STRONG)),
    );
    if nodes.is_empty() {
        widgets::WorkspaceStatePanel::new(
            widgets::WorkspaceState::Empty,
            "No state/voice nodes projected",
            EMPTY_VOICE_NODES_NOTE,
        )
        .show(ui, |_| {});
        return;
    }
    for node in nodes {
        if !node.sip_uri.is_empty() {
            widgets::field(
                ui,
                &node.hostname,
                &node.sip_uri,
                theme_color(ui, Style::TEXT),
            );
        }
    }
    let mut list = widgets::DenseList::new();
    list.header(ui, |ui| {
        ui.label(
            Style::typography_text("Node", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.label(
            Style::typography_text("State", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.label(
            Style::typography_text("SIP", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.label(
            Style::typography_text("DIDs", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.label(
            Style::typography_text("Failover", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.label(
            Style::typography_text("Age", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
    });
    let now_ms = data.now_unix_ms();
    for node in nodes {
        list.row(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::status_dot(ui, theme_color(ui, voice_reg_tone(&node.reg_state)));
                ui.label(
                    Style::typography_text(&node.hostname, TypographyRole::Caption)
                        .color(theme_color(ui, Style::TEXT)),
                );
                if node.node_id != node.hostname {
                    ui.label(
                        Style::typography_text(&node.node_id, TypographyRole::Caption)
                            .color(theme_color(ui, Style::TEXT_DIM)),
                    );
                }
                let state_label = match &node.reg_state {
                    VoiceRegState::Error { reason } => format!("error: {reason}"),
                    other => other.wire_tag().to_owned(),
                };
                ui.label(
                    Style::typography_text(state_label, TypographyRole::Caption)
                        .color(theme_color(ui, voice_reg_tone(&node.reg_state))),
                );
                ui.label(
                    Style::typography_text(&node.sip_uri, TypographyRole::Caption)
                        .color(theme_color(ui, Style::TEXT)),
                );
                let dids = if node.routed_dids.is_empty() {
                    "—".to_owned()
                } else {
                    node.routed_dids.join(", ")
                };
                ui.label(
                    Style::typography_text(dids, TypographyRole::Caption)
                        .color(theme_color(ui, Style::TEXT)),
                );
                let failover = node
                    .failover
                    .as_ref()
                    .map_or_else(|| "—".to_owned(), VoiceFailoverPolicy::label);
                ui.label(
                    Style::typography_text(failover, TypographyRole::Caption)
                        .color(theme_color(ui, Style::TEXT)),
                );
                let then_ms = i64::try_from(node.updated_at_s.saturating_mul(1000)).unwrap_or(0);
                ui.label(
                    Style::typography_text(relative_age(now_ms, then_ms), TypographyRole::Caption)
                        .color(theme_color(ui, Style::TEXT_DIM)),
                );
            });
        });
    }
}

fn voice_reg_tone(state: &VoiceRegState) -> egui::Color32 {
    match state {
        VoiceRegState::Registered => Style::OK,
        VoiceRegState::Unregistered => Style::WARN,
        VoiceRegState::Provisioning => Style::ACCENT,
        VoiceRegState::Error { .. } => Style::DANGER,
    }
}

fn voice_did_routing(
    ui: &mut egui::Ui,
    form: &mut VoiceAdminFormState,
    dids: &[VoiceDid],
    nodes: &[VoiceNodeProjection],
    sink: &mut VoiceAdminSink,
) {
    ui.label(
        Style::typography_text("DID routing", TypographyRole::Title)
            .color(theme_color(ui, Style::TEXT_STRONG)),
    );
    if dids.is_empty() {
        widgets::muted_note(ui, EMPTY_VOICE_DIDS_NOTE);
    } else {
        let mut list = widgets::DenseList::new();
        for did in dids {
            list.row(ui, |ui| {
                widgets::field(
                    ui,
                    &did.number,
                    did.routed_to.as_deref().unwrap_or("main account"),
                    theme_color(ui, Style::TEXT),
                );
            });
        }
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(
            Style::typography_text("DID", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.did)
                .desired_width(Style::SP_XL * 6.0)
                .hint_text("15551234567"),
        );
        ui.label(
            Style::typography_text("Node", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.route_node)
                .desired_width(Style::SP_XL * 6.0)
                .hint_text("peer:eagle or hostname"),
        );
        if ui.button("Route DID").clicked() {
            let _ = apply_voice_admin(
                form,
                nodes,
                dids,
                None,
                sink,
                VoiceAdminFormIntent::DidRoute,
            );
        }
        if ui
            .button("Unroute")
            .comms_hover_text("Return this DID to the master account main line")
            .clicked()
        {
            let _ = apply_voice_admin(form, nodes, dids, None, sink, VoiceAdminFormIntent::Unroute);
        }
    });
}

fn voice_failover(
    ui: &mut egui::Ui,
    form: &mut VoiceAdminFormState,
    nodes: &[VoiceNodeProjection],
    sink: &mut VoiceAdminSink,
) {
    ui.label(
        Style::typography_text("Failover policy", TypographyRole::Title)
            .color(theme_color(ui, Style::TEXT_STRONG)),
    );
    ui.horizontal_wrapped(|ui| {
        ui.label(
            Style::typography_text("Node", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.failover_node)
                .desired_width(Style::SP_XL * 6.0)
                .hint_text("peer:eagle or hostname"),
        );
        for (index, label) in ["Voicemail", "Forward", "None"].into_iter().enumerate() {
            if ui
                .selectable_label(form.failover_kind == index, label)
                .clicked()
            {
                form.failover_kind = index;
            }
        }
        if form.failover_kind == 1 {
            ui.add(
                egui::TextEdit::singleline(&mut form.failover_number)
                    .desired_width(Style::SP_XL * 5.0)
                    .hint_text("forward number"),
            );
        }
        if ui.button("Apply failover").clicked() {
            let _ = apply_voice_admin(form, nodes, &[], None, sink, VoiceAdminFormIntent::Failover);
        }
    });
}

fn voice_shared_outbound(
    ui: &mut egui::Ui,
    form: &mut VoiceAdminFormState,
    shared: Option<&VoiceSharedOutbound>,
    nodes: &[VoiceNodeProjection],
    sink: &mut VoiceAdminSink,
) {
    ui.label(
        Style::typography_text("Shared outbound", TypographyRole::Title)
            .color(theme_color(ui, Style::TEXT_STRONG)),
    );
    match shared {
        Some(config) => {
            widgets::field(
                ui,
                "Caller ID in force",
                &config.caller_id,
                theme_color(ui, Style::TEXT),
            );
            widgets::field(
                ui,
                "Trunk in force",
                &config.outbound_trunk,
                theme_color(ui, Style::TEXT),
            );
        }
        None => {
            widgets::muted_note(ui, EMPTY_VOICE_SHARED_NOTE);
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(
            Style::typography_text("Caller ID", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.caller_id)
                .desired_width(Style::SP_XL * 5.0)
                .hint_text("15551234567"),
        );
        ui.label(
            Style::typography_text("Trunk", TypographyRole::Caption)
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.outbound_trunk)
                .desired_width(Style::SP_XL * 5.0)
                .hint_text("main"),
        );
        if ui
            .button("Apply to fleet")
            .comms_hover_text("Publish action/voice/shared-config")
            .clicked()
        {
            let _ = apply_voice_admin(
                form,
                nodes,
                &[],
                None,
                sink,
                VoiceAdminFormIntent::SharedConfig,
            );
        }
    });
}

fn voice_cutover(
    ui: &mut egui::Ui,
    form: &mut VoiceAdminFormState,
    cutover: Option<&VoiceCutoverStatus>,
    nodes: &[VoiceNodeProjection],
    data: &dyn crate::CollabData,
    sink: &mut VoiceAdminSink,
) {
    ui.label(
        Style::typography_text("Cutover", TypographyRole::Title)
            .color(theme_color(ui, Style::TEXT_STRONG)),
    );
    match cutover {
        Some(status) => {
            widgets::field(
                ui,
                "Phase",
                status.phase.headline(),
                theme_color(ui, Style::TEXT),
            );
            widgets::field(
                ui,
                "Progress",
                &format!("{}/{} nodes", status.reprovisioned, status.total_nodes),
                theme_color(ui, Style::TEXT),
            );
            let pending = if status.pending_nodes.is_empty() {
                "none".to_owned()
            } else {
                status.pending_nodes.join(", ")
            };
            widgets::field(ui, "Pending", &pending, theme_color(ui, Style::TEXT));
            let then_ms = i64::try_from(status.updated_at_s.saturating_mul(1000)).unwrap_or(0);
            widgets::field(
                ui,
                "Freshness",
                &relative_age(data.now_unix_ms(), then_ms),
                theme_color(ui, Style::TEXT_DIM),
            );
        }
        None => {
            widgets::muted_note(ui, EMPTY_VOICE_CUTOVER_NOTE);
        }
    }
    ui.horizontal(|ui| {
        if form.confirm_cutover {
            if ui
                .button("Confirm cutover")
                .comms_hover_text("Force remaining nodes onto the split model")
                .clicked()
            {
                let _ = apply_voice_admin(
                    form,
                    nodes,
                    &[],
                    cutover,
                    sink,
                    VoiceAdminFormIntent::Cutover,
                );
            }
            if ui.button("Cancel").clicked() {
                let _ = apply_voice_admin(
                    form,
                    nodes,
                    &[],
                    cutover,
                    sink,
                    VoiceAdminFormIntent::CancelCutover,
                );
            }
        } else if ui.button("Continue cutover").clicked() {
            let _ = apply_voice_admin(
                form,
                nodes,
                &[],
                cutover,
                sink,
                VoiceAdminFormIntent::Cutover,
            );
        }
    });
}

fn gateway_admin_panel(ui: &mut egui::Ui, admin: &dyn ActivityAdminData, sink: &mut GatewaySink) {
    let readout = admin.gateway();
    let mut form = gateway_admin_form_state(ui, readout);

    widgets::section().show(ui, |ui| {
        widgets::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(ui, "globe", Style::SP_M, theme_color(ui, Style::ACCENT));
                ui.label(
                    Style::typography_text("SIP gateway", TypographyRole::Headline)
                        .color(theme_color(ui, Style::TEXT_STRONG)),
                );
            });
            widgets::muted_note(
                ui,
                "Mesh-wide outbound registrar. The write path never echoes the password; get-gateway is redacted.",
            );

            ui.add_space(Style::SP_S);
            gateway_readout(ui, readout);

            ui.add_space(Style::SP_S);
            ui.horizontal_wrapped(|ui| {
                labeled_field(ui, "Host", &mut form.host, "pbx.example.com");
                labeled_field(ui, "Port", &mut form.port, "5060");
                labeled_field(ui, "Username", &mut form.username, "alice");
                ui.label(
                    Style::typography_text("Password", TypographyRole::Caption)
                        .color(theme_color(ui, Style::TEXT_DIM)),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut form.password)
                        .password(true)
                        .desired_width(Style::SP_XL * 5.0)
                        .hint_text("unchanged if blank"),
                );
                labeled_field(ui, "Display name", &mut form.display_name, "optional");
                labeled_field(ui, "Expires", &mut form.expires, "3600");
            });

            ui.horizontal_wrapped(|ui| {
                if ui.button("Set gateway").clicked() {
                    let _ = apply_gateway_form(&mut form, readout, sink, GatewayFormIntent::Set);
                }
                if ui
                    .button("Refresh")
                    .comms_hover_text("Publish get-gateway")
                    .clicked()
                {
                    let _ = apply_gateway_form(&mut form, readout, sink, GatewayFormIntent::Get);
                }
                if form.confirm_clear {
                    if ui
                        .button("Confirm clear gateway")
                        .comms_hover_text("Remove gateway.toml and revert the mesh to P2P")
                        .clicked()
                    {
                        let _ =
                            apply_gateway_form(&mut form, readout, sink, GatewayFormIntent::Clear);
                    }
                    if ui.button("Cancel clear").clicked() {
                        let _ = apply_gateway_form(
                            &mut form,
                            readout,
                            sink,
                            GatewayFormIntent::CancelClear,
                        );
                    }
                } else if ui.button("Clear gateway").clicked() {
                    let _ = apply_gateway_form(&mut form, readout, sink, GatewayFormIntent::Clear);
                }
            });

            if let Some(notice) = form.notice.as_deref() {
                ui.add_space(Style::SP_XS);
                widgets::muted_note(ui, notice);
            }
        });
    });

    store_gateway_admin_form_state(ui, form);
}

fn labeled_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(
        Style::typography_text(label, TypographyRole::Caption)
            .color(theme_color(ui, Style::TEXT_DIM)),
    );
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(Style::SP_XL * 5.0)
            .hint_text(hint),
    );
}

fn parse_optional_u64(raw: &str) -> Result<Option<u64>, ()> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<u64>().map(Some).map_err(|_| ())
}

fn gateway_readout(ui: &mut egui::Ui, readout: Option<&GatewayReadout>) {
    match readout {
        Some(row) if row.present => {
            widgets::field(ui, "Present", "true", theme_color(ui, Style::OK));
            widgets::field(ui, "Host", &row.host, theme_color(ui, Style::TEXT));
            widgets::field(
                ui,
                "Port",
                &row.port.to_string(),
                theme_color(ui, Style::TEXT),
            );
            widgets::field(ui, "Username", &row.username, theme_color(ui, Style::TEXT));
            widgets::field(
                ui,
                "Password",
                row.redacted_password(),
                theme_color(ui, Style::TEXT_DIM),
            );
            widgets::field(
                ui,
                "Password set",
                if row.password_set { "true" } else { "false" },
                theme_color(ui, Style::TEXT),
            );
            if !row.display_name.is_empty() {
                widgets::field(
                    ui,
                    "Display name",
                    &row.display_name,
                    theme_color(ui, Style::TEXT),
                );
            }
            widgets::field(
                ui,
                "Expires",
                &row.expires.to_string(),
                theme_color(ui, Style::TEXT_DIM),
            );
        }
        _ => {
            widgets::WorkspaceStatePanel::new(
                widgets::WorkspaceState::Empty,
                "No SIP gateway configured",
                "Host, port, and credentials stay empty until set-gateway lands a registrar.",
            )
            .show(ui, |_| {});
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mde_collab_types::{ActorClock, ActorId, AlertPayload, AlertView, EventId};

    use super::{
        apply_gateway_form, apply_voice_admin, coalesced_activity_rows,
        has_provisioned_voice_account, hydrate_gateway_readout, resolve_voice_node,
        seed_voice_shared_form_state, validate_cutover, validate_did_route, validate_failover,
        validate_gateway_clear, validate_gateway_set, validate_shared_config,
        voice_projection_empty_notes, voice_unprovisioned_detail, voice_unprovisioned_headline,
        workgroup_gateway_toml_path, ActivityAdminSnapshot, ActivityEntry, ActivityFilter,
        AlertInbox, GatewayCommand, GatewayFormIntent, GatewayFormOutcome, GatewayFormState,
        GatewayReadout, GatewayRefuse, GatewaySink, Severity, SpaceId, VoiceAdminCommand,
        VoiceAdminFormIntent, VoiceAdminFormOutcome, VoiceAdminFormState, VoiceAdminRefuse,
        VoiceAdminSink, VoiceCutoverPhase, VoiceCutoverStatus, VoiceDid, VoiceFailoverPolicy,
        VoiceNodeProjection, VoiceRegState, VoiceSharedOutbound, VOICE_DID_ROUTE_TOPIC,
        VOICE_FAILOVER_TOPIC, VOICE_PROVISION_TOPIC, VOICE_SHARED_CONFIG_TOPIC,
        VOIP_CLEAR_GATEWAY_TOPIC, VOIP_GET_GATEWAY_TOPIC, VOIP_SET_GATEWAY_TOPIC,
    };

    fn entry(
        event_id: EventId,
        space: SpaceId,
        actor: &ActorId,
        created_unix_ms: i64,
        kind_tag: &str,
        summary: &str,
    ) -> ActivityEntry {
        ActivityEntry {
            event_id,
            space,
            actor: actor.clone(),
            clock: ActorClock::at(created_unix_ms.max(0) as u64, 0),
            created_unix_ms,
            kind_tag: kind_tag.to_owned(),
            summary: summary.to_owned(),
        }
    }

    fn alert(event_id: EventId, space: SpaceId, severity: Severity) -> AlertView {
        AlertView {
            event_id,
            space,
            alert: AlertPayload {
                severity,
                source: "test-source".to_owned(),
                headline: "test alert".to_owned(),
                fields: BTreeMap::new(),
                actions: Vec::new(),
                goto: None,
            },
            acknowledged: false,
            snoozed_until_unix_ms: None,
        }
    }

    #[test]
    fn coalesces_adjacent_repeats_and_keeps_truthful_count() {
        let space = SpaceId::new();
        let actor = ActorId::new("eagle");
        let entries = vec![
            entry(
                EventId::new(),
                space,
                &actor,
                10_000,
                "alert_raised",
                "disk warning",
            ),
            entry(
                EventId::new(),
                space,
                &actor,
                10_001,
                "alert_raised",
                "disk warning",
            ),
            entry(
                EventId::new(),
                space,
                &actor,
                10_002,
                "alert_raised",
                "disk warning",
            ),
        ];

        let rows = coalesced_activity_rows(&entries, ActivityFilter::All, None);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows.rows[0].count(), 3);
        assert_eq!(rows.rows[0].entry(), &entries[0]);
    }

    #[test]
    fn severity_change_breaks_an_alert_repeat_group() {
        let space = SpaceId::new();
        let actor = ActorId::new("eagle");
        let info_id = EventId::new();
        let critical_id = EventId::new();
        let entries = vec![
            entry(
                info_id,
                space,
                &actor,
                10_000,
                "alert_raised",
                "disk warning",
            ),
            entry(
                critical_id,
                space,
                &actor,
                10_001,
                "alert_raised",
                "disk warning",
            ),
        ];
        let inbox = AlertInbox {
            alerts: vec![
                alert(info_id, space, Severity::Info),
                alert(critical_id, space, Severity::Critical),
            ],
        };

        let rows = coalesced_activity_rows(&entries, ActivityFilter::Alerts, Some(&inbox));

        assert_eq!(rows.len(), 2);
        assert_eq!(rows.rows[0].severity(), Some(Severity::Info));
        assert_eq!(rows.rows[1].severity(), Some(Severity::Critical));
    }

    #[test]
    fn repeat_after_the_bounded_window_stays_a_new_row() {
        let space = SpaceId::new();
        let actor = ActorId::new("eagle");
        let entries = vec![
            entry(
                EventId::new(),
                space,
                &actor,
                10_000,
                "message_posted",
                "same",
            ),
            entry(
                EventId::new(),
                space,
                &actor,
                10_000 + 5 * 60 * 1_000,
                "message_posted",
                "same",
            ),
        ];

        let rows = coalesced_activity_rows(&entries, ActivityFilter::Messages, None);

        assert_eq!(rows.len(), 2);
        assert!(rows.rows.iter().all(|row| row.count() == 1));
    }

    fn provisioned_node(id: &str, host: &str, username: &str) -> VoiceNodeProjection {
        VoiceNodeProjection {
            node_id: id.to_owned(),
            hostname: host.to_owned(),
            username: username.to_owned(),
            sip_uri: format!("{username}@sip.vitelity.net"),
            reg_state: VoiceRegState::Unregistered,
            routed_dids: Vec::new(),
            failover: None,
            updated_at_s: 1_700_000_000,
        }
    }

    fn inventory(number: &str, routed_to: Option<&str>) -> VoiceDid {
        VoiceDid {
            number: number.to_owned(),
            routed_to: routed_to.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn verb_bodies_match_the_worker_and_responder_shapes() {
        let provision = VoiceAdminCommand::Provision;
        assert_eq!(provision.topic(), VOICE_PROVISION_TOPIC);
        assert_eq!(provision.json_body(), "{}");

        let route = VoiceAdminCommand::DidRoute {
            did: "15551234567".to_owned(),
            node_id: Some("peer:eagle".to_owned()),
        };
        assert_eq!(route.topic(), VOICE_DID_ROUTE_TOPIC);
        assert_eq!(
            route.json_body(),
            r#"{"did":"15551234567","node_id":"peer:eagle"}"#
        );

        let unroute = VoiceAdminCommand::DidRoute {
            did: "15551234567".to_owned(),
            node_id: None,
        };
        assert_eq!(
            unroute.json_body(),
            r#"{"did":"15551234567","node_id":null}"#
        );

        let failover = VoiceAdminCommand::Failover {
            node_id: "peer:eagle".to_owned(),
            policy: VoiceFailoverPolicy::Forward {
                number: "15557654321".to_owned(),
            },
        };
        assert_eq!(failover.topic(), VOICE_FAILOVER_TOPIC);
        assert_eq!(
            failover.json_body(),
            r#"{"node_id":"peer:eagle","policy":{"Forward":{"number":"15557654321"}}}"#
        );

        let shared = VoiceAdminCommand::SharedConfig {
            caller_id: "15551234567".to_owned(),
            outbound_trunk: "main".to_owned(),
        };
        assert_eq!(shared.topic(), VOICE_SHARED_CONFIG_TOPIC);
        assert_eq!(
            shared.json_body(),
            r#"{"caller_id":"15551234567","outbound_trunk":"main"}"#
        );

        let cutover = VoiceAdminCommand::Cutover;
        assert_eq!(cutover.topic(), VOICE_PROVISION_TOPIC);
        assert_eq!(cutover.json_body(), "{}");

        let set = GatewayCommand::Set {
            host: "pbx.example.com".to_owned(),
            port: Some(5062),
            username: "alice".to_owned(),
            password: "s3cret".to_owned(),
            display_name: "Alice".to_owned(),
            expires: Some(3600),
        };
        assert_eq!(set.topic(), VOIP_SET_GATEWAY_TOPIC);
        assert_eq!(
            set.json_body().as_deref(),
            Some(
                r#"{"host":"pbx.example.com","username":"alice","password":"s3cret","display_name":"Alice","port":5062,"expires":3600}"#
            )
        );
        let redacted = set.redacted_json_body().expect("set has a body");
        assert!(
            !redacted.contains("s3cret"),
            "write-path redaction leaked the password: {redacted}"
        );
        assert!(redacted.contains("\"password\":\"\""));
        assert_eq!(GatewayCommand::Get.topic(), VOIP_GET_GATEWAY_TOPIC);
        assert_eq!(GatewayCommand::Get.json_body(), None);
        assert_eq!(GatewayCommand::Clear.topic(), VOIP_CLEAR_GATEWAY_TOPIC);
        assert_eq!(GatewayCommand::Clear.json_body().as_deref(), Some("{}"));
    }

    #[test]
    fn invalid_dids_unknown_nodes_and_conflicts_refuse() {
        let nodes = vec![
            provisioned_node("peer:eagle", "eagle", "eagle"),
            provisioned_node("peer:otter", "otter", "otter"),
        ];
        let dids = vec![inventory("15551234567", Some("eagle"))];

        assert_eq!(
            validate_did_route("not-a-did", Some("peer:eagle"), &dids, &nodes, &[]).unwrap_err(),
            VoiceAdminRefuse::InvalidDid
        );
        assert_eq!(
            validate_did_route("15550001111", Some("peer:eagle"), &dids, &nodes, &[]).unwrap_err(),
            VoiceAdminRefuse::InvalidDid
        );
        assert_eq!(
            validate_did_route("15551234567", Some("peer:ghost"), &dids, &nodes, &[]).unwrap_err(),
            VoiceAdminRefuse::UnknownNode
        );

        let pending = vec![("15551234567".to_owned(), Some("peer:eagle".to_owned()))];
        assert_eq!(
            validate_did_route("15551234567", Some("peer:otter"), &dids, &nodes, &pending)
                .unwrap_err(),
            VoiceAdminRefuse::ConflictingRoute
        );

        let ok = validate_did_route("15551234567", Some("peer:otter"), &dids, &nodes, &[])
            .expect("replacement of an applied route is allowed");
        assert_eq!(
            ok,
            VoiceAdminCommand::DidRoute {
                did: "15551234567".to_owned(),
                node_id: Some("peer:otter".to_owned()),
            }
        );
        let trimmed = validate_did_route(" 15551234567 ", Some("peer:eagle"), &dids, &nodes, &[])
            .expect("whitespace DID must trim at the verb boundary");
        assert_eq!(
            trimmed,
            VoiceAdminCommand::DidRoute {
                did: "15551234567".to_owned(),
                node_id: Some("peer:eagle".to_owned()),
            }
        );
        assert_eq!(
            validate_did_route("15551234567", Some("peer:eagle"), &dids, &nodes, &pending)
                .expect("same DID pending toward the same node is not a conflict"),
            VoiceAdminCommand::DidRoute {
                did: "15551234567".to_owned(),
                node_id: Some("peer:eagle".to_owned()),
            }
        );
        let twins = vec![
            provisioned_node("peer:east", "eagle", "east"),
            provisioned_node("peer:west", "eagle", "west"),
        ];
        assert_eq!(
            validate_did_route("15551234567", Some("eagle"), &dids, &twins, &[]).unwrap_err(),
            VoiceAdminRefuse::UnknownNode
        );
    }

    #[test]
    fn apply_voice_admin_refuses_invalid_dids_unknown_nodes_and_conflicts() {
        let nodes = vec![
            provisioned_node("peer:eagle", "eagle", "eagle"),
            provisioned_node("peer:otter", "otter", "otter"),
        ];
        let dids = vec![inventory("15551234567", Some("eagle"))];

        let mut form = VoiceAdminFormState::draft(
            "not-a-did",
            "peer:eagle",
            "peer:eagle",
            0,
            "",
            "15551234567",
            "main",
        );
        let mut sink = VoiceAdminSink::new();
        assert_eq!(
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                None,
                &mut sink,
                VoiceAdminFormIntent::DidRoute,
            ),
            VoiceAdminFormOutcome::Refused(VoiceAdminRefuse::InvalidDid)
        );
        assert_eq!(
            form.notice.as_deref(),
            Some(VoiceAdminRefuse::InvalidDid.label())
        );

        form.did = "15551234567".to_owned();
        form.route_node.clear();
        assert_eq!(
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                None,
                &mut sink,
                VoiceAdminFormIntent::DidRoute,
            ),
            VoiceAdminFormOutcome::Refused(VoiceAdminRefuse::UnknownNode)
        );

        form.did = "not-a-did".to_owned();
        form.route_node.clear();
        assert_eq!(
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                None,
                &mut sink,
                VoiceAdminFormIntent::DidRoute,
            ),
            VoiceAdminFormOutcome::Refused(VoiceAdminRefuse::InvalidDid),
            "invalid DID must refuse before an empty node identity"
        );

        form.did = "15551234567".to_owned();
        form.route_node = "peer:ghost".to_owned();
        assert_eq!(
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                None,
                &mut sink,
                VoiceAdminFormIntent::DidRoute,
            ),
            VoiceAdminFormOutcome::Refused(VoiceAdminRefuse::UnknownNode)
        );
        assert!(
            sink.is_empty(),
            "refused DID/node must not queue a voice verb: {:?}",
            sink.drain()
        );

        form.route_node = "peer:eagle".to_owned();
        assert!(matches!(
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                None,
                &mut sink,
                VoiceAdminFormIntent::DidRoute,
            ),
            VoiceAdminFormOutcome::Published(VoiceAdminCommand::DidRoute { .. })
        ));
        form.route_node = "peer:otter".to_owned();
        assert_eq!(
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                None,
                &mut sink,
                VoiceAdminFormIntent::DidRoute,
            ),
            VoiceAdminFormOutcome::Refused(VoiceAdminRefuse::ConflictingRoute)
        );
        assert_eq!(
            form.notice.as_deref(),
            Some(VoiceAdminRefuse::ConflictingRoute.label())
        );
        let queued = sink.drain();
        assert_eq!(queued.len(), 1);
        assert_eq!(
            queued[0],
            VoiceAdminCommand::DidRoute {
                did: "15551234567".to_owned(),
                node_id: Some("peer:eagle".to_owned()),
            }
        );

        let mixed = vec![
            provisioned_node("peer:eagle", "eagle", "eagle"),
            VoiceNodeProjection {
                node_id: "peer:otter".to_owned(),
                hostname: "otter".to_owned(),
                username: "otter".to_owned(),
                sip_uri: "otter@sip.vitelity.net".to_owned(),
                reg_state: VoiceRegState::Provisioning,
                routed_dids: Vec::new(),
                failover: None,
                updated_at_s: 1_700_000_000,
            },
        ];
        assert!(has_provisioned_voice_account(&mixed));
        assert_eq!(
            validate_did_route("15551234567", Some("peer:otter"), &dids, &mixed, &[]).unwrap_err(),
            VoiceAdminRefuse::NoProvisionedAccount
        );
    }

    #[test]
    fn voice_panel_stays_empty_without_a_provisioned_account() {
        let unprovisioned = [VoiceNodeProjection {
            node_id: "peer:eagle".to_owned(),
            hostname: "eagle".to_owned(),
            username: String::new(),
            sip_uri: String::new(),
            reg_state: VoiceRegState::Provisioning,
            routed_dids: Vec::new(),
            failover: None,
            updated_at_s: 0,
        }];
        assert!(!has_provisioned_voice_account(&unprovisioned));
        assert_eq!(
            validate_did_route(
                "15551234567",
                Some("peer:eagle"),
                &[inventory("15551234567", None)],
                &unprovisioned,
                &[]
            )
            .unwrap_err(),
            VoiceAdminRefuse::NoProvisionedAccount
        );
        assert_eq!(
            validate_failover("peer:eagle", VoiceFailoverPolicy::Voicemail, &unprovisioned)
                .unwrap_err(),
            VoiceAdminRefuse::NoProvisionedAccount
        );
    }

    #[test]
    fn derived_username_while_provisioning_is_not_a_provisioned_account() {
        // voice_provision::awaiting_master_key publishes a hostname-derived
        // username + sip_uri in Provisioning. That is not a sealed sub-account.
        let awaiting = [VoiceNodeProjection {
            node_id: "peer:eagle".to_owned(),
            hostname: "eagle".to_owned(),
            username: "eagle".to_owned(),
            sip_uri: "eagle@sip.vitelity.net".to_owned(),
            reg_state: VoiceRegState::Provisioning,
            routed_dids: Vec::new(),
            failover: None,
            updated_at_s: 1_700_000_000,
        }];
        assert!(!awaiting[0].is_provisioned());
        assert!(!has_provisioned_voice_account(&awaiting));
        assert_eq!(
            voice_unprovisioned_headline(&awaiting),
            "Awaiting Vitelity master key"
        );
        assert_eq!(
            voice_unprovisioned_detail(&awaiting),
            "Enrolled nodes are awaiting a sealed Vitelity master key or a live provider pass. DID routing stays closed until a node is Registered or Unregistered."
        );
        assert_eq!(
            validate_did_route(
                "15551234567",
                Some("eagle"),
                &[inventory("15551234567", None)],
                &awaiting,
                &[]
            )
            .unwrap_err(),
            VoiceAdminRefuse::NoProvisionedAccount
        );
        assert_eq!(
            validate_failover("eagle", VoiceFailoverPolicy::Voicemail, &awaiting).unwrap_err(),
            VoiceAdminRefuse::NoProvisionedAccount
        );

        let gated = [VoiceNodeProjection {
            node_id: "peer:eagle".to_owned(),
            hostname: "eagle".to_owned(),
            username: "eagle".to_owned(),
            sip_uri: String::new(),
            reg_state: VoiceRegState::Error {
                reason: "cannot list Vitelity sub-accounts: integration-gated".to_owned(),
            },
            routed_dids: Vec::new(),
            failover: None,
            updated_at_s: 1_700_000_000,
        }];
        assert!(!has_provisioned_voice_account(&gated));
        assert_eq!(voice_unprovisioned_headline(&gated), "Voice provider error");
        assert_eq!(
            voice_unprovisioned_detail(&gated),
            "Provider has not sealed a sub-account. The fleet board shows the honest error."
        );
    }

    #[test]
    fn route_and_failover_resolve_hostname_and_username_to_the_worker_node_id() {
        let nodes = vec![provisioned_node("peer:eagle", "eagle", "eagle")];
        let dids = vec![inventory("15551234567", Some("eagle"))];

        assert_eq!(
            resolve_voice_node(&nodes, "eagle")
                .expect("hostname")
                .node_id,
            "peer:eagle"
        );
        let by_host = validate_did_route("15551234567", Some("eagle"), &dids, &nodes, &[])
            .expect("hostname route");
        assert_eq!(
            by_host,
            VoiceAdminCommand::DidRoute {
                did: "15551234567".to_owned(),
                node_id: Some("peer:eagle".to_owned()),
            }
        );
        let by_user = validate_failover("eagle", VoiceFailoverPolicy::None, &nodes)
            .expect("username failover");
        assert_eq!(
            by_user,
            VoiceAdminCommand::Failover {
                node_id: "peer:eagle".to_owned(),
                policy: VoiceFailoverPolicy::None,
            }
        );
        assert_eq!(
            validate_did_route("15551234567", Some("peer:ghost"), &dids, &nodes, &[]).unwrap_err(),
            VoiceAdminRefuse::UnknownNode
        );
    }

    #[test]
    fn retained_shared_config_seeds_the_apply_form_for_a_round_trip() {
        let shared = VoiceSharedOutbound {
            caller_id: "15551234567".to_owned(),
            outbound_trunk: "main".to_owned(),
        };
        let form = seed_voice_shared_form_state(VoiceAdminFormState::default(), Some(&shared));

        assert_eq!(form.caller_id, shared.caller_id);
        assert_eq!(form.outbound_trunk, shared.outbound_trunk);
        assert!(form.shared_seeded);

        let edited = VoiceAdminFormState {
            caller_id: "15557654321".to_owned(),
            outbound_trunk: "backup".to_owned(),
            shared_seeded: true,
            ..form
        };
        let unchanged = seed_voice_shared_form_state(edited, Some(&shared));
        assert_eq!(unchanged.caller_id, "15557654321");
        assert_eq!(unchanged.outbound_trunk, "backup");
    }

    #[test]
    fn failover_and_shared_config_and_cutover_validate() {
        let nodes = vec![provisioned_node("peer:eagle", "eagle", "eagle")];
        assert!(validate_failover("peer:ghost", VoiceFailoverPolicy::Voicemail, &nodes).is_err());
        assert_eq!(
            validate_failover(
                "peer:eagle",
                VoiceFailoverPolicy::Forward {
                    number: "12".to_owned()
                },
                &nodes
            )
            .unwrap_err(),
            VoiceAdminRefuse::InvalidDid
        );
        assert!(validate_failover("peer:eagle", VoiceFailoverPolicy::Voicemail, &nodes).is_ok());

        assert_eq!(
            validate_shared_config("bad", "main").unwrap_err(),
            VoiceAdminRefuse::InvalidSharedConfig
        );
        assert!(validate_shared_config("15551234567", "main").is_ok());

        let ready = VoiceCutoverStatus {
            phase: VoiceCutoverPhase::NodesReprovisioning,
            total_nodes: 2,
            reprovisioned: 1,
            pending_nodes: vec!["otter".to_owned()],
            shared_outbound_lifted: true,
            updated_at_s: 1,
        };
        assert!(validate_cutover(Some(&ready), &nodes).is_ok());
        let done = VoiceCutoverStatus {
            phase: VoiceCutoverPhase::CutoverComplete,
            pending_nodes: Vec::new(),
            ..ready.clone()
        };
        assert_eq!(
            validate_cutover(Some(&done), &nodes).unwrap_err(),
            VoiceAdminRefuse::CutoverNotReady
        );
    }

    #[test]
    fn malformed_hosts_and_replayed_clears_refuse() {
        assert_eq!(
            validate_gateway_set("http://pbx.example.com", None, "alice", "", "", None)
                .unwrap_err(),
            GatewayRefuse::MalformedHost
        );
        assert_eq!(
            validate_gateway_set("pbx.example.com/sip", None, "alice", "", "", None).unwrap_err(),
            GatewayRefuse::MalformedHost
        );
        assert_eq!(
            validate_gateway_set("not a host", None, "alice", "", "", None).unwrap_err(),
            GatewayRefuse::MalformedHost
        );
        assert_eq!(
            validate_gateway_set("", None, "alice", "", "", None).unwrap_err(),
            GatewayRefuse::MalformedHost
        );
        assert_eq!(
            validate_gateway_set("pbx.example.com", Some(0), "alice", "", "", None).unwrap_err(),
            GatewayRefuse::InvalidPort
        );
        assert_eq!(
            validate_gateway_set(
                "pbx.example.com",
                None,
                "alice",
                "",
                "",
                Some(u64::from(u32::MAX) + 1)
            )
            .unwrap_err(),
            GatewayRefuse::InvalidExpiry
        );
        assert_eq!(
            validate_gateway_set("pbx.example.com", None, "alice", "", "", Some(0)).unwrap_err(),
            GatewayRefuse::InvalidExpiry
        );
        assert_eq!(
            validate_gateway_set("pbx.example.com", None, "  ", "", "", None).unwrap_err(),
            GatewayRefuse::UsernameRequired
        );
        assert!(validate_gateway_set(
            "pbx.example.com",
            Some(5062),
            "alice",
            "s3cret",
            "Alice",
            None
        )
        .is_ok());

        let absent = GatewayReadout::absent();
        let mut sink = GatewaySink::new();
        assert_eq!(
            validate_gateway_clear(Some(&absent), &sink).unwrap_err(),
            GatewayRefuse::ReplayClear
        );
        assert_eq!(
            validate_gateway_clear(None, &sink).unwrap_err(),
            GatewayRefuse::ReplayClear
        );

        let present =
            GatewayReadout::present("pbx.example.com", 5062, "alice", true, "Alice", 3600);
        assert!(present.password.is_empty());
        assert_eq!(present.redacted_password(), "");
        let first = validate_gateway_clear(Some(&present), &sink).expect("first clear is live");
        sink.emit(first);
        assert_eq!(
            validate_gateway_clear(Some(&present), &sink).unwrap_err(),
            GatewayRefuse::ReplayClear
        );
    }

    #[test]
    fn gateway_debug_never_echoes_the_password() {
        let set = GatewayCommand::Set {
            host: "pbx.example.com".to_owned(),
            port: Some(5062),
            username: "alice".to_owned(),
            password: "s3cret".to_owned(),
            display_name: "Alice".to_owned(),
            expires: None,
        };
        let debug = format!("{set:?}");
        assert!(
            !debug.contains("s3cret"),
            "Debug echoed the gateway password: {debug}"
        );
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn gateway_form_set_get_clear_never_renders_password() {
        let leaked = r#"{"present":true,"host":"pbx.example.com","port":5062,"username":"alice","password":"s3cret","password_set":true,"display_name":"Alice","expires":3600}"#;
        let readout = GatewayReadout::from_get_reply(leaked).expect("present reply binds");
        assert!(readout.present);
        assert_eq!(readout.host, "pbx.example.com");
        assert_eq!(readout.port, 5062);
        assert!(readout.password.is_empty());
        assert_eq!(readout.redacted_password(), "");
        assert!(readout.password_set);
        let leaked_debug = format!("{readout:?}");
        assert!(
            !leaked_debug.contains("s3cret"),
            "get-reply readout Debug leaked the password: {leaked_debug}"
        );
        assert_eq!(
            GatewayReadout::from_get_reply(r#"{"present":false}"#),
            Some(GatewayReadout::absent())
        );

        let mut form = GatewayFormState::draft(
            "http://pbx.example.com",
            "5062",
            "alice",
            "s3cret",
            "Alice",
            "3600",
        );
        let mut sink = GatewaySink::new();
        assert_eq!(
            apply_gateway_form(&mut form, Some(&readout), &mut sink, GatewayFormIntent::Set),
            GatewayFormOutcome::Refused(GatewayRefuse::MalformedHost)
        );
        assert!(
            sink.is_empty(),
            "malformed host must not publish set-gateway"
        );
        assert_eq!(form.password, "s3cret");

        form.host = "pbx.example.com".to_owned();
        match apply_gateway_form(&mut form, Some(&readout), &mut sink, GatewayFormIntent::Set) {
            GatewayFormOutcome::Published(GatewayCommand::Set { password, .. }) => {
                assert_eq!(password, "s3cret");
            }
            other => panic!("expected published set, got {other:?}"),
        }
        assert!(
            form.password.is_empty(),
            "write path must wipe the draft password"
        );
        let notice = form.notice.clone().expect("set publishes a notice");
        assert!(
            !notice.contains("s3cret"),
            "notice echoed the password: {notice}"
        );
        let form_debug = format!("{form:?}");
        assert!(
            !form_debug.contains("s3cret"),
            "form Debug echoed the password: {form_debug}"
        );

        assert!(matches!(
            apply_gateway_form(&mut form, Some(&readout), &mut sink, GatewayFormIntent::Get),
            GatewayFormOutcome::Published(GatewayCommand::Get)
        ));

        assert_eq!(
            apply_gateway_form(
                &mut form,
                Some(&readout),
                &mut sink,
                GatewayFormIntent::Clear
            ),
            GatewayFormOutcome::ArmedClear
        );
        assert!(
            !sink
                .queued()
                .iter()
                .any(|c| matches!(c, GatewayCommand::Clear)),
            "first clear click only arms confirm"
        );
        assert_eq!(
            apply_gateway_form(
                &mut form,
                Some(&readout),
                &mut sink,
                GatewayFormIntent::Clear
            ),
            GatewayFormOutcome::Published(GatewayCommand::Clear)
        );
        assert_eq!(
            apply_gateway_form(
                &mut form,
                Some(&readout),
                &mut sink,
                GatewayFormIntent::Clear
            ),
            GatewayFormOutcome::ArmedClear
        );
        assert_eq!(
            apply_gateway_form(
                &mut form,
                Some(&readout),
                &mut sink,
                GatewayFormIntent::Clear
            ),
            GatewayFormOutcome::Refused(GatewayRefuse::ReplayClear)
        );

        let drained = sink.drain();
        assert!(matches!(drained[0], GatewayCommand::Set { .. }));
        assert!(matches!(drained[1], GatewayCommand::Get));
        assert!(matches!(drained[2], GatewayCommand::Clear));
        if let GatewayCommand::Set { password, .. } = &drained[0] {
            let debug = format!("{:?}", drained[0]);
            assert_eq!(password, "s3cret");
            assert!(
                !debug.contains("s3cret"),
                "queued set Debug leaked: {debug}"
            );
        }
    }

    #[test]
    fn hydrate_migrates_in_place_gateway_toml_without_echoing_password() {
        let tmp = tempfile::tempdir().expect("workgroup scratch");
        let path = workgroup_gateway_toml_path(tmp.path());
        std::fs::create_dir_all(path.parent().expect("voip dir")).expect("voip dir");
        let toml = concat!(
            "username = \"alice\"\n",
            "password = \"s3cret\"\n",
            "server = \"pbx.example.com:5062\"\n",
            "display_name = \"Alice\"\n",
            "expires = 3600\n",
        );
        std::fs::write(&path, toml).expect("write in-place gateway.toml");
        let before = std::fs::read(&path).expect("read gateway.toml");

        let expected =
            GatewayReadout::present("pbx.example.com", 5062, "alice", true, "Alice", 3600);
        let readout = hydrate_gateway_readout(Some(tmp.path()), None);
        assert_eq!(readout, expected);
        assert!(readout.present);
        assert!(readout.password.is_empty());
        assert_eq!(readout.redacted_password(), "");
        let debug = format!("{readout:?}");
        assert!(
            !debug.contains("s3cret"),
            "hydrated readout Debug leaked the password: {debug}"
        );
        assert_eq!(
            std::fs::read(&path).expect("re-read gateway.toml"),
            before,
            "in-place migrate must not rewrite gateway.toml"
        );
        assert!(
            std::str::from_utf8(&before)
                .expect("utf8")
                .contains("s3cret"),
            "the on-disk secret must stay in the 0600 file"
        );

        let empty = tempfile::tempdir().expect("empty workgroup");
        let leaked = r#"{"present":true,"host":"pbx.example.com","port":5062,"username":"alice","password":"s3cret","password_set":true,"display_name":"Alice","expires":3600}"#;
        let from_get = hydrate_gateway_readout(Some(empty.path()), Some(leaked));
        assert_eq!(from_get, expected);
        assert!(from_get.password.is_empty());
        let from_get_debug = format!("{from_get:?}");
        assert!(
            !from_get_debug.contains("s3cret"),
            "retained get Debug leaked the password: {from_get_debug}"
        );

        let ignored = r#"{"present":true,"host":"other.example","port":5060,"username":"bob","password":"s3cret","password_set":true,"display_name":"Bob","expires":120}"#;
        let prefer_file = hydrate_gateway_readout(Some(tmp.path()), Some(ignored));
        assert_eq!(prefer_file, expected);

        assert_eq!(
            hydrate_gateway_readout(Some(empty.path()), None),
            GatewayReadout::absent()
        );

        let host_only = concat!(
            "username = \"alice\"\n",
            "server = \"pbx.example.com\"\n",
            "display_name = \"Alice\"\n",
        );
        assert_eq!(
            GatewayReadout::from_gateway_toml(host_only),
            Some(GatewayReadout::present(
                "pbx.example.com",
                5060,
                "alice",
                false,
                "Alice",
                3600
            ))
        );

        let mut snapshot = ActivityAdminSnapshot::default();
        assert!(snapshot.gateway.is_none());
        snapshot.hydrate_gateway(Some(tmp.path()), Some(leaked));
        assert_eq!(snapshot.gateway.as_ref(), Some(&expected));
        assert!(snapshot
            .gateway
            .as_ref()
            .is_some_and(|row| row.password.is_empty() && row.password_set));
    }

    #[test]
    fn cutover_headlines_match_the_operator_phase() {
        assert_eq!(
            VoiceCutoverPhase::Legacy.headline(),
            "Legacy single-account model — apply the fleet shared-outbound to begin"
        );
        assert_eq!(
            VoiceCutoverPhase::LiftedSharedOutbound.headline(),
            "Shared-outbound lifted — outbound alive; reprovisioning nodes onto the split model"
        );
        assert_eq!(
            VoiceCutoverPhase::NodesReprovisioning.headline(),
            "Cutover in progress — some nodes still on the legacy model"
        );
        assert_eq!(
            VoiceCutoverPhase::CutoverComplete.headline(),
            "Cutover complete — every node on the split model"
        );
    }

    #[test]
    fn fleet_voice_admin_apply_publishes_typed_verbs_and_empty_projections_stay_honest() {
        assert_eq!(
            voice_projection_empty_notes(&[], &[], None, None),
            [
                "No state/voice nodes projected.",
                "No master-account DIDs projected.",
                "Shared-outbound is not lifted yet.",
                "No cutover status projected.",
            ]
        );
        assert_eq!(
            voice_unprovisioned_headline(&[]),
            "No provisioned voice account"
        );
        assert_eq!(
            voice_unprovisioned_detail(&[]),
            "DID routing, failover, shared-outbound, and cutover stay empty until a node has a sub-account."
        );

        let mut empty_form = VoiceAdminFormState::draft(
            "15551234567",
            "peer:eagle",
            "peer:eagle",
            0,
            "",
            "15551234567",
            "main",
        );
        let mut sink = VoiceAdminSink::new();
        match apply_voice_admin(
            &mut empty_form,
            &[],
            &[],
            None,
            &mut sink,
            VoiceAdminFormIntent::Provision,
        ) {
            VoiceAdminFormOutcome::Published(command) => {
                assert_eq!(command.topic(), VOICE_PROVISION_TOPIC);
                assert_eq!(command.json_body(), "{}");
            }
            other => panic!("empty projections must still publish provision, got {other:?}"),
        }
        empty_form.confirm_cutover = true;
        for intent in [
            VoiceAdminFormIntent::DidRoute,
            VoiceAdminFormIntent::Unroute,
            VoiceAdminFormIntent::Failover,
            VoiceAdminFormIntent::SharedConfig,
            VoiceAdminFormIntent::Cutover,
        ] {
            assert_eq!(
                apply_voice_admin(&mut empty_form, &[], &[], None, &mut sink, intent),
                VoiceAdminFormOutcome::Refused(VoiceAdminRefuse::NoProvisionedAccount),
                "{intent:?} must refuse honestly when state/voice projections are empty"
            );
        }
        assert!(
            !empty_form.confirm_cutover,
            "unprovisioned cutover must not stay armed"
        );
        let drained = sink.drain();
        assert_eq!(drained.len(), 1);
        assert!(matches!(drained[0], VoiceAdminCommand::Provision));

        let nodes = vec![provisioned_node("peer:eagle", "eagle", "eagle")];
        let dids = vec![inventory("15551234567", Some("eagle"))];
        let shared = VoiceSharedOutbound {
            caller_id: "15551234567".to_owned(),
            outbound_trunk: "main".to_owned(),
        };
        let cutover = VoiceCutoverStatus {
            phase: VoiceCutoverPhase::NodesReprovisioning,
            total_nodes: 2,
            reprovisioned: 1,
            pending_nodes: vec!["otter".to_owned()],
            shared_outbound_lifted: true,
            updated_at_s: 1,
        };
        assert!(
            voice_projection_empty_notes(&nodes, &dids, Some(&shared), Some(&cutover)).is_empty()
        );

        let mut form = VoiceAdminFormState::draft(
            "15551234567",
            "peer:eagle",
            "peer:eagle",
            2,
            "",
            "15551234567",
            "main",
        );
        let mut live = VoiceAdminSink::new();
        let published = [
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                Some(&cutover),
                &mut live,
                VoiceAdminFormIntent::Provision,
            ),
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                Some(&cutover),
                &mut live,
                VoiceAdminFormIntent::DidRoute,
            ),
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                Some(&cutover),
                &mut live,
                VoiceAdminFormIntent::Failover,
            ),
            apply_voice_admin(
                &mut form,
                &nodes,
                &dids,
                Some(&cutover),
                &mut live,
                VoiceAdminFormIntent::SharedConfig,
            ),
        ];
        assert!(matches!(
            published[0],
            VoiceAdminFormOutcome::Published(VoiceAdminCommand::Provision)
        ));
        match &published[1] {
            VoiceAdminFormOutcome::Published(command) => {
                assert_eq!(command.topic(), VOICE_DID_ROUTE_TOPIC);
                assert_eq!(
                    command.json_body(),
                    r#"{"did":"15551234567","node_id":"peer:eagle"}"#
                );
            }
            other => panic!("expected published DID route, got {other:?}"),
        }
        match &published[2] {
            VoiceAdminFormOutcome::Published(command) => {
                assert_eq!(command.topic(), VOICE_FAILOVER_TOPIC);
                assert_eq!(
                    command.json_body(),
                    r#"{"node_id":"peer:eagle","policy":"None"}"#
                );
            }
            other => panic!("expected published failover, got {other:?}"),
        }
        match &published[3] {
            VoiceAdminFormOutcome::Published(command) => {
                assert_eq!(command.topic(), VOICE_SHARED_CONFIG_TOPIC);
                assert_eq!(
                    command.json_body(),
                    r#"{"caller_id":"15551234567","outbound_trunk":"main"}"#
                );
            }
            other => panic!("expected published shared-config, got {other:?}"),
        }
        let live_cmds = live.drain();
        assert_eq!(live_cmds.len(), 4);
        assert!(matches!(live_cmds[0], VoiceAdminCommand::Provision));
        assert!(matches!(live_cmds[1], VoiceAdminCommand::DidRoute { .. }));
        assert!(matches!(live_cmds[2], VoiceAdminCommand::Failover { .. }));
        assert!(matches!(
            live_cmds[3],
            VoiceAdminCommand::SharedConfig { .. }
        ));
    }

    #[test]
    fn hydrate_voice_folds_latest_retained_topics_and_stays_honest_when_empty() {
        let mut snapshot = ActivityAdminSnapshot::default();
        snapshot.hydrate_voice(Some("[]"), Some("null"), None, &[]);
        assert!(snapshot.voice_nodes.is_empty(), "must not invent node rows");
        assert!(snapshot.voice_dids.is_empty());
        assert!(snapshot.voice_shared.is_none());
        assert!(snapshot.voice_cutover.is_none());
        assert_eq!(
            voice_projection_empty_notes(
                &snapshot.voice_nodes,
                &snapshot.voice_dids,
                snapshot.voice_shared.as_ref(),
                snapshot.voice_cutover.as_ref(),
            ),
            [
                "No state/voice nodes projected.",
                "No master-account DIDs projected.",
                "Shared-outbound is not lifted yet.",
                "No cutover status projected.",
            ]
        );

        let hud = r#"{"registered":true,"listening":true,"not":"a-fleet-row"}"#;
        let worker_empty_cutover = concat!(
            r#"{"phase":"legacy","total_nodes":0,"reprovisioned":0,"#,
            r#""pending_nodes":[],"shared_outbound_lifted":false,"updated_at_s":1}"#,
        );
        snapshot.hydrate_voice(
            Some("[]"),
            Some("null"),
            Some(worker_empty_cutover),
            &[
                ("state/voice/status", hud),
                (
                    "state/voice-dids",
                    r#"[{"number":"15551234567","routed_to":"eagle"}]"#,
                ),
                (
                    "state/voice/",
                    r#"{"node_id":"ghost","state":"registered","updated_at_s":1}"#,
                ),
                ("state/voice/peer:missing", "null"),
            ],
        );
        assert!(
            snapshot.voice_nodes.is_empty(),
            "HUD / fleet-wide / empty suffix / null body must not invent node rows"
        );
        assert!(snapshot.voice_dids.is_empty());
        assert!(snapshot.voice_shared.is_none());
        assert_eq!(
            snapshot.voice_cutover.as_ref().map(|row| row.phase),
            Some(VoiceCutoverPhase::Legacy)
        );
        assert_eq!(
            snapshot.voice_cutover.as_ref().map(|row| row.total_nodes),
            Some(0)
        );

        let node = concat!(
            r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle","#,
            r#""sip_uri":"eagle@sip.vitelity.net","state":"unregistered","#,
            r#""routed_dids":["15551234567"],"failover":"Voicemail","updated_at_s":1700000000}"#,
        );
        let otter = concat!(
            r#"{"node_id":"peer:otter","hostname":"otter","username":"otter","#,
            r#""sip_uri":"otter@sip.vitelity.net","state":"error","reason":"timeout","#,
            r#""routed_dids":[],"failover":{"Forward":{"number":"15557654321"}},"#,
            r#""updated_at_s":1700000001}"#,
        );
        snapshot.hydrate_voice(
            Some(r#"[{"number":"15551234567","routed_to":"eagle"},{"number":"15557654321","routed_to":null}]"#),
            Some(r#"{"caller_id":"15551234567","outbound_trunk":"main"}"#),
            Some(concat!(
                r#"{"phase":"nodes-reprovisioning","total_nodes":2,"reprovisioned":1,"#,
                r#""pending_nodes":["otter"],"shared_outbound_lifted":true,"updated_at_s":1700000000}"#,
            )),
            &[
                ("state/voice/status", hud),
                ("state/voice/peer:otter", otter),
                ("state/voice/peer:eagle", node),
            ],
        );
        assert_eq!(
            snapshot.voice_nodes.len(),
            2,
            "only published state/voice/<node> rows bind"
        );
        assert_eq!(snapshot.voice_nodes[0].node_id, "peer:eagle");
        assert_eq!(snapshot.voice_nodes[0].sip_uri, "eagle@sip.vitelity.net");
        assert_eq!(
            snapshot.voice_nodes[0].routed_dids,
            vec!["15551234567".to_owned()]
        );
        assert_eq!(
            snapshot.voice_nodes[0].failover,
            Some(VoiceFailoverPolicy::Voicemail)
        );
        assert_eq!(snapshot.voice_nodes[1].node_id, "peer:otter");
        assert_eq!(
            snapshot.voice_nodes[1].reg_state,
            VoiceRegState::Error {
                reason: "timeout".to_owned()
            }
        );
        assert_eq!(
            snapshot.voice_nodes[1].failover,
            Some(VoiceFailoverPolicy::Forward {
                number: "15557654321".to_owned()
            })
        );
        assert_eq!(
            snapshot.voice_dids,
            vec![
                inventory("15551234567", Some("eagle")),
                inventory("15557654321", None),
            ]
        );
        assert_eq!(
            snapshot.voice_shared.as_ref(),
            Some(&VoiceSharedOutbound {
                caller_id: "15551234567".to_owned(),
                outbound_trunk: "main".to_owned(),
            })
        );
        assert_eq!(
            snapshot.voice_cutover.as_ref().map(|row| row.phase),
            Some(VoiceCutoverPhase::NodesReprovisioning)
        );
        assert_eq!(
            snapshot
                .voice_cutover
                .as_ref()
                .map(|row| row.pending_nodes.clone()),
            Some(vec!["otter".to_owned()])
        );
        assert!(voice_projection_empty_notes(
            &snapshot.voice_nodes,
            &snapshot.voice_dids,
            snapshot.voice_shared.as_ref(),
            snapshot.voice_cutover.as_ref(),
        )
        .is_empty());

        snapshot.hydrate_voice(None, None, None, &[]);
        assert!(snapshot.voice_nodes.is_empty());
        assert!(snapshot.voice_dids.is_empty());
        assert!(snapshot.voice_shared.is_none());
        assert!(snapshot.voice_cutover.is_none());
        assert!(
            snapshot.gateway.is_none(),
            "voice refresh must not invent a gateway"
        );
    }
}
