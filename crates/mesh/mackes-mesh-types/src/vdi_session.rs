//! VDI session-lifecycle wire contract — the `action/vdi/session` request verb
//! shared by the `mackesd` session broker (which drains it and folds it into the
//! roaming-session roster) and the desktop shell (which mints `Open` on Connect
//! and projects the live lifecycle into the bottom rail).
//!
//! arch-2 (2026-07-11) — hoisted out of `mackesd::workers::session_broker` so the
//! two shell mirrors (`discovery`, `session_rail`) reuse the ONE type instead of
//! hand-maintaining byte-compatible copies of a wire type that can silently drift.
//! It lands here (like [`crate::mesh_storage`] / [`crate::device_control`]) so the
//! desktop tier depends only on this lightweight serde crate, never the heavy
//! `async-services`-gated (`tokio` / `zbus` / `etcd`) daemon crate (§6). `mackesd`
//! re-exports it from `session_broker` so its own `SessionRequest` paths are
//! unchanged.

// This is a stable wire-contract surface. Keep its public API and serialized
// representation unchanged while avoiding documentation-only protocol churn.
#![allow(
    clippy::missing_errors_doc,
    clippy::too_long_first_doc_paragraph,
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::double_must_use
)]

use serde::{Deserialize, Serialize};

const MAX_APP_VM_FIELD_BYTES: usize = 255;

/// The only guest profile currently admitted by the App VM runtime path.
pub const APP_VM_GUEST_PROFILE_WAYLAND_STANDARD: &str = "wayland-standard";

/// Whether a guest profile names a supported, image-backed App VM runtime.
#[must_use]
pub fn is_supported_app_vm_guest_profile(value: &str) -> bool {
    value == APP_VM_GUEST_PROFILE_WAYLAND_STANDARD
}

/// A typed, fail-closed request to place or resume one guest-owned application
/// inside an App VM. Callers cannot smuggle a command, mount, environment, or
/// socket through it; the guest runtime resolves the admitted profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppVmLaunchRequest {
    /// Stable reverse-DNS Flatpak identity.
    pub app_id: String,
    /// Signed catalog revision used to select the guest declaration.
    pub catalog_revision: String,
    /// Named, approved guest profile; never an image path or command line.
    pub guest_profile: String,
    /// Capability names admitted by policy for this app session.
    pub requested_capabilities: Vec<String>,
    /// Stable session identity used for resume/reconnect convergence.
    pub session_id: String,
    /// Whether an existing guest session should be resumed when available.
    pub resume: bool,
}

/// Bus topic where a guest App VM reports bounded runtime evidence. The guest
/// reports observations only; the serving daemon remains the authority that
/// turns accepted evidence into signed `action/vdi/session` readiness events.
pub const APP_VM_RUNTIME_TOPIC: &str = "state/vdi/app-runtime";

/// Runtime states a guest is allowed to report. Placement, policy, and catalog
/// freshness remain daemon-owned and are intentionally absent here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppVmRuntimeState {
    /// The guest is installing or updating the admitted application.
    Installing,
    /// The guest compositor and application are starting.
    StartingApp,
    /// The application surface is ready.
    Connected,
    /// The application is intentionally suspended.
    Paused,
    /// The guest is recovering its application surface.
    Reconnecting,
    /// The guest cannot currently serve the application.
    Unavailable,
    /// The guest runtime failed and needs recovery.
    Failed,
}

impl AppVmRuntimeState {
    /// Convert guest evidence into the corresponding daemon-owned lifecycle
    /// state after identity and transition checks have succeeded.
    #[must_use]
    pub const fn lifecycle_state(self) -> AppVmLifecycleState {
        match self {
            Self::Installing => AppVmLifecycleState::Installing,
            Self::StartingApp => AppVmLifecycleState::StartingApp,
            Self::Connected => AppVmLifecycleState::Connected,
            Self::Paused => AppVmLifecycleState::Paused,
            Self::Reconnecting => AppVmLifecycleState::Reconnecting,
            Self::Unavailable => AppVmLifecycleState::Unavailable,
            Self::Failed => AppVmLifecycleState::Failed,
        }
    }
}

