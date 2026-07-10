//! OW-8 / QC-15 — `mackesd onboard first-desktop`: bring up this Workstation's
//! **first cloud-backed VM desktop**.
//!
//! Workstation provisioning ends with the operator sitting in front of a running
//! desktop. The shell/DRM-boot half (E12-2/E12-3) is hardware-gated and lands in
//! its own units; THIS verb is the mackesd planner that **plans + offers the first
//! cloud-backed VM desktop**: it selects a golden image, asks the VDI broker to
//! place it as a Nova desktop, and offers the broker session the shell's Desktop
//! surface renders.
//!
//! The shape mirrors the sibling onboard verbs ([`crate::onboard::spawn_lighthouse`]
//! / [`crate::onboard::network`] and the OW-13 `recovery` verb): a pure planning
//! core the unit tests pin, plus a thin **injectable apply seam** so the live side
//! effects are faked in tests and honestly integration-gated in production.
//! * [`gather`] — impure probe: the mesh-id (founding bundle), the image catalog,
//!   and whether a brokered desktop session is already known.
//! * [`plan_first_desktop`] — pure fold: `[FirstDesktopFacts] → [FirstDesktopPlan]`,
//!   deciding **place vs reconnect vs no-image**, selecting the golden image, and
//!   building the cloud placement request.
//! * [`FirstDesktopApply`] — the injectable side-effect seam
//!   ([`FirstDesktopApply::place_desktop`] → [`FirstDesktopApply::open_session`]).
//!   Production [`LiveFirstDesktop`] returns a typed
//!   [`FirstDesktopError::IntegrationGated`] naming exactly what the live call needs
//!   (a live Nova+Heat cloud + the Bus); tests drive a recording fake.
//! * [`execute`] — pure orchestration over the seam (place → open-session),
//!   fully unit-tested through the fake.
//!
//! # Reuse, not reimplementation (§6) — glue over three existing primitives
//! This verb invents no VM/image/session model; it is glue over what the mesh has:
//! * The **golden image** comes from the PLANES-22 image catalog
//!   ([`crate::image_catalog::load_manifests`] / [`ImageManifest`]): the verb selects
//!   the newest `vm`-kind (golden desktop) manifest ([`select_golden_image`]). No
//!   VM golden image present ⇒ the honest [`FirstDesktopPlan::NoImage`] outcome
//!   (LAN-only, "see Services ▸ Provisioning ▸ Images"), never a fake success.
//! * The **VM** is a VDI broker [`DesktopSpec`](crate::workers::session_broker::DesktopSpec)
//!   in `async-services` builds: image + client peer + owner + desktop class. The
//!   broker owns Nova/Heat placement, flavor selection, metadata, and the final
//!   serving host; this unit does not build a parallel OpenStack model.
//! * The **session** is the broker's
//!   [`SessionRequest::Open`](crate::workers::session_broker::SessionRequest) wire
//!   verb on [`ACTION_TOPIC`](crate::workers::session_broker::ACTION_TOPIC): after
//!   Nova placement returns the server id and serving host, this verb emits a
//!   session-open ([`session_open_request`]) so the shell's Desktop surface renders
//!   it. The verb reuses that type verbatim — it does not invent a parallel session
//!   request.
//!
//! # This slice (QC-15): retire the local cloud-hypervisor first desktop
//! The older mde-kvm/cloud-hypervisor plan is deleted from this onboarding verb.
//! Live Nova placement and the real Bus publish land behind [`FirstDesktopApply`],
//! exactly as OW-7's live provision sits behind
//! [`crate::onboard::spawn_lighthouse::Provisioner`]. [`LiveFirstDesktop`]
//! returning a typed `IntegrationGated` error (never a fake success) is §7-legal.

use std::path::{Path, PathBuf};

use crate::image_catalog::{images_dir, ImageKind, ImageManifest};

/// The default first-desktop Nova flavor. Mirrors the broker's Standard desktop
/// class (`m1.medium`) without pulling the async worker module into lean builds.
pub const DEFAULT_DESKTOP_FLAVOR: &str = "m1.medium";

/// The broker session topic this verb's session-open publishes on.
///
/// Reuses [`crate::workers::session_broker::ACTION_TOPIC`] verbatim when the worker
/// surface is compiled in (the `async-services` build the daemon + tests use); a
/// byte-identical literal keeps the lean library build (no `async-services`)
/// compiling without pulling the worker pool in.
#[cfg(feature = "async-services")]
const ACTION_TOPIC: &str = crate::workers::session_broker::ACTION_TOPIC;
/// The broker session topic this verb's session-open publishes on (lean-build twin
/// of the [`crate::workers::session_broker::ACTION_TOPIC`] constant).
#[cfg(not(feature = "async-services"))]
const ACTION_TOPIC: &str = "action/vdi/session";

/// The parameters of the broker session-open this desktop publishes once the VM is
/// placed.
///
/// Before placement, the planned serving peer is this node; Nova placement replaces
/// it with the selected compute host. The live [`FirstDesktopApply::open_session`]
/// folds these into the broker's
/// [`SessionRequest::Open`](crate::workers::session_broker::SessionRequest) wire
/// verb verbatim (§6, see [`session_open_request`]); the shell's Desktop surface
/// then renders the session the broker tracks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SessionOpen {
    /// The session id to mint (deterministic from the VM id so the plan is pure).
    pub session_id: String,
    /// The peer serving the VM desktop.
    pub serving_peer: String,
    /// The target VM's id (the Nova server id / broker `vm_id`).
    pub vm_id: String,
    /// The peer whose shell drives the desktop — this node, for the first local one.
    pub client_peer: String,
}

