//! The plain data the [`MeshView`](crate::MeshView) renders.
//!
//! These are *just data* — no rendering, no egui context, no mesh-substrate
//! dependency. A caller (the shell, a panel, the example) builds a [`MeshState`]
//! from whatever live source it has (`mackesd peers`, the registry, …) and hands
//! it to the widget each frame. The widget draws **only** what is in here.

use mde_egui::egui::Vec2;

/// A node's place in the mesh hierarchy.
///
/// Determines its drawn size (Lighthouse largest → Workstation smallest) and
/// where the auto-layout puts it (a lighthouse clusters at the centre; peers
/// ring around it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A lighthouse — the always-on mesh anchor / relay (drawn largest, centred).
    Lighthouse,
    /// A headless server tier — an always-on box carrying storage / broker
    /// services but no interactive seat (drawn between the lighthouse and a
    /// workstation, ringed with the peers).
    Server,
    /// A workstation peer (an interactive seat; a headless box is a workstation
    /// without a local display).
    Workstation,
}

/// A node's current health, mapped to the shared status palette
/// (`Style::OK` / `Style::WARN` / `Style::DANGER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Reachable and healthy.
    Ok,
    /// Reachable but degraded (high latency, partial service).
    Warn,
    /// Unreachable / down.
    Down,
}

/// One mesh node.
#[derive(Clone, Debug)]
pub struct MeshNode {
    /// Stable identifier — what [`MeshLink`] endpoints reference.
    pub id: String,
    /// Human label drawn beneath the node (rendered in Intel One Mono).
    pub label: String,
    /// Hierarchy role (size + auto-placement).
    pub role: Role,
    /// Current health (node colour).
    pub health: Health,
    /// The node's running build version (e.g. `"12.0.0"`), drawn as a sub-label
    /// beneath the hostname. `None` ⇒ the source carries no version for this node
    /// and the widget draws an honest `—` placeholder rather than a fabricated
    /// build string.
    pub version: Option<String>,
    /// Whether this node runs an **older** build than the newest on the mesh — a
    /// node the fleet has moved past. Flagged in the widget (WARN-tinted version
    /// line) so an out-of-date peer is distinguishable at a glance.
    pub stale: bool,
    /// Whether this node is the elected leader — gets the pulsing accent ring.
    pub is_leader: bool,
    /// Optional fixed position, **normalized** to `0.0..=1.0` of the canvas.
    /// `None` ⇒ the widget auto-places it (lighthouse-centred radial layout).
    pub pos: Option<Vec2>,
}

impl MeshNode {
    /// A node with the given identity, role and health — not a leader, auto-placed.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        role: Role,
        health: Health,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            role,
            health,
            version: None,
            stale: false,
            is_leader: false,
            pos: None,
        }
    }

    /// Set this node's running build version (builder).
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Mark this node as running an older build than the newest on the mesh
    /// (builder) — the widget flags it so an out-of-date peer stands out.
    #[must_use]
    pub const fn stale(mut self) -> Self {
        self.stale = true;
        self
    }

    /// Mark this node as the elected leader (builder).
    #[must_use]
    pub const fn leader(mut self) -> Self {
        self.is_leader = true;
        self
    }

    /// Pin this node at a fixed **normalized** `0.0..=1.0` position (builder),
    /// opting it out of auto-layout.
    #[must_use]
    pub const fn at(mut self, pos: Vec2) -> Self {
        self.pos = Some(pos);
        self
    }
}

/// An edge between two nodes, carrying a current activity level that drives the
/// travelling pulse animation.
#[derive(Clone, Debug)]
pub struct MeshLink {
    /// `id` of one endpoint node.
    pub a: String,
    /// `id` of the other endpoint node.
    pub b: String,
    /// Activity on this link, `0.0..=1.0`. `0.0` ⇒ idle (no travelling pulse);
    /// higher ⇒ a brighter line and faster/denser pulses.
    pub activity: f32,
}

impl MeshLink {
    /// A link between two node `id`s at the given activity level.
    #[must_use]
    pub fn new(a: impl Into<String>, b: impl Into<String>, activity: f32) -> Self {
        Self {
            a: a.into(),
            b: b.into(),
            activity,
        }
    }
}

/// A snapshot of the whole mesh: the nodes and the links between them. This is
/// the sole input to [`MeshView`](crate::MeshView).
#[derive(Clone, Debug, Default)]
pub struct MeshState {
    /// Every node in the mesh.
    pub nodes: Vec<MeshNode>,
    /// Every link; endpoints reference [`MeshNode::id`]. A link to an unknown
    /// id is silently skipped at render time.
    pub links: Vec<MeshLink>,
}