/// One bounded guest runtime observation. It carries no command, path, mount,
/// environment, or socket; all identities must match the admitted session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppVmRuntimeEvidence {
    /// Session identity from the admitted App VM declaration.
    pub session_id: String,
    /// App VM identity from the cloud declaration.
    pub vm_id: String,
    /// Flatpak identity from the catalog declaration.
    pub app_id: String,
    /// Monotonic generation assigned by the guest runtime for this session.
    /// Zero represents legacy evidence that predates generation ordering.
    #[serde(default, skip_serializing_if = "is_zero_generation")]
    pub generation: u64,
    /// Guest-observed runtime state.
    pub state: AppVmRuntimeState,
    /// Optional bounded diagnostic context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AppVmRuntimeEvidence {
    /// Validate the guest evidence before it is accepted by the daemon.
    pub fn validate(&self) -> Result<(), &'static str> {
        for value in [&self.session_id, &self.vm_id, &self.app_id] {
            if value.trim().is_empty()
                || value.len() > MAX_APP_VM_FIELD_BYTES
                || value.chars().any(char::is_control)
                || value.contains('/')
                || value.contains('\\')
            {
                return Err("invalid App VM runtime identity");
            }
        }
        // A legacy generation-zero observation is useful for ordinary
        // readiness compatibility, but it cannot identify which runtime
        // incarnation owns a reconnect. Requiring an explicit generation here
        // prevents a delayed pre-generation reconnect report from reopening a
        // newer session after recovery.
        if self.generation == 0 && self.state == AppVmRuntimeState::Reconnecting {
            return Err("reconnecting App VM runtime evidence requires a generation");
        }
        if self.reason.as_ref().is_some_and(|reason| {
            reason.len() > MAX_APP_VM_FIELD_BYTES || reason.chars().any(char::is_control)
        }) {
            return Err("invalid App VM runtime reason");
        }
        Ok(())
    }
}

/// Readiness of the guest-owned application, independent of the desktop link.
/// A connected VDI transport does not imply that the guest or application is
/// ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppVmLifecycleState {
    /// The guest image or application declaration is being installed.
    Installing,
    /// The desired App VM exists but has not been placed yet.
    WaitingForPlacement,
    /// The guest VM is booting.
    StartingGuest,
    /// The guest is ready and the application is launching.
    StartingApp,
    /// The application is available to the client.
    Connected,
    /// The application is intentionally suspended.
    Paused,
    /// The guest or application is recovering a lost connection.
    Reconnecting,
    /// The service cannot currently be reached.
    Unavailable,
    /// Policy prevented the requested launch.
    Denied,
    /// The catalog revision is no longer usable.
    StaleCatalog,
    /// Launch or runtime failed.
    Failed,
}

impl AppVmLifecycleState {
    /// Whether an application readiness state may advance from `self` to
    /// `next`. Repeating the current state is idempotent; all other edges are
    /// explicit so stale or hostile updates cannot claim a false connection.
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::WaitingForPlacement,
                Self::Installing | Self::StartingGuest | Self::Denied | Self::StaleCatalog | Self::Failed)
                | (Self::Installing, Self::StartingGuest | Self::Denied | Self::StaleCatalog | Self::Failed)
                | (Self::StartingGuest, Self::Installing | Self::StartingApp | Self::Unavailable | Self::Failed)
                | (Self::StartingApp, Self::Connected | Self::Unavailable | Self::Failed)
                | (Self::Connected, Self::Paused | Self::Reconnecting | Self::Unavailable | Self::Failed)
                | (Self::Paused, Self::StartingGuest | Self::Reconnecting | Self::Connected | Self::Failed)
                | (Self::Reconnecting, Self::StartingGuest | Self::StartingApp | Self::Connected | Self::Unavailable | Self::Failed)
                | (Self::Unavailable, Self::WaitingForPlacement | Self::Installing | Self::StartingGuest | Self::Failed)
                | (Self::Denied | Self::StaleCatalog, Self::WaitingForPlacement | Self::Installing)
                | (Self::Failed, Self::WaitingForPlacement | Self::Installing | Self::StartingGuest)
        )
    }
}