/// Build the deterministic session-open for VM `vm_id` served + driven by `node_id`.
fn session_for(vm_id: &str, node_id: &str) -> SessionOpen {
    SessionOpen {
        session_id: format!("first-desktop-{vm_id}"),
        serving_peer: node_id.to_string(),
        vm_id: vm_id.to_string(),
        client_peer: node_id.to_string(),
    }
}

/// One ordered step of standing up the first desktop.
///
/// The steps *describe* the flow [`execute`] drives over the [`FirstDesktopApply`]
/// seam; a **reconnect** plan carries only [`FirstDesktopStep::OpenSession`] (the
/// desktop already exists), a **place** plan the full ordered set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FirstDesktopStep {
    /// 1. Ask the VDI broker to place the desktop as a Nova instance.
    PlaceNovaDesktop,
    /// 2. Publish the broker session-open so the shell's Desktop surface renders it.
    OpenSession,
}

impl FirstDesktopStep {
    /// The canonical ordered sequence for a fresh **place**.
    #[must_use]
    pub fn ordered_create() -> Vec<Self> {
        vec![Self::PlaceNovaDesktop, Self::OpenSession]
    }

    /// A one-line human description of the step.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::PlaceNovaDesktop => "place the desktop as a Nova instance through the VDI broker",
            Self::OpenSession => "open a broker session so the shell renders the desktop",
        }
    }
}

/// The cloud placement this onboarding verb asks the VDI broker to perform.
///
/// This is deliberately the lean-build mirror of
/// [`crate::workers::session_broker::DesktopSpec`]. In `async-services` builds,
/// [`broker_desktop_spec`] folds it into that exact worker type before placement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CloudDesktopSpec {
    /// The session id to mint for the placed desktop.
    pub session_id: String,
    /// The peer whose shell drives the desktop.
    pub client_peer: String,
    /// The owner used for quota attribution.
    pub owner: String,
    /// The Glance image to boot.
    pub image: String,
    /// The selected Nova flavor.
    pub flavor: String,
}

/// Why the first desktop cannot be offered as a fresh create right now — a real,
/// honest outcome (not a failure): the mesh image catalog holds no VM golden image
/// to place a desktop from yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum NoImageReason {
    /// The catalog holds no `vm`-kind (golden desktop) manifest.
    NoVmImage,
}

impl NoImageReason {
    /// What the operator does to unblock a retry.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::NoVmImage => {
                "build or replicate a VM image into Glance (Services ▸ Provisioning ▸ Images), then retry"
            }
        }
    }
}

impl std::fmt::Display for NoImageReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoVmImage => f.write_str("no VM golden image in the catalog"),
        }
    }
}

/// A resolved first-desktop plan — the headless body the CLI prints and [`execute`]
/// drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstDesktopPlan {
    /// No desktop exists yet and a golden image is available → place a fresh Nova
    /// desktop, then open its broker session.
    Create {
        /// The mesh this desktop's VM enrolls into.
        mesh_id: String,
        /// The selected golden VM image (from the catalog). Boxed to keep this
        /// variant size-balanced against the tiny `NoImage` one.
        image: Box<ImageManifest>,
        /// The cloud desktop placement request. Boxed to
        /// keep this variant size-balanced against the tiny `NoImage` one.
        desktop: Box<CloudDesktopSpec>,
        /// The ordered create steps.
        steps: Vec<FirstDesktopStep>,
        /// The planned session-open; the live placement overwrites `vm_id` and
        /// `serving_peer` with Nova's returned server id and compute host.
        session: SessionOpen,
    },
    /// A desktop VM already exists → **offer the existing one** (reconnect),
    /// never a duplicate. Only the broker session is re-opened.
    Reconnect {
        /// The mesh the existing desktop's VM belongs to.
        mesh_id: String,
        /// The existing VM's id.
        vm_id: String,
        /// The reconnect steps (just the session re-open).
        steps: Vec<FirstDesktopStep>,
        /// The session-open re-published for the existing VM.
        session: SessionOpen,
    },
    /// No VM golden image is available → the mesh stays LAN-only for now; the
    /// operator retries once a golden image is present (see [`NoImageReason::hint`]).
    NoImage {
        /// Why no fresh desktop can be created (and, via the hint, the fix).
        reason: NoImageReason,
    },
}

impl FirstDesktopPlan {
    /// The ordered steps this plan drives (empty for [`FirstDesktopPlan::NoImage`]).
    #[must_use]
    pub fn steps(&self) -> &[FirstDesktopStep] {
        match self {
            Self::Create { steps, .. } | Self::Reconnect { steps, .. } => steps,
            Self::NoImage { .. } => &[],
        }
    }

    /// Whether this plan places a fresh VM (else reconnect / no-image).
    #[must_use]
    pub const fn is_create(&self) -> bool {
        matches!(self, Self::Create { .. })
    }

    /// Whether this plan reconnects to an existing VM (offers it, not a duplicate).
    #[must_use]
    pub const fn is_reconnect(&self) -> bool {
        matches!(self, Self::Reconnect { .. })
    }

    /// A one-line human summary (no trailing newline — the CLI wraps it in
    /// `println!`, mirroring the sibling verbs).
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Self::Create {
                mesh_id,
                image,
                desktop,
                steps,
                ..
            } => format!(
                "create the first desktop for mesh `{mesh_id}` from golden image \
                 `{}` v{} as Nova flavor {}, then open a session in {} step(s)",
                image.name,
                image.version,
                desktop.flavor,
                steps.len()
            ),
            Self::Reconnect { mesh_id, vm_id, .. } => format!(
                "reconnect to the existing desktop VM `{vm_id}` on mesh `{mesh_id}` — \
                 re-open its session (no duplicate created)"
            ),
            Self::NoImage { reason } => format!(
                "no VM golden image available ({reason}) — the mesh stays LAN-only for now; {}",
                reason.hint()
            ),
        }
    }
}