/// Why an App VM launch request was rejected before it reaches a broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppVmLaunchRequestError {
    /// A required identity/profile field was blank or malformed.
    InvalidField(&'static str),
    /// A field exceeded the bounded wire contract.
    FieldTooLong(&'static str),
    /// A capability was repeated, blank, contained a delimiter, or is outside
    /// the admitted guest policy.
    InvalidCapability,
}

impl core::fmt::Display for AppVmLaunchRequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidField(field) => write!(f, "invalid App VM field: {field}"),
            Self::FieldTooLong(field) => write!(f, "App VM field exceeds 255 bytes: {field}"),
            Self::InvalidCapability => {
                f.write_str("invalid, duplicate, or unadmitted App VM capability")
            }
        }
    }
}

impl std::error::Error for AppVmLaunchRequestError {}

impl AppVmLaunchRequest {
    /// Validate and construct a bounded guest-owned App VM launch request.
    pub fn new(
        app_id: impl Into<String>,
        catalog_revision: impl Into<String>,
        guest_profile: impl Into<String>,
        requested_capabilities: Vec<String>,
        session_id: impl Into<String>,
        resume: bool,
    ) -> Result<Self, AppVmLaunchRequestError> {
        let request = Self {
            app_id: app_id.into(),
            catalog_revision: catalog_revision.into(),
            guest_profile: guest_profile.into(),
            requested_capabilities,
            session_id: session_id.into(),
            resume,
        };
        request.validate()?;
        Ok(request)
    }

    /// Re-check a request received from an untrusted boundary before it is
    /// serialized onto the lifecycle bus.
    pub fn validate(&self) -> Result<(), AppVmLaunchRequestError> {
        for (field, value) in [
            ("app_id", self.app_id.as_str()),
            ("catalog_revision", self.catalog_revision.as_str()),
            ("guest_profile", self.guest_profile.as_str()),
            ("session_id", self.session_id.as_str()),
        ] {
            if value.trim().is_empty()
                || value.chars().any(char::is_control)
                || value.contains('/')
                || value.contains('\\')
            {
                return Err(AppVmLaunchRequestError::InvalidField(field));
            }
            if value.len() > MAX_APP_VM_FIELD_BYTES {
                return Err(AppVmLaunchRequestError::FieldTooLong(field));
            }
        }
        if !is_supported_app_vm_guest_profile(&self.guest_profile) {
            return Err(AppVmLaunchRequestError::InvalidField("guest_profile"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for capability in &self.requested_capabilities {
            if capability.is_empty()
                || capability.len() > MAX_APP_VM_FIELD_BYTES
                || capability
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace())
                || capability.contains('/')
                || !seen.insert(capability)
            {
                return Err(AppVmLaunchRequestError::InvalidCapability);
            }
        }
        Ok(())
    }

    /// Validate the request at the App VM admission/session boundary.
    ///
    /// [`Self::validate`] checks the bounded wire shape. This stronger check is
    /// required before a request can become a Workloads/session-rail handoff:
    /// the app identity must be a reverse-DNS Flatpak identity and every
    /// capability must belong to the closed guest policy. Keeping this separate
    /// preserves the ability to parse and report a syntactically valid but
    /// unadmitted request without treating it as launchable.
    pub fn validate_admitted(&self) -> Result<(), AppVmLaunchRequestError> {
        self.validate()?;
        if !crate::app_catalog::is_valid_flatpak_app_id(&self.app_id) {
            return Err(AppVmLaunchRequestError::InvalidField("app_id"));
        }
        if self.requested_capabilities.len() > crate::cloud::APP_VM_MAX_CAPABILITIES
            || self.requested_capabilities.iter().any(|capability| {
                !crate::cloud::APP_VM_ALLOWED_CAPABILITIES.contains(&capability.as_str())
            })
        {
            return Err(AppVmLaunchRequestError::InvalidCapability);
        }
        Ok(())
    }

    /// Serialize only after construction-time validation has succeeded.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Stable Workloads identity for the guest-owned Chromium VM.
pub const BROWSER_VM_WORKLOAD_ID: &str = "browser-vm";

/// The operator-selected Browser VM display path.
///
/// RDP is the released default because the shell has an in-process IronRDP
/// client. Sunshine remains an explicit performance transport and must not be
/// selected until a seat-side Moonlight adapter is available.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVmTransport {
    /// Guest Sunshine streamed to a seat-side Moonlight client.
    Sunshine,
    /// Guest RDP rendered by the shell's live IronRDP transport.
    #[default]
    Rdp,
}

impl BrowserVmTransport {
    /// Human-readable transport label used by the shell.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sunshine => "Sunshine/Moonlight",
            Self::Rdp => "RDP",
        }
    }