/// The live facts [`gather`] reads off this node — the seam between the impure
/// probes and the pure [`plan_first_desktop`] fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstDesktopFacts {
    /// This mesh's id (from the founding bundle) — the desktop VM enrolls into it.
    pub mesh_id: String,
    /// This node's id (names the VM + its tap devices + the session peers).
    pub node_id: String,
    /// An already-known desktop VM's id, if one exists → reconnect instead of
    /// creating a duplicate. `None` ⇒ plan a fresh placement (when an image exists).
    pub existing_desktop: Option<String>,
    /// The mesh image catalog (from [`crate::image_catalog::load_manifests`]) — the
    /// planner selects the golden VM image from it, or resolves [`NoImageReason`].
    pub catalog: Vec<ImageManifest>,
    /// The shared workgroup root — the image catalog resolves under its `images/`
    /// dir.
    pub workgroup_root: PathBuf,
}

/// Pure fold: turn gathered [`FirstDesktopFacts`] into a [`FirstDesktopPlan`]. No
/// I/O — fully unit-testable.
///
/// The three branches: an already-existing desktop VM ⇒
/// [`FirstDesktopPlan::Reconnect`] (offer it, never a duplicate); no VM golden image
/// in the catalog ⇒ the honest [`FirstDesktopPlan::NoImage`]; otherwise select the
/// golden image, build a cloud placement request, and plan the ordered place →
/// [`FirstDesktopPlan::Create`].
#[must_use]
pub fn plan_first_desktop(facts: &FirstDesktopFacts) -> FirstDesktopPlan {
    // A desktop VM already exists → offer it (reconnect), never a duplicate.
    if let Some(vm_id) = &facts.existing_desktop {
        return FirstDesktopPlan::Reconnect {
            mesh_id: facts.mesh_id.clone(),
            vm_id: vm_id.clone(),
            steps: vec![FirstDesktopStep::OpenSession],
            session: session_for(vm_id, &facts.node_id),
        };
    }
    // No VM golden image in the catalog → an honest NoImage outcome (not a fail).
    let Some(image) = select_golden_image(&facts.catalog) else {
        return FirstDesktopPlan::NoImage {
            reason: NoImageReason::NoVmImage,
        };
    };
    let vm_name = desktop_vm_name(&facts.node_id);
    let desktop = build_cloud_desktop_spec(&vm_name, &facts.node_id, &image);
    let session = session_for(&vm_name, &facts.node_id);
    FirstDesktopPlan::Create {
        mesh_id: facts.mesh_id.clone(),
        image: Box::new(image),
        desktop: Box::new(desktop),
        steps: FirstDesktopStep::ordered_create(),
        session,
    }
}

/// Pure: pick the golden **VM** image to boot the first desktop from — the newest
/// `vm`-kind manifest, preferring one whose baked-in profile is `workstation` (a
/// desktop image) when several exist.
///
/// Returns `None` when the catalog holds no VM-kind manifest at all (→
/// [`NoImageReason::NoVmImage`]). The catalog is newest-first
/// ([`crate::image_catalog::load_manifests`] sorts by build time), so the first
/// match is the newest.
#[must_use]
pub fn select_golden_image(catalog: &[ImageManifest]) -> Option<ImageManifest> {
    let is_vm = |m: &&ImageManifest| ImageKind::parse(&m.kind) == Some(ImageKind::Vm);
    catalog
        .iter()
        .filter(is_vm)
        .find(|m| m.profile.as_deref() == Some("workstation"))
        .or_else(|| catalog.iter().find(is_vm))
        .cloned()
}

/// Pure: the image artifact path for `image` inside the mesh image catalog
/// (`<root>/images/<name>/<version>/<name>.img`). The current Nova path uses the
/// image manifest name as the Glance image, but this helper keeps the catalog path
/// explicit for audits and dry-run output.
#[must_use]
pub fn golden_base_disk(workgroup_root: &Path, image: &ImageManifest) -> PathBuf {
    images_dir(workgroup_root)
        .join(&image.name)
        .join(&image.version)
        .join(format!("{}.img", image.name))
}

/// Pure: a filesystem-safe VM name for `node_id` (`desktop-<slug>`). The name drives
/// the VM's socket paths + running-disk name, so it is reduced to `[a-z0-9-]`.
#[must_use]
pub fn desktop_vm_name(node_id: &str) -> String {
    // Drop a `peer:` (or any) prefix, then slugify.
    let raw = node_id.rsplit(':').next().unwrap_or(node_id);
    let slug: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "desktop-node".to_string()
    } else {
        format!("desktop-{trimmed}")
    }
}

/// Pure: build the cloud placement request for the first desktop `vm_name`.
#[must_use]
pub fn build_cloud_desktop_spec(
    vm_name: &str,
    node_id: &str,
    image: &ImageManifest,
) -> CloudDesktopSpec {
    CloudDesktopSpec {
        session_id: format!("first-desktop-{vm_name}"),
        client_peer: node_id.to_string(),
        owner: node_id.to_string(),
        image: image.name.clone(),
        flavor: DEFAULT_DESKTOP_FLAVOR.to_string(),
    }
}