    /// Session profile encoding for this transport selection.
    #[must_use]
    pub const fn session_profile(self) -> DesktopSessionProfile {
        match self {
            Self::Sunshine => DesktopSessionProfile::BrowserVm,
            Self::Rdp => DesktopSessionProfile::BrowserVmRdp,
        }
    }
}

/// Typed desktop profile carried by a brokered session open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopSessionProfile {
    /// The guest-owned Chromium Browser VM route using its default
    /// Sunshine/Moonlight transport.
    BrowserVm,
    /// The same Browser VM route with an explicit operator-selected RDP
    /// alternate for this session. This is never synthesized as a fallback.
    BrowserVmRdp,
}

impl DesktopSessionProfile {
    /// The exact Browser VM transport selected by this profile.
    #[must_use]
    pub const fn browser_transport(self) -> BrowserVmTransport {
        match self {
            Self::BrowserVm => BrowserVmTransport::Sunshine,
            Self::BrowserVmRdp => BrowserVmTransport::Rdp,
        }
    }
}

/// Why a desktop session's Browser VM profile/identity pairing is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserVmProfileError {
    /// The reserved Browser VM workload omitted its required typed profile.
    MissingProfile,
    /// A Browser VM profile was attached to a different VM identity.
    WrongWorkload,
}

impl core::fmt::Display for BrowserVmProfileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingProfile => {
                f.write_str("browser-vm requires an explicit browser_vm or browser_vm_rdp profile")
            }
            Self::WrongWorkload => f.write_str("a Browser VM profile may target only browser-vm"),
        }
    }
}

impl std::error::Error for BrowserVmProfileError {}