/// The desktop a [`FirstDesktopApply::place_desktop`] placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootedDesktop {
    /// The placed Nova server id — the broker session points at this.
    pub vm_id: String,
    /// The compute host Nova placed the desktop on — the session's serving peer.
    pub serving_peer: String,
}

/// A typed failure from the injectable [`FirstDesktopApply`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstDesktopError {
    /// The live path is not runnable in this build/environment yet — it needs a real
    /// prerequisite (a live Nova+Heat cloud / the Bus).
    /// Names the step + what is missing. §7-legal: a real method returning a real
    /// typed error, exactly as OW-7's [`crate::onboard::spawn_lighthouse`] seam does.
    IntegrationGated {
        /// Which seam step (`place-desktop` / `open-session`).
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

impl std::fmt::Display for FirstDesktopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { step, reason } => {
                write!(f, "{step}: integration-gated — {reason}")
            }
            Self::Failed { step, reason } => write!(f, "{step}: {reason}"),
        }
    }
}

impl std::error::Error for FirstDesktopError {}

/// The injectable side-effect seam. Production is [`LiveFirstDesktop`]; tests use a
/// recording fake so the pure orchestration is exercised without a real Nova
/// placement / Bus publish.
pub trait FirstDesktopApply {
    /// Place the desktop as a Nova instance.
    ///
    /// # Errors
    /// A [`FirstDesktopError`] — `IntegrationGated` until a live Nova+Heat cloud is
    /// reachable, else `Failed`.
    fn place_desktop(&self, desktop: &CloudDesktopSpec)
        -> Result<BootedDesktop, FirstDesktopError>;

    /// Publish the broker session-open so the shell's Desktop surface renders the
    /// desktop.
    ///
    /// # Errors
    /// A [`FirstDesktopError`] — `IntegrationGated` until the live Bus is reachable,
    /// else `Failed`.
    fn open_session(&self, open: &SessionOpen) -> Result<(), FirstDesktopError>;
}

/// Fold a [`SessionOpen`] into the broker's
/// [`SessionRequest::Open`](crate::workers::session_broker::SessionRequest) wire
/// verb — the exact type [`crate::workers::session_broker`] folds into the roaming
/// session roster (§6 reuse: this verb does NOT invent a parallel session type).
///
/// The live Bus publisher (and the tests) send THIS on
/// [`ACTION_TOPIC`](crate::workers::session_broker::ACTION_TOPIC). Compiled only in
/// the `async-services` build that carries the worker surface.
#[cfg(feature = "async-services")]
#[must_use]
pub fn session_open_request(open: &SessionOpen) -> crate::workers::session_broker::SessionRequest {
    crate::workers::session_broker::SessionRequest::Open {
        id: open.session_id.clone(),
        serving_peer: open.serving_peer.clone(),
        vm_id: open.vm_id.clone(),
        client_peer: open.client_peer.clone(),
    }
}

/// Production [`FirstDesktopApply`] — live Nova placement + Bus session publish.
///
/// OW-8's **open-session** is a **day-2** remote push (the serving host is an
/// enrolled mesh member): [`open_session`](Self::open_session) drives the shared
/// OW-15 [`RemotePush`](crate::onboard::remote_push::RemotePush) executor over the
/// §9-native [`BusApply`](crate::onboard::remote_push::BusApply) transport (an
/// [`Action::OpenBroker`](crate::onboard::remote_push::Action::OpenBroker) to the
/// serving peer). The transport is an **injectable seam** (default: the
/// honestly-gated production `BusApply`; tests use a fake), and the live cross-node
/// round-trip stays operator/live-gated (§7).
///
/// `place_desktop` drives the same Nova placement seam the session broker owns; it
/// stays honestly integration-gated until a live Nova+Heat cloud is reachable.
pub struct LiveFirstDesktop {
    /// The OW-15 day-2 remote-push transport. Default: [`BusApply`]; tests inject a
    /// recording fake to prove the wiring without a live round-trip.
    ///
    /// [`BusApply`]: crate::onboard::remote_push::BusApply
    remote_push: std::sync::Arc<dyn crate::onboard::remote_push::RemotePush + Send + Sync>,
}

impl Default for LiveFirstDesktop {
    fn default() -> Self {
        Self {
            remote_push: std::sync::Arc::new(crate::onboard::remote_push::BusApply),
        }
    }
}

impl LiveFirstDesktop {
    /// Inject the remote-push transport (tests use a recording fake).
    #[must_use]
    pub fn with_remote_push(
        mut self,
        transport: std::sync::Arc<dyn crate::onboard::remote_push::RemotePush + Send + Sync>,
    ) -> Self {
        self.remote_push = transport;
        self
    }

    #[cfg(feature = "async-services")]
    fn place_desktop_impl(
        &self,
        desktop: &CloudDesktopSpec,
    ) -> Result<BootedDesktop, FirstDesktopError> {
        use crate::workers::session_broker::NovaPlacement as _;

        let spec = broker_desktop_spec(desktop);
        let req = crate::workers::session_broker::build_placement(&spec);
        crate::workers::session_broker::LiveNovaPlacement::new()
            .place(&req)
            .map(|placed| BootedDesktop {
                vm_id: placed.vm_id,
                serving_peer: placed.serving_host,
            })
            .map_err(|e| match e {
                crate::workers::session_broker::PlacementError::IntegrationGated {
                    verb,
                    reason,
                } => FirstDesktopError::IntegrationGated {
                    step: "place-desktop",
                    reason: format!(
                        "desktop session `{}` → needs the live Nova placement path \
                         (`{verb}` on `{}`): {reason}",
                        desktop.session_id,
                        req.topic()
                    ),
                },
                crate::workers::session_broker::PlacementError::Failed { verb, reason } => {
                    FirstDesktopError::Failed {
                        step: "place-desktop",
                        reason: format!("{verb}: {reason}"),
                    }
                }
            })
    }

    #[cfg(not(feature = "async-services"))]
    fn place_desktop_impl(
        &self,
        desktop: &CloudDesktopSpec,
    ) -> Result<BootedDesktop, FirstDesktopError> {
        Err(FirstDesktopError::IntegrationGated {
            step: "place-desktop",
            reason: format!(
                "desktop session `{}` → needs the async-services VDI broker and \
                 live Nova+Heat placement path",
                desktop.session_id
            ),
        })
    }
}

/// Fold the lean first-desktop request into the session broker's exact placement
/// wire type. This is the §6 reuse proof for QC-15: first-desktop no longer owns a
/// local hypervisor VM model.
#[cfg(feature = "async-services")]
#[must_use]
pub fn broker_desktop_spec(
    desktop: &CloudDesktopSpec,
) -> crate::workers::session_broker::DesktopSpec {
    crate::workers::session_broker::DesktopSpec {
        session_id: desktop.session_id.clone(),
        client_peer: desktop.client_peer.clone(),
        owner: desktop.owner.clone(),
        class: crate::workers::session_broker::DesktopClass::Standard,
        image: desktop.image.clone(),
        network: None,
        mode: crate::workers::session_broker::PlacementMode::Create,
    }
}

impl FirstDesktopApply for LiveFirstDesktop {
    fn place_desktop(
        &self,
        desktop: &CloudDesktopSpec,
    ) -> Result<BootedDesktop, FirstDesktopError> {
        self.place_desktop_impl(desktop)
    }

    fn open_session(&self, open: &SessionOpen) -> Result<(), FirstDesktopError> {
        // OW-15 day-2 remote push: ask the serving peer (an enrolled mesh member)
        // to open the broker session over the §9 BusApply transport.
        let target = crate::onboard::remote_push::Target::Enrolled {
            node_id: open.serving_peer.clone(),
        };
        let actions = [crate::onboard::remote_push::Action::OpenBroker {
            session_id: open.session_id.clone(),
        }];
        self.remote_push.apply(&target, &actions).map_err(|e| {
            use crate::onboard::remote_push::RemotePushError as R;
            let detail = e.to_string();
            match e {
                R::NotWired { .. } | R::Unreachable { .. } => FirstDesktopError::IntegrationGated {
                    step: "open-session",
                    reason: format!(
                        "session `{}` → needs the live Bus to publish a broker \
                         SessionRequest::Open on `{ACTION_TOPIC}` (over the §9 BusApply transport: \
                         {detail}) so the shell's Desktop surface renders VM `{}`",
                        open.session_id, open.vm_id
                    ),
                },
                R::BundleRejected { why } => FirstDesktopError::Failed {
                    step: "open-session",
                    reason: why,
                },
                R::ActionFailed { action, why } => FirstDesktopError::Failed {
                    step: "open-session",
                    reason: format!("{action}: {why}"),
                },
            }
        })
    }
}

/// The result of an [`execute`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstDesktopOutcome {
    /// A fresh desktop VM was placed + its session opened.
    Created {
        /// The placed VM's id.
        vm_id: String,
    },
    /// An existing desktop VM was offered again (its session re-opened).
    Reconnected {
        /// The existing VM's id.
        vm_id: String,
    },
    /// The plan was no-image — nothing was created; a retry is available.
    NoImage {
        /// Why no fresh desktop could be created.
        reason: NoImageReason,
    },
}

impl FirstDesktopOutcome {
    /// A one-line human summary (no trailing newline).
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            Self::Created { vm_id } => format!("first desktop `{vm_id}` placed + session opened"),
            Self::Reconnected { vm_id } => {
                format!("reconnected to existing desktop `{vm_id}` (session re-opened)")
            }
            Self::NoImage { reason } => {
                format!("no-op — no golden image ({reason}); retry available")
            }
        }
    }
}

/// Pure orchestration over the [`FirstDesktopApply`] seam.
///
/// For [`FirstDesktopPlan::Create`] run placement → open-session **in that order**
/// (the session points at Nova's placed server id, mirroring how OW-7 threads
/// `provision`'s endpoint into `push-enroll`); for [`FirstDesktopPlan::Reconnect`]
/// only re-open the session (the VM already exists); for [`FirstDesktopPlan::NoImage`]
/// short-circuit to the retryable outcome (no seam calls).
///
/// This is the tested orchestration the fake pins; the real side effects live
/// entirely in the injected `apply`.
///
/// # Errors
/// Propagates the first [`FirstDesktopError`] any seam step returns.
pub fn execute(
    plan: &FirstDesktopPlan,
    apply: &dyn FirstDesktopApply,
) -> Result<FirstDesktopOutcome, FirstDesktopError> {
    match plan {
        FirstDesktopPlan::NoImage { reason } => {
            Ok(FirstDesktopOutcome::NoImage { reason: *reason })
        }
        FirstDesktopPlan::Reconnect { vm_id, session, .. } => {
            apply.open_session(session)?;
            Ok(FirstDesktopOutcome::Reconnected {
                vm_id: vm_id.clone(),
            })
        }
        FirstDesktopPlan::Create {
            desktop, session, ..
        } => {
            let placed = apply.place_desktop(desktop)?;
            // The session points at the PLACED VM (Nova is the authority on the
            // server id and compute host), not the planned name.
            let open = SessionOpen {
                vm_id: placed.vm_id.clone(),
                serving_peer: placed.serving_peer.clone(),
                ..session.clone()
            };
            apply.open_session(&open)?;
            Ok(FirstDesktopOutcome::Created {
                vm_id: placed.vm_id,
            })
        }
    }
}