/// A session lifecycle request drained off the `action/vdi/session` topic — the
/// wire verb the shell / connect flow publishes. Internally tagged on `op`.
///
/// Field ids are plain strings on the wire: the broker's `SessionId` / `NodeId` /
/// `VmId` are all `= String` aliases, so this is byte-identical to the daemon's
/// former definition and to the shell's former `String`-typed mirrors (a variant's
/// tag plus its fields serialise in declaration order — see the wire-shape tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SessionRequest {
    /// Open a new session (broker state `Requested`).
    Open {
        /// The session id to mint (the roster key).
        id: String,
        /// The peer that will serve the VM (a scheduler node id).
        serving_peer: String,
        /// The target desktop: a VM desktop names the libvirt domain (the UUID
        /// isn't on the discovery wire); a **host** desktop names the peer itself.
        /// The broker's `VmId` is a plain string that accepts both.
        vm_id: String,
        /// The peer whose shell drives the desktop.
        client_peer: String,
        /// Optional typed profile intent. Omitted for legacy/generic desktop
        /// sessions so their wire shape remains byte-compatible.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<DesktopSessionProfile>,
    },
    /// Open a guest-owned Flatpak application through the same VDI lifecycle
    /// plane. These fields are policy identities, never commands, paths,
    /// environments, or sockets.
    OpenApp {
        /// The session id to mint (the roster key).
        id: String,
        /// The peer that will serve the App VM.
        serving_peer: String,
        /// The target App VM identity.
        vm_id: String,
        /// The peer whose shell drives the application surface.
        client_peer: String,
        /// Stable Flatpak app identity.
        app_id: String,
        /// Signed catalog revision selected for this launch.
        catalog_revision: String,
        /// Approved named guest profile.
        guest_profile: String,
        /// Capabilities admitted by catalog/policy.
        requested_capabilities: Vec<String>,
        /// Whether an existing guest session should be resumed.
        resume: bool,
    },
    /// Report guest/application readiness without changing the desktop link.
    AppState {
        /// The App VM session id.
        id: String,
        /// Monotonic runtime generation; zero denotes a legacy update.
        #[serde(default, skip_serializing_if = "is_zero_generation")]
        generation: u64,
        /// The latest guest/application lifecycle state.
        state: AppVmLifecycleState,
        /// Optional bounded failure or denial context.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The connect completed — mark the session `Active`.
    Active {
        /// The session id minted by the matching `Open`.
        id: String,
    },
    /// The link dropped — mark the session `Disconnected`.
    Disconnect {
        /// The session id minted by the matching `Open`.
        id: String,
    },
    /// The session ended — mark it `Closed` (terminal).
    Close {
        /// The session id minted by the matching `Open`.
        id: String,
    },
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_generation(generation: &u64) -> bool {
    *generation == 0
}

impl SessionRequest {
    /// Validate the Browser VM identity/profile pairing and return its exact
    /// transport. Generic desktop opens return `Ok(None)`; `browser-vm` without
    /// a profile and Browser profiles attached to another VM fail closed.
    /// `browser_vm` always means Sunshine, never RDP.
    ///
    /// # Errors
    /// [`BrowserVmProfileError`] when a Browser workload/profile is incomplete
    /// or attached to the wrong workload identity.
    #[must_use]
    pub fn browser_transport(&self) -> Result<Option<BrowserVmTransport>, BrowserVmProfileError> {
        match self {
            Self::Open {
                vm_id,
                profile: Some(profile),
                ..
            } if vm_id == BROWSER_VM_WORKLOAD_ID => Ok(Some(profile.browser_transport())),
            Self::Open {
                vm_id,
                profile: None,
                ..
            } if vm_id == BROWSER_VM_WORKLOAD_ID => Err(BrowserVmProfileError::MissingProfile),
            Self::Open {
                profile: Some(_), ..
            } => Err(BrowserVmProfileError::WrongWorkload),
            _ => Ok(None),
        }
    }

    /// Serialise to the `action/vdi/session` request body. A fixed derive-backed
    /// shape ⇒ serialisation can't realistically fail; an empty body (never
    /// produced here) would simply be rejected by the broker's parser.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_vm_launch_request_is_bounded_and_round_trips() {
        let request = AppVmLaunchRequest::new(
            "org.example.Editor",
            "catalog-42",
            "wayland-standard",
            vec!["audio".into(), "gpu".into()],
            "app-session-1",
            true,
        )
        .expect("valid App VM request");
        let body = request.to_body();
        let decoded: AppVmLaunchRequest = serde_json::from_str(&body).expect("round trip");
        assert_eq!(decoded, request);
        assert!(body.contains("catalog_revision"));
        assert!(!body.contains("command"));
    }

    #[test]
    fn app_vm_launch_request_rejects_untrusted_fields() {
        assert!(matches!(
            AppVmLaunchRequest::new(
                "org.example.Editor",
                "catalog",
                "/tmp/guest-image",
                vec![],
                "session",
                false,
            ),
            Err(AppVmLaunchRequestError::InvalidField("guest_profile"))
        ));
        assert_eq!(
            AppVmLaunchRequest::new(
                "org.example.Editor",
                "catalog",
                "wayland-standard",
                vec!["audio".into(), "audio".into()],
                "session",
                false,
            ),
            Err(AppVmLaunchRequestError::InvalidCapability)
        );
    }

    #[test]
    fn app_vm_launch_request_requires_admitted_identity_and_capabilities() {
        let unadmitted_identity = AppVmLaunchRequest::new(
            "host-command",
            "catalog",
            "wayland-standard",
            Vec::new(),
            "session",
            false,
        )
        .expect("identity is syntactically bounded");
        assert_eq!(
            unadmitted_identity.validate_admitted(),
            Err(AppVmLaunchRequestError::InvalidField("app_id"))
        );

        let unadmitted_capability = AppVmLaunchRequest::new(
            "org.example.Editor",
            "catalog",
            "wayland-standard",
            vec!["host_socket".into()],
            "session",
            false,
        )
        .expect("capability is syntactically bounded");
        assert_eq!(
            unadmitted_capability.validate_admitted(),
            Err(AppVmLaunchRequestError::InvalidCapability)
        );

        let admitted = AppVmLaunchRequest::new(
            "org.example.Editor",
            "catalog",
            "wayland-standard",
            vec!["audio".into(), "clipboard".into()],
            "session",
            true,
        )
        .expect("admitted request");
        assert_eq!(admitted.validate_admitted(), Ok(()));
    }

    #[test]
    fn app_vm_lifecycle_allows_idempotent_retries_but_rejects_false_jumps() {
        use AppVmLifecycleState::*;

        assert!(WaitingForPlacement.can_transition_to(WaitingForPlacement));
        assert!(WaitingForPlacement.can_transition_to(Installing));
        assert!(Installing.can_transition_to(StartingGuest));
        assert!(StartingGuest.can_transition_to(Installing));
        assert!(StartingGuest.can_transition_to(StartingApp));
        assert!(StartingApp.can_transition_to(Connected));
        assert!(Connected.can_transition_to(Reconnecting));
        assert!(Reconnecting.can_transition_to(Connected));
        assert!(Failed.can_transition_to(Installing));
        assert!(!WaitingForPlacement.can_transition_to(Connected));
        assert!(!StartingGuest.can_transition_to(Connected));
        assert!(!Denied.can_transition_to(Connected));
    }

    #[test]
    fn runtime_evidence_is_bounded_and_maps_only_guest_owned_states() {
        let evidence = AppVmRuntimeEvidence {
            session_id: "session-1".into(),
            vm_id: "app-vm-1".into(),
            app_id: "org.example.Editor".into(),
            generation: 7,
            state: AppVmRuntimeState::Connected,
            reason: Some("portal ready".into()),
        };
        evidence.validate().expect("valid runtime evidence");
        assert_eq!(
            evidence.state.lifecycle_state(),
            AppVmLifecycleState::Connected
        );
        assert!(serde_json::to_string(&evidence)
            .expect("serialize")
            .contains("connected"));

        let invalid = AppVmRuntimeEvidence {
            session_id: "../escape".into(),
            ..evidence
        };
        assert_eq!(invalid.validate(), Err("invalid App VM runtime identity"));
    }

    #[test]
    fn reconnect_runtime_evidence_rejects_legacy_generation_replays() {
        let legacy_reconnect = AppVmRuntimeEvidence {
            session_id: "session-1".into(),
            vm_id: "app-vm-1".into(),
            app_id: "org.example.Editor".into(),
            generation: 0,
            state: AppVmRuntimeState::Reconnecting,
            reason: Some("transport dropped".into()),
        };
        assert_eq!(
            legacy_reconnect.validate(),
            Err("reconnecting App VM runtime evidence requires a generation")
        );

        let numbered_reconnect = AppVmRuntimeEvidence {
            generation: 1,
            ..legacy_reconnect
        };
        numbered_reconnect
            .validate()
            .expect("reconnect evidence must identify its runtime generation");
    }

    #[test]
    fn app_vm_generation_defaults_for_legacy_payloads_and_round_trips_when_present() {
        let legacy_evidence: AppVmRuntimeEvidence = serde_json::from_str(
            r#"{"session_id":"session-1","vm_id":"app-vm-1","app_id":"org.example.Editor","state":"connected"}"#,
        )
        .expect("legacy runtime evidence");
        assert_eq!(legacy_evidence.generation, 0);

        let legacy_state: SessionRequest =
            serde_json::from_str(r#"{"op":"app_state","id":"session-1","state":"connected"}"#)
                .expect("legacy app state");
        assert!(matches!(
            legacy_state,
            SessionRequest::AppState { generation: 0, .. }
        ));

        let current = SessionRequest::AppState {
            id: "session-1".into(),
            generation: 7,
            state: AppVmLifecycleState::Connected,
            reason: None,
        };
        assert_eq!(
            current.to_body(),
            r#"{"op":"app_state","id":"session-1","generation":7,"state":"connected"}"#
        );
        assert_eq!(
            serde_json::from_str::<SessionRequest>(&current.to_body()).expect("current app state"),
            current
        );
    }

    /// Pins the exact `Open` wire bytes — the tag plus the four fields in
    /// declaration order. This is the byte-identical guarantee both the broker's
    /// `parse_request` and the shell's mirrors relied on before the fold.
    #[test]
    fn open_wire_shape_is_stable() {
        let req = SessionRequest::Open {
            id: "vdi-1-win11".into(),
            serving_peer: "anvil".into(),
            vm_id: "win11".into(),
            client_peer: "seat".into(),
            profile: None,
        };
        assert_eq!(
            req.to_body(),
            r#"{"op":"open","id":"vdi-1-win11","serving_peer":"anvil","vm_id":"win11","client_peer":"seat"}"#
        );
    }

    #[test]
    fn browser_profile_is_explicit_on_the_wire() {
        let req = SessionRequest::Open {
            id: "vdi-browser".into(),
            serving_peer: "dell".into(),
            vm_id: "browser-vm".into(),
            client_peer: "seat-15".into(),
            profile: Some(DesktopSessionProfile::BrowserVm),
        };
        assert_eq!(
            req.to_body(),
            r#"{"op":"open","id":"vdi-browser","serving_peer":"dell","vm_id":"browser-vm","client_peer":"seat-15","profile":"browser_vm"}"#
        );
        assert_eq!(
            serde_json::from_str::<SessionRequest>(&req.to_body()).expect("browser profile"),
            req
        );
        assert_eq!(
            req.browser_transport(),
            Ok(Some(BrowserVmTransport::Sunshine)),
            "the legacy browser_vm profile is the Sunshine/Moonlight default"
        );
    }

    #[test]
    fn browser_rdp_is_a_distinct_explicit_wire_profile() {
        let req = SessionRequest::Open {
            id: "vdi-browser-rdp".into(),
            serving_peer: "dell".into(),
            vm_id: BROWSER_VM_WORKLOAD_ID.into(),
            client_peer: "seat-15".into(),
            profile: Some(BrowserVmTransport::Rdp.session_profile()),
        };
        assert_eq!(
            req.to_body(),
            r#"{"op":"open","id":"vdi-browser-rdp","serving_peer":"dell","vm_id":"browser-vm","client_peer":"seat-15","profile":"browser_vm_rdp"}"#
        );
        assert_eq!(req.browser_transport(), Ok(Some(BrowserVmTransport::Rdp)));
        assert_eq!(
            serde_json::from_str::<SessionRequest>(&req.to_body()).expect("RDP alternate"),
            req
        );
    }

    #[test]
    fn browser_profile_pairing_fails_closed() {
        let missing_profile = SessionRequest::Open {
            id: "vdi-browser-untyped".into(),
            serving_peer: "dell".into(),
            vm_id: BROWSER_VM_WORKLOAD_ID.into(),
            client_peer: "seat-15".into(),
            profile: None,
        };
        assert_eq!(
            missing_profile.browser_transport(),
            Err(BrowserVmProfileError::MissingProfile)
        );

        let wrong_workload = SessionRequest::Open {
            id: "vdi-wrong-workload".into(),
            serving_peer: "dell".into(),
            vm_id: "unrelated-vm".into(),
            client_peer: "seat-15".into(),
            profile: Some(DesktopSessionProfile::BrowserVmRdp),
        };
        assert_eq!(
            wrong_workload.browser_transport(),
            Err(BrowserVmProfileError::WrongWorkload)
        );

        let generic = SessionRequest::Open {
            id: "vdi-generic".into(),
            serving_peer: "dell".into(),
            vm_id: "unrelated-vm".into(),
            client_peer: "seat-15".into(),
            profile: None,
        };
        assert_eq!(generic.browser_transport(), Ok(None));
    }

    /// Pins the three single-field lifecycle verbs.
    #[test]
    fn lifecycle_wire_shapes_are_stable() {
        assert_eq!(
            SessionRequest::Active { id: "s1".into() }.to_body(),
            r#"{"op":"active","id":"s1"}"#
        );
        assert_eq!(
            SessionRequest::Disconnect { id: "s1".into() }.to_body(),
            r#"{"op":"disconnect","id":"s1"}"#
        );
        assert_eq!(
            SessionRequest::Close { id: "s1".into() }.to_body(),
            r#"{"op":"close","id":"s1"}"#
        );
    }

    /// Every variant round-trips through the JSON boundary the broker parses.
    #[test]
    fn round_trips_every_variant() {
        let cases = [
            SessionRequest::Open {
                id: "s".into(),
                serving_peer: "p".into(),
                vm_id: "v".into(),
                client_peer: "c".into(),
                profile: None,
            },
            SessionRequest::OpenApp {
                id: "app-s".into(),
                serving_peer: "p".into(),
                vm_id: "app-vm".into(),
                client_peer: "c".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: vec!["audio".into()],
                resume: true,
            },
            SessionRequest::Active { id: "s".into() },
            SessionRequest::Disconnect { id: "s".into() },
            SessionRequest::Close { id: "s".into() },
        ];
        for c in cases {
            let body = c.to_body();
            let back: SessionRequest = serde_json::from_str(&body).expect("deserialize");
            assert_eq!(back, c);
        }
    }
}