/// Impure probe shell: gather the live first-desktop facts off this node.
///
/// Best-effort — a missing bundle / empty catalog degrades to `None`/empty fields
/// rather than erroring, so the pure [`plan_first_desktop`] fold always runs and
/// produces the real verdict (`NoImage` when no golden image exists). The mesh-id
/// comes from the founding bundle, the catalog from the mesh image catalog, and the
/// existing-desktop signal is supplied by callers that already know about a live
/// brokered desktop; this lean probe does not infer one from retired local disks.
#[must_use]
pub fn gather(workgroup_root: &Path, node_id: &str) -> FirstDesktopFacts {
    let mesh_id = crate::onboard::invite::resolve_mesh_id(workgroup_root, node_id);
    let catalog = crate::image_catalog::load_manifests(workgroup_root);
    FirstDesktopFacts {
        mesh_id,
        node_id: node_id.to_string(),
        existing_desktop: None,
        catalog,
        workgroup_root: workgroup_root.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn manifest(name: &str, kind: &str, ver: &str, profile: Option<&str>) -> ImageManifest {
        ImageManifest {
            name: name.into(),
            kind: kind.into(),
            version: ver.into(),
            built_at_ms: Some(1_700_000_000_000),
            size_bytes: Some(4096),
            profile: profile.map(str::to_string),
        }
    }

    fn facts(existing: Option<&str>, catalog: Vec<ImageManifest>) -> FirstDesktopFacts {
        FirstDesktopFacts {
            mesh_id: "home-deadbeef".into(),
            node_id: "peer:eagle".into(),
            existing_desktop: existing.map(str::to_string),
            catalog,
            workgroup_root: PathBuf::from("/mnt/mesh-storage"),
        }
    }

    // ── the three planner branches: create / reconnect / no-image ──

    #[test]
    fn create_selects_the_vm_image_and_builds_a_nova_desktop_placement() {
        let plan = plan_first_desktop(&facts(
            None,
            vec![
                manifest("cosmic-iso", "iso", "1.0", None),
                manifest("win10-gold", "vm", "3.2", Some("workstation")),
            ],
        ));
        let FirstDesktopPlan::Create {
            mesh_id,
            image,
            desktop,
            steps,
            session,
        } = &plan
        else {
            panic!("expected a Create plan, got {plan:?}");
        };
        assert_eq!(mesh_id, "home-deadbeef");
        // Selected the VM-kind golden image (not the ISO).
        assert_eq!(image.kind, "vm");
        assert_eq!(image.name, "win10-gold");
        assert_eq!(desktop.session_id, "first-desktop-desktop-eagle");
        assert_eq!(desktop.client_peer, "peer:eagle");
        assert_eq!(desktop.owner, "peer:eagle");
        assert_eq!(desktop.image, "win10-gold");
        assert_eq!(desktop.flavor, DEFAULT_DESKTOP_FLAVOR);
        // The planned session starts with the stable desktop name; live Nova
        // placement overwrites vm_id + serving_peer with the returned server.
        assert_eq!(session.serving_peer, "peer:eagle");
        assert_eq!(session.client_peer, "peer:eagle");
        assert_eq!(session.vm_id, "desktop-eagle");
        assert_eq!(steps, &FirstDesktopStep::ordered_create());
        assert!(plan.is_create());
        assert!(plan.human().contains("create the first desktop"));
    }

    #[test]
    fn reconnect_when_a_brokered_desktop_vm_already_exists() {
        // A desktop already exists → offer it, never a duplicate.
        let plan = plan_first_desktop(&facts(
            Some("desktop-eagle"),
            vec![manifest("win10-gold", "vm", "3.2", Some("workstation"))],
        ));
        let FirstDesktopPlan::Reconnect {
            vm_id,
            steps,
            session,
            ..
        } = &plan
        else {
            panic!("expected a Reconnect plan, got {plan:?}");
        };
        assert_eq!(vm_id, "desktop-eagle");
        assert_eq!(
            steps,
            &[FirstDesktopStep::OpenSession],
            "reconnect only re-opens the session"
        );
        assert_eq!(session.vm_id, "desktop-eagle");
        assert!(plan.is_reconnect());
        assert!(plan.human().contains("no duplicate"));
    }

    #[test]
    fn no_image_when_the_catalog_has_no_vm_manifest() {
        // ISO/USB/container present, but no VM golden image → honest NoImage.
        let plan = plan_first_desktop(&facts(
            None,
            vec![
                manifest("cosmic-iso", "iso", "1.0", None),
                manifest("writer", "usb", "1.0", None),
                manifest("mesh-svc", "container", "1.0", None),
            ],
        ));
        assert_eq!(
            plan,
            FirstDesktopPlan::NoImage {
                reason: NoImageReason::NoVmImage
            }
        );
        assert!(plan.steps().is_empty());
        assert!(plan.human().contains("no VM golden image available"));
        assert!(
            plan.human().contains("Images"),
            "the hint points at Services ▸ Images"
        );
    }

    // ── the golden-image selection ──

    #[test]
    fn select_golden_image_prefers_workstation_then_falls_back_to_any_vm() {
        // Prefers the workstation-profile VM even when a plain VM is newer.
        let with_ws = [
            manifest("plain-vm", "vm", "9.0", None),
            manifest("desk-vm", "vm", "1.0", Some("workstation")),
        ];
        assert_eq!(
            select_golden_image(&with_ws).map(|m| m.name),
            Some("desk-vm".into())
        );
        // No workstation profile ⇒ falls back to the first (newest) VM-kind.
        let no_ws = [
            manifest("iso-a", "iso", "1.0", None),
            manifest("plain-vm", "vm", "9.0", None),
        ];
        assert_eq!(
            select_golden_image(&no_ws).map(|m| m.name),
            Some("plain-vm".into())
        );
        // No VM at all ⇒ None.
        assert!(select_golden_image(&[manifest("iso-a", "iso", "1.0", None)]).is_none());
        assert!(select_golden_image(&[]).is_none());
    }

    #[test]
    fn golden_base_disk_is_under_the_versioned_image_dir() {
        let m = manifest("win10-gold", "vm", "3.2", Some("workstation"));
        assert_eq!(
            golden_base_disk(Path::new("/mnt/mesh-storage"), &m),
            PathBuf::from("/mnt/mesh-storage/images/win10-gold/3.2/win10-gold.img")
        );
    }

    // ── the VM/session name + placement builders ──

    #[test]
    fn desktop_vm_name_slugifies_and_drops_the_peer_prefix() {
        assert_eq!(desktop_vm_name("peer:eagle"), "desktop-eagle");
        assert_eq!(desktop_vm_name("peer:Big Boy!"), "desktop-big-boy");
        assert_eq!(desktop_vm_name("plainhost"), "desktop-plainhost");
        // Degenerate ids never yield an empty / unsafe name.
        assert_eq!(desktop_vm_name("peer:"), "desktop-node");
        assert_eq!(desktop_vm_name("::"), "desktop-node");
    }

    #[test]
    fn build_cloud_desktop_spec_targets_the_broker_placement_shape() {
        let image = manifest("win10-gold", "vm", "3.2", Some("workstation"));
        let spec = build_cloud_desktop_spec("desktop-eagle", "peer:eagle", &image);
        assert_eq!(spec.session_id, "first-desktop-desktop-eagle");
        assert_eq!(spec.client_peer, "peer:eagle");
        assert_eq!(spec.owner, "peer:eagle");
        assert_eq!(spec.image, "win10-gold");
        assert_eq!(spec.flavor, DEFAULT_DESKTOP_FLAVOR);
    }

    #[test]
    fn create_steps_are_ordered_and_described() {
        let steps = FirstDesktopStep::ordered_create();
        assert_eq!(
            steps,
            vec![
                FirstDesktopStep::PlaceNovaDesktop,
                FirstDesktopStep::OpenSession
            ]
        );
        // Placement must precede opening the display session.
        assert!(steps.iter().all(|s| !s.describe().is_empty()));
    }

    // ── execute over the seam (recording fake) ──

    /// Recording [`FirstDesktopApply`] fake: records the ordered calls + what it saw
    /// so the pure orchestration is asserted without a real placement / Bus publish.
    struct FakeApply {
        calls: RefCell<Vec<&'static str>>,
        seen_desktop: RefCell<Option<CloudDesktopSpec>>,
        seen_open: RefCell<Option<SessionOpen>>,
        placed_id: String,
        serving_peer: String,
    }

    impl FakeApply {
        fn new(placed_id: &str, serving_peer: &str) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                seen_desktop: RefCell::new(None),
                seen_open: RefCell::new(None),
                placed_id: placed_id.to_string(),
                serving_peer: serving_peer.to_string(),
            }
        }
    }

    impl FirstDesktopApply for FakeApply {
        fn place_desktop(
            &self,
            desktop: &CloudDesktopSpec,
        ) -> Result<BootedDesktop, FirstDesktopError> {
            self.calls.borrow_mut().push("place_desktop");
            *self.seen_desktop.borrow_mut() = Some(desktop.clone());
            Ok(BootedDesktop {
                vm_id: self.placed_id.clone(),
                serving_peer: self.serving_peer.clone(),
            })
        }
        fn open_session(&self, open: &SessionOpen) -> Result<(), FirstDesktopError> {
            self.calls.borrow_mut().push("open_session");
            *self.seen_open.borrow_mut() = Some(open.clone());
            Ok(())
        }
    }

    #[test]
    fn execute_create_drives_place_desktop_then_open_session() {
        let plan = plan_first_desktop(&facts(
            None,
            vec![manifest("win10-gold", "vm", "3.2", Some("workstation"))],
        ));
        let apply = FakeApply::new("uuid-boot", "peer:compute-3");
        let outcome = execute(&plan, &apply).expect("execute");
        assert_eq!(
            outcome,
            FirstDesktopOutcome::Created {
                vm_id: "uuid-boot".into()
            }
        );
        // Seam ran place_desktop → open_session, in that order.
        assert_eq!(*apply.calls.borrow(), vec!["place_desktop", "open_session"]);
        // It saw the broker placement request.
        assert_eq!(
            apply
                .seen_desktop
                .borrow()
                .as_ref()
                .map(|s| s.image.clone()),
            Some("win10-gold".into())
        );
        // The session opened points at the placed Nova server and compute host.
        assert_eq!(
            apply.seen_open.borrow().as_ref().map(|o| o.vm_id.clone()),
            Some("uuid-boot".into())
        );
        assert_eq!(
            apply
                .seen_open
                .borrow()
                .as_ref()
                .map(|o| o.serving_peer.clone()),
            Some("peer:compute-3".into())
        );
    }

    #[test]
    fn execute_reconnect_only_opens_a_session() {
        let plan = plan_first_desktop(&facts(
            Some("desktop-eagle"),
            vec![manifest("win10-gold", "vm", "3.2", Some("workstation"))],
        ));
        let apply = FakeApply::new("desktop-eagle", "peer:eagle");
        let outcome = execute(&plan, &apply).expect("execute");
        assert_eq!(
            outcome,
            FirstDesktopOutcome::Reconnected {
                vm_id: "desktop-eagle".into()
            }
        );
        // No VM created — just the session re-open.
        assert_eq!(*apply.calls.borrow(), vec!["open_session"]);
    }

    #[test]
    fn execute_no_image_makes_no_seam_calls() {
        let plan = plan_first_desktop(&facts(None, vec![manifest("iso-a", "iso", "1.0", None)]));
        let apply = FakeApply::new("desktop-eagle", "peer:eagle");
        let outcome = execute(&plan, &apply).expect("execute");
        assert_eq!(
            outcome,
            FirstDesktopOutcome::NoImage {
                reason: NoImageReason::NoVmImage
            }
        );
        assert!(apply.calls.borrow().is_empty(), "no seam calls on no-image");
    }

    // ── the production seam is integration-gated, never a fake success ──

    #[test]
    fn live_first_desktop_is_integration_gated_not_fake_success() {
        let apply = LiveFirstDesktop::default();
        let image = manifest("win10-gold", "vm", "3.2", Some("workstation"));
        let desktop = build_cloud_desktop_spec("desktop-eagle", "peer:eagle", &image);
        let err = apply
            .place_desktop(&desktop)
            .expect_err("live placement must not fake success");
        match err {
            FirstDesktopError::IntegrationGated { step, reason } => {
                assert_eq!(step, "place-desktop");
                assert!(
                    reason.contains("Nova") || reason.contains("async-services"),
                    "names the missing cloud placement path: {reason}"
                );
                assert!(
                    reason.contains("first-desktop-desktop-eagle"),
                    "names the session: {reason}"
                );
            }
            FirstDesktopError::Failed { .. } => panic!("expected an integration-gated error"),
        }
        let open = session_for("desktop-eagle", "peer:eagle");
        match apply
            .open_session(&open)
            .expect_err("live session publish is gated")
        {
            FirstDesktopError::IntegrationGated { step, reason } => {
                assert_eq!(step, "open-session");
                assert!(reason.contains("Bus"), "names the missing Bus: {reason}");
                assert!(
                    reason.contains("action/vdi/session"),
                    "names the broker topic: {reason}"
                );
            }
            FirstDesktopError::Failed { .. } => panic!("expected an integration-gated error"),
        }
    }

    #[test]
    fn execute_propagates_the_integration_gated_error() {
        // Through the LIVE seam, execute surfaces the first typed error.
        let plan = plan_first_desktop(&facts(
            None,
            vec![manifest("win10-gold", "vm", "3.2", Some("workstation"))],
        ));
        let err = execute(&plan, &LiveFirstDesktop::default()).expect_err("live path is gated");
        assert!(matches!(
            err,
            FirstDesktopError::IntegrationGated {
                step: "place-desktop",
                ..
            }
        ));
    }

    // ── OW-15 wiring: open_session drives the day-2 RemotePush (fake) ──

    #[test]
    fn open_session_drives_the_remote_push_with_open_broker() {
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
        let apply = LiveFirstDesktop::default().with_remote_push(push.clone());
        let open = session_for("desktop-eagle", "peer:eagle");
        apply
            .open_session(&open)
            .expect("fake transport ⇒ wiring proven");

        let seen = push.seen.lock().expect("seen mutex");
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].0,
            Target::Enrolled {
                node_id: open.serving_peer.clone()
            },
            "day-2 broker open targets the enrolled serving peer"
        );
        assert!(matches!(&seen[0].1[0], Action::OpenBroker { .. }));
    }

    #[cfg(feature = "async-services")]
    #[test]
    fn cloud_desktop_spec_maps_to_the_broker_desktop_spec() {
        use crate::workers::session_broker::{DesktopClass, PlacementMode};

        let image = manifest("win10-gold", "vm", "3.2", Some("workstation"));
        let desktop = build_cloud_desktop_spec("desktop-eagle", "peer:eagle", &image);
        let broker = broker_desktop_spec(&desktop);
        assert_eq!(broker.session_id, "first-desktop-desktop-eagle");
        assert_eq!(broker.client_peer, "peer:eagle");
        assert_eq!(broker.owner, "peer:eagle");
        assert_eq!(broker.class, DesktopClass::Standard);
        assert_eq!(broker.image, "win10-gold");
        assert_eq!(broker.network, None);
        assert_eq!(broker.mode, PlacementMode::Create);
    }

    #[test]
    fn session_open_serde_round_trips() {
        let open = session_for("desktop-eagle", "peer:eagle");
        let json = serde_json::to_string(&open).expect("serialize");
        assert!(json.contains("desktop-eagle"));
        assert!(json.contains("first-desktop-desktop-eagle"));
    }

    // ── §6 reuse: the session-open folds into the broker's wire verb verbatim ──
    #[cfg(feature = "async-services")]
    #[test]
    fn session_open_maps_to_the_broker_session_request_open_verb() {
        use crate::workers::session_broker::SessionRequest;
        let open = session_for("desktop-eagle", "peer:eagle");
        let req = session_open_request(&open);
        // Reuses SessionRequest::Open verbatim (no parallel session type invented).
        assert_eq!(
            req,
            SessionRequest::Open {
                id: "first-desktop-desktop-eagle".into(),
                serving_peer: "peer:eagle".into(),
                vm_id: "desktop-eagle".into(),
                client_peer: "peer:eagle".into(),
            }
        );
    }
}
