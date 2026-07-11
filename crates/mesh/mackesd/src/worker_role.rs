//! E1.2 — role-gated worker subsets.
//!
//! Each `mackesd` worker is tiered to the **minimum deployment role rank** that
//! runs it (`Lighthouse ⊂ Workstation`). `run_serve` resolves the box's rank
//! once via [`resolve_rank`] and gates every `sup.spawn` with [`runs`], so a
//! Lighthouse never starts the fleet/media/voice/desktop workers.
//!
//! **Interpretation (E1.2):** a Lighthouse IS a VPS relay, so it runs Nebula +
//! mde-bus + mesh routing + leader + health. Over-tiering a relay-essential
//! worker would break routing, so the mesh/control plane sits at rank 0; every
//! fleet + voice/media + desktop worker sits at rank 1 (Workstation — a headless
//! box is a Workstation too, its desktop workers idle without a local display).

use mde_role::{Capability, Role, RoleClass};

/// MEDIA-1 — the deployment **class** that gates worker spawns.
///
/// The role rank plus its capability tags. `run_serve` resolves this once and
/// gates every `sup.spawn` through [`runs_in`], so a rank-gated worker checks the
/// tier and a capability-gated worker (the Navidrome media worker — MEDIA-3)
/// additionally requires the matching tag. Keeping rank + tags together is the §9
/// doctrine: a `Lighthouse_Media` box is the Lighthouse tier carrying
/// [`Capability::Media`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeployClass {
    /// The role rank (0 lighthouse · 1 workstation).
    pub rank: u8,
    /// `true` when this box carries [`Capability::Media`] — the `Lighthouse_Media`
    /// subclass that hosts the media service.
    pub media: bool,
}

impl DeployClass {
    /// A plain rank with no capability tags — the back-compat path for the
    /// rank-only callers (`resolve_rank`).
    #[must_use]
    pub const fn plain(rank: u8) -> Self {
        Self { rank, media: false }
    }

    /// Build the class from a pinned [`RoleClass`].
    #[must_use]
    pub const fn from_role_class(class: &RoleClass) -> Self {
        Self {
            rank: class.role.rank(),
            media: class.is_media_lighthouse(),
        }
    }
}

/// Minimum role rank that runs each worker. The census MUST list every worker
/// spawned in `run_serve` (a unit test pins the count) — a worker missing from
/// the table defaults to rank 0 (runs everywhere), a safe default that never
/// silently drops a worker from a role, but the count test catches the omission
/// so the tier is a deliberate decision.
const WORKER_TIERS: &[(&str, u8)] = &[
    // ── Lighthouse (rank 0) — the relay control plane: Nebula, mde-bus,
    //    mesh routing/discovery, leader, health, security baseline.
    ("nebula_supervisor", 0),
    ("heartbeat", 0),
    ("health_reconciler", 0),
    ("mesh_router", 0),
    ("stun_gather", 0),
    ("mdns_relay", 0),
    ("mesh_latency", 0),
    // MESHMAP-6 — per-link byte-counter collector (nftables accounting on
    // the Nebula iface). A control-plane traffic observer that runs on
    // every node, like mesh_latency.
    ("link-traffic", 0),
    ("mesh_dns", 0),
    ("hardware_probe", 0),
    ("bus_supervisor", 0),
    ("firewall_preset", 0),
    ("sshd_overlay_bind", 0),
    ("ssh_pubkey_gossip", 0),
    ("fleet_reconcile", 0),
    ("presence_watch", 0),
    ("etcd_watch", 0),
    ("lifecycle_exec", 0),
    // DEVMGR-8 — the device-control executor: privileged hardware ops
    // (enable/disable, reload module, rescan bus) the Device-Manager surface
    // dispatches to a target node. UNIVERSAL (rank 0) like hardware_probe /
    // lifecycle_exec — every node can be an action target and drains only its
    // own replicated fleet/device-control/<self> request dir.
    ("device_control", 0),
    ("reconcile", 0),
    ("netstate_apply", 0),
    ("validation_suite", 0),
    ("metrics_exporter", 0),
    // BUG-STORAGE-1 — the E12-20 storage worker: a UNIVERSAL per-node topology
    // mirror (read-only UDisks2 enumerate → `state/storage/<node>`). Pinned at
    // rank 0 so it provably publishes on EVERY role — a Workstation has local
    // disks the seated user manages, and a Lighthouse still publishes an honest
    // (often `backend: Unavailable`) mirror. It previously rode the silent
    // "unknown worker ⇒ rank 0" default, which spawned it at runtime but OMITTED
    // it from this census, so `workers_for_rank` / `mackesd role-workers` wrongly
    // reported the Workstation as NOT running storage. Only the READ/publish path
    // is enabled here; the live UDisks2Executor stays IntegrationGated as-is.
    ("storage", 0),
    // QC-2 — the OpenStack supervision worker: UNIVERSAL (rank 0). The
    // QUASAR-CLOUD design's universal-node premise (quasar-cloud.md Q1/Q5/Q22:
    // any-role node, APIs on every node, no controller box) means every node —
    // lighthouse included — can carry cloud duties; the fleet/one-state
    // doctrine (not the role) decides WHICH Kolla services a node hosts, and a
    // node assigned none (or a pre-doctrine mesh, where the read is honestly
    // IntegrationGated) still publishes its `state/openstack/<node>` mirror.
    // A deliberate census entry like storage's (BUG-STORAGE-1), never the
    // silent unknown-worker default.
    ("openstack", 0),
    // EXPLORER-1 — the unit_aggregator worker: the daemon spine of the Hero unit
    // explorer (unit-explorer.md #18). UNIVERSAL (rank 0) like storage/openstack:
    // every node folds its OWN unit view (self-first #23) — the mesh mirror it
    // already reads + the union of every node's openstack mirror + its LAN scan —
    // and publishes `state/units/<node>`. There is no leader/center to elect (lock
    // #20: "no center"); a lighthouse publishes an honest units view too. A
    // deliberate rank-0 entry (the BUG-STORAGE-1 lesson), never the silent
    // unknown-worker default.
    ("unit_aggregator", 0),
    // CHAT-FIX-2 — the local-notification producer worker: watches this node's
    // OWN event sources (mesh peer join/leave, dnf/platform updates, systemctl
    // --failed, df/SMART, journal WARN+) and publishes typed notifications the
    // Chat surface renders as a timestamped feed + tray badge (the real empty-Chat
    // fix — console-frontdoor.md Q34/46/47). UNIVERSAL (rank 0) like the chat
    // worker it feeds: every node — lighthouse included — has local services /
    // disks / a journal / peers to report on, and its notifications ride the same
    // bus the chat worker folds on every role. A deliberate rank-0 census entry
    // (the BUG-STORAGE-1 lesson), never the silent unknown-worker default.
    ("notify", 0),
    // NODE-GRADE-1 (node-grade.md #11) — the per-node self-grade worker. UNIVERSAL
    // (rank 0): every node computes + publishes its OWN A–F capability grade
    // (`<workgroup_root>/node-grade/<hostname>.json`) from the telemetry the
    // platform already gathers, so a lighthouse grades itself too. A deliberate
    // rank-0 census entry (the BUG-STORAGE-1 lesson), never the silent
    // unknown-worker default.
    ("node_grade", 0),
    // KDC-MESH-3 (kdc-mesh.md #15) — the KDE Connect host is UNIVERSAL (rank 0):
    // it runs on EVERY node incl. lighthouses/headless so the mesh-wide "every
    // node recognizes the phone" (#5) + "all nodes serve the phone at once" (#6)
    // goals actually hold. Safe on a headless/relay node because KDC-MESH-1's
    // transport is overlay-ONLY — it binds 1716 on the Nebula overlay IP, never
    // the public NIC, so `kdc_host` on a lighthouse opens NO public port (the
    // firewall preset opens 1716 on the overlay/trusted zone only; public stays
    // default-deny). Was Workstation-only (rank 1) pre-KDC-MESH-3.
    ("kdc_host", 0),
    // CHAT-FIX-1 — the mesh chat worker: folds every node's chat/notification
    // traffic off the bus into the Chat surface's feed. UNIVERSAL (rank 0): it
    // ALREADY ran on every node — a lighthouse included — via the silent
    // "unknown worker ⇒ rank 0" default (live-verified on Eagle: boot log
    // `starting worker: chat`), but that default OMITTED it from this census, so
    // `mackesd role-workers` dishonestly failed to list a worker every node runs.
    // A deliberate rank-0 census entry now (the BUG-STORAGE-1 lesson) — same rank
    // it always had, now EXPLICIT + counted. Pairs with `notify` (CHAT-FIX-2), the
    // producer whose events it folds.
    ("chat", 0),
    // ── ARCH-5 (drift guard) — universal (rank-0) workers that were spawned in
    //    `run_serve` gated on `worker_role::runs(...)` but OMITTED from this census,
    //    so they silently rode the "unknown worker ⇒ rank 0" default: they DID run
    //    everywhere (correct) but `mackesd role-workers` never listed them — the exact
    //    BUG-STORAGE-1 omission, repeated. The new
    //    `worker_spawns_and_the_census_do_not_drift` reconcile test now REFUSES that
    //    silent default: every `runs(...)`-gated worker must be a deliberate census
    //    entry. Pinned at rank 0 = the rank they already resolved to via the default,
    //    so runtime behavior is UNCHANGED; they are now EXPLICIT + listed. Each spawn
    //    site documents its own "rank-0 / runs-everywhere / universal" intent
    //    (self-marker-gated where relevant).
    ("boot_readiness", 0), // BOOT-STATUS-1 — fabric bring-up snapshot, all roles
    ("xcp_host", 0),       // XCP-6 — hypervisor-capacity advertiser, self-gates on the dom0 marker
    ("kvm_health", 0),     // MV-2 — per-node KVM service health, universal virt stack
    ("vm_lifecycle", 0),   // MV — per-node libvirt VM executor, every node hosts VMs
    ("container", 0),      // MV — per-node Podman container executor, every node hosts containers
    ("scheduler", 0),      // MV-5 — placement scheduler (single-actor election), runs everywhere
    ("session_broker", 0), // VDI — session-roster broker, leader-gated internally, runs everywhere
    ("session_roaming", 0), // VDI — roaming-session reconciler, runs everywhere
    ("console_broker", 0), // VDI — live-console overlay relay, serving-peer-gated, runs everywhere
    ("clipboard_bridge", 0), // VDI — per-session clipboard relay, node-local, runs everywhere
    ("service_onboard", 0), // onboard — action/onboard/service-add engine, leader-gated, runs everywhere
    ("spawn_lighthouse_onboard", 0), // onboard — action/onboard/spawn-lighthouse engine, leader-gated
    ("onboard_apply", 0),            // onboard — addressed remote-bundle applier, runs everywhere
    ("lighthouse_probe", 0), // LIGHTHOUSE-8 — per-lighthouse deep-probe lane (gated in workers/mod.rs), rank-0
    // ── Workstation (rank 1) — everything beyond the relay control plane: the
    //    fleet + mesh storage workers AND voice / clipboard / kdc / remmina /
    //    music. A headless box is a Workstation too (the desktop workers idle
    //    gracefully without a local display).
    ("ansible-pull", 1),
    ("app-sync", 1),
    ("job_exec", 1),
    ("voice_config", 1),
    ("clipboard_sync", 1),
    // BOOKMARKS-2 — the mesh-synced bookmarks worker. A desktop feature (the
    // seated user edits the Bookmarks surface), so Workstation-tier; it idles
    // gracefully on a headless box (no action/bookmarks/* requests) while still
    // replaying peers' Syncthing segments into the shared collection.
    ("bookmarks", 1),
    // BOOKMARKS-7 — the mesh-wide ad-blocker worker. A desktop feature (it feeds
    // the mde-web-preview browser's block engine), so Workstation-tier; it idles
    // gracefully on a headless box (no browser, no action/adfilter/* requests)
    // while still replicating peers' filter-store blobs over Syncthing and, when
    // leader, compiling the shared engine blob.
    ("adfilter", 1),
    // BOOKMARKS-8 — the mesh-wide browser/ad-blocker POLICY worker. Reads the
    // Syncthing-synced fleet policy doc, folds it for THIS node's role, and
    // enforces at the browser launch/spawn seam (refuse-to-spawn on a disallowed
    // role, inject the forced ad-blocker + URL allowlist + custom lists, reject
    // out-of-policy navigate / adblock-off actions). A desktop-governance feature,
    // so Workstation-tier; it idles gracefully on a headless box (no browser, no
    // action/browser/* requests) while still replicating the policy doc.
    ("browser_policy", 1),
    // BROWSER-DD-6 — Browser passkey/WebAuthn ceremony owner. Drains strict
    // Browser-origin passkey ceremony metadata, persists pending challenges into
    // local + Syncthing-backed roots, and publishes honest pending/error state.
    // A desktop/browser security feature, so Workstation-tier; it idles
    // gracefully on headless boxes with no Browser publishes.
    ("browser_passkeys", 1),
    // BROWSER-DD-7 — Browser session-sync owner. Drains the Browser's
    // action/browser/session-sync snapshots into a local durable latest snapshot
    // and mirrors them to the Syncthing share for follow-me/startup restore. A
    // desktop/browser feature, so Workstation-tier; idles gracefully on a headless
    // box with no Browser publishes.
    ("browser_session_sync", 1),
    // BROWSER-DD-11 — Browser read-aloud/TTS owner. Drains Browser page-text
    // read-aloud requests and speaks them through the configured offline TTS
    // command when present, publishing honest unavailable/error state otherwise.
    // A desktop/browser accessibility feature, so Workstation-tier.
    ("browser_read_aloud", 1),
    // BROWSER-DD-11 — Browser voice-command/dictation STT owner. Drains Browser
    // command/dictation requests and transcribes them through the configured
    // offline STT/capture command when present, publishing honest unavailable or
    // error state otherwise. A desktop/browser accessibility feature.
    ("browser_voice_command", 1),
    // BROWSER-DD-12 — Browser external-protocol owner. Drains Browser
    // action/browser/protocol handoffs for schemes the shell refuses to navigate,
    // publishing retained route status/events for downstream Email/Transfers
    // owners without faking those surfaces. A desktop/browser integration
    // feature, so Workstation-tier.
    ("browser_protocol", 1),
    // BROWSER-DD-12 — Browser platform-share owner. Drains Browser
    // action/browser/share handoffs for Peer/Email/QR targets, publishing
    // retained route status/events without faking downstream delivery. A
    // desktop/browser integration feature, so Workstation-tier.
    ("browser_share", 1),
    // BROWSER-DD-12 — Browser private offline/mesh translation owner. Drains
    // Browser page-text translation requests and translates them through a
    // configured local/mesh command when present, publishing honest unavailable
    // or error state otherwise. A desktop/browser integration feature.
    ("browser_translate", 1),
    // BROWSER-DD-12 — Browser offline/mesh cache owner. Drains explicit Browser
    // page snapshots, keeps the helper no-store policy intact, and mirrors the
    // bounded private cache records onto the Syncthing-backed file plane.
    ("browser_offline_cache", 1),
    // BROWSER-DD-12 — Browser CEF security-update status owner. Watches the
    // packaged fast-update manifest and active CEF runtime, publishing an honest
    // current/missing/mismatch posture for the independent browser engine update
    // path. A desktop/browser integration feature, so Workstation-tier.
    ("browser_security_update", 1),
    // BROWSER-DD-12 — Browser idle-tab suspend owner. Drains shell-published
    // action/browser/tab-suspend handoffs after the shell stops inactive helper
    // load paths, publishing retained suspend status/events for diagnostics and
    // future orchestration. A desktop/browser integration feature, so
    // Workstation-tier.
    ("browser_tab_suspend", 1),
    // KDC-MESH-6 — phone-as-touchpad/keyboard seat consumer. Drains KDC
    // worker's action/seat/remote-input handoffs and invokes the configured
    // local uinput/seat helper when present. Workstation-tier; idles on
    // headless nodes.
    ("seat_remote_input", 1),
    // FILEMGR-5 — the Files-surface sshfs mesh-mount worker. A desktop feature
    // (the seated user browses peers), so Workstation-tier; it idles gracefully
    // with no mount requests on a headless box.
    ("mesh_mount", 1),
    // CHOOSER-1 — the desktop-source discovery aggregator behind the Chooser
    // surface. A desktop feature (the seated user picks a desktop to connect
    // to), so Workstation-tier; it idles gracefully on a headless box (the
    // aggregation is cheap and the verbs simply never arrive).
    ("desktop_sources", 1),
    ("remmina-sync", 1),
    // MEDIA-8 — Workstation music auto-config: a desktop worker (no seated user
    // on a Lighthouse, so Workstation-tier), reads the published shared
    // account off the registry plane + writes the desktop user's creds.
    ("music_autoconfig", 1),
    // MEDIA-14 — the mesh media-source discovery aggregator behind the
    // mde-media Sources panel. A desktop feature (the seated user picks a media
    // source to play), so Workstation-tier; it idles gracefully on a headless
    // box (the aggregation is cheap and simply publishes an empty roster).
    ("media_sources", 1),
    // MEDIA-15 — the mesh media server + DLNA/UPnP + aggregation (the PRODUCER
    // half MEDIA-14 discovers). A desktop feature (the seated user shares their
    // media folders), so Workstation-tier; it idles gracefully on a headless
    // box (empty share manifest, empty aggregated library).
    ("media_server", 1),
    // TERM-7 — the mesh PTY-broker: opens remote shells on peers over the
    // overlay for the mde-term-egui terminal surface. A desktop feature (the
    // seated user opens a terminal on a mesh node), so Workstation-tier; it
    // idles gracefully on a headless box (no action/pty/* requests arrive).
    ("pty_broker", 1),
    // TRANSFERS-1 — the transfers worker: the daemon-owned queue/ledger/verb spine
    // of the Transfers surface (docs/design/transfers-surface.md). A desktop feature
    // fronted by the File Browser (Q1), the sibling of pty_broker/mesh_mount, so
    // Workstation-tier; it idles gracefully on a headless box or a Lighthouse relay
    // (an empty inbox + empty ledger, no transfer.submit verbs arrive). A deliberate
    // census entry (the BUG-STORAGE-1 lesson — a worker absent from the census
    // silently never runs).
    ("transfers", 1),
];

/// MEDIA-1 — workers that ALSO require a capability tag beyond their rank tier.
///
/// A capability-gated worker runs only on a box that is at (or above) its rank
/// AND carries the tag — so the Navidrome media worker (MEDIA-3) runs on a
/// `Lighthouse_Media` node but never on a stock lighthouse / server / peer
/// (acceptance: "container absent on a non-media node"). The worker is still
/// listed in [`WORKER_TIERS`] for the rank floor; this table adds the tag gate.
///
/// `navidrome` is the foundation entry MEDIA-3 spawns onto: a rank-0 (lighthouse
/// tier) worker that additionally requires [`Capability::Media`]. It is wired
/// here now (not at MEDIA-3) so the gate is a single source of truth the worker
/// pool reads — MEDIA-3 adds the spawn, the tier table already refuses it
/// everywhere but a media-lighthouse.
const WORKER_CAPABILITIES: &[(&str, Capability)] = &[("navidrome", Capability::Media)];

/// Lighthouse tier (rank 0) — the rank floor the media worker sits at. The
/// `navidrome` worker is a lighthouse-tier worker that additionally requires the
/// [`Capability::Media`] tag (it never runs on a stock lighthouse).
const MEDIA_WORKER_RANK: u8 = 0;

/// Minimum rank that runs `worker`. Unknown workers default to 0 (Lighthouse).
///
/// NOTE this is the rank floor ONLY — a capability-gated worker (see
/// [`WORKER_CAPABILITIES`]) ALSO needs its tag; use [`runs_in`] for the full gate.
#[must_use]
pub fn min_rank(worker: &str) -> u8 {
    if let Some(rank) = capability_min_rank(worker) {
        return rank;
    }
    WORKER_TIERS
        .iter()
        .find(|(n, _)| *n == worker)
        .map_or(0, |(_, r)| *r)
}

/// The rank floor for a capability-gated worker that isn't in [`WORKER_TIERS`]
/// (the media worker lives in the capability table, not the rank census, so its
/// rank floor is pinned here). `None` for a plain rank-gated worker.
fn capability_min_rank(worker: &str) -> Option<u8> {
    WORKER_CAPABILITIES
        .iter()
        .find(|(n, _)| *n == worker)
        .map(|(_, cap)| match cap {
            Capability::Media => MEDIA_WORKER_RANK,
        })
}

/// MEDIA-1 — the capability tag `worker` requires (beyond its rank), if any.
#[must_use]
pub fn required_capability(worker: &str) -> Option<Capability> {
    WORKER_CAPABILITIES
        .iter()
        .find(|(n, _)| *n == worker)
        .map(|(_, c)| *c)
}

/// BOOKMARKS-8 — the canonical role name for a resolved `rank`.
///
/// The browser-policy worker folds its per-role fleet policy by this name. An
/// unknown rank falls back to the top tier, matching the tolerant [`resolve_rank`]
/// posture.
#[must_use]
pub fn role_name(rank: u8) -> &'static str {
    Role::all()
        .into_iter()
        .find(|r| r.rank() == rank)
        .unwrap_or(Role::Workstation)
        .as_str()
}

/// Resolve the deployment rank that gates worker spawns: the pinned role's
/// rank, or **Workstation (1) when unpinned** (a dev tree / pre-role-pin box
/// runs the full set — the desktop workers idle gracefully without a Wayland
/// session), or **Lighthouse (0) when `role.toml` is malformed** (fail closed —
/// run only the relay control plane, never assume a Workstation default).
/// Reads `/var/lib/mde/role.toml` locally; no mesh needed.
#[must_use]
pub fn resolve_rank() -> u8 {
    resolve_class().rank
}

/// MEDIA-1 — resolve the full deployment **class** (rank + capability tags) that
/// gates worker spawns.
///
/// Same fail-soft contract as [`resolve_rank`]: an unpinned box → Workstation (no
/// media tag — the desktop set, never the media worker), a malformed `role.toml`
/// → Lighthouse fail-closed (no media tag). The media tag is only ever set when a
/// valid `Lighthouse_Media` class is pinned.
#[must_use]
pub fn resolve_class() -> DeployClass {
    match mde_role::load_class() {
        Ok(class) => DeployClass::from_role_class(&class),
        Err(mde_role::LoadError::NotPinned) => DeployClass::plain(Role::Workstation.rank()),
        Err(_) => DeployClass::plain(Role::Lighthouse.rank()),
    }
}

/// ENT-2 (C3) — the FAIL-CLOSED resolver the worker pool boots
/// through: an unpinned box refuses to start instead of silently
/// running the fattest (Workstation) worker set. Display/diagnostic
/// paths keep the tolerant [`resolve_rank`]; the supervisor uses this.
///
/// # Errors
/// A human-actionable message naming the fix (`mackesd role pin …`).
pub fn resolve_rank_strict() -> Result<u8, String> {
    resolve_class_strict().map(|c| c.rank)
}

/// MEDIA-1 — the fail-closed counterpart to [`resolve_class`] (ENT-2).
///
/// The same refuse-when-unpinned contract as [`resolve_rank_strict`], returning
/// the full [`DeployClass`] so the worker pool gates capability workers (the
/// media worker) as well as rank workers off a single resolved class.
///
/// # Errors
/// A human-actionable message naming the fix (`mackesd role pin …`).
pub fn resolve_class_strict() -> Result<DeployClass, String> {
    match mde_role::load_class() {
        Ok(class) => Ok(DeployClass::from_role_class(&class)),
        Err(mde_role::LoadError::NotPinned) => Err(
            "no deployment role pinned (/var/lib/mde/role.toml absent) — this box refuses to \
             start its worker pool unpinned (ENT-2 fail-closed). Pin one first: \
             `mackesd role pin <lighthouse|workstation>`"
                .to_string(),
        ),
        Err(e) => Err(format!(
            "role.toml unreadable ({e}) — refusing to start the worker pool (ENT-2). \
             Repair or re-pin: `mackesd role pin <role>`"
        )),
    }
}

/// Whether a box at `role_rank` runs `worker` — the **rank-only** gate.
///
/// A capability-gated worker (the media worker) is NOT runnable through this path
/// (it needs its tag too); [`runs`] returns `false` for one, and the full gate
/// lives in [`runs_in`]. Plain rank-gated workers are unaffected.
#[must_use]
pub fn runs(worker: &str, role_rank: u8) -> bool {
    runs_in(worker, DeployClass::plain(role_rank))
}

/// MEDIA-1 — the full spawn gate: whether a box of `class` runs `worker`.
///
/// A worker runs iff the box is at (or above) the worker's rank floor AND — for a
/// capability-gated worker — the box carries the required tag. This is the single
/// predicate `run_serve` gates every `sup.spawn` through, so the media worker
/// lands on a `Lighthouse_Media` node and is absent everywhere else.
#[must_use]
pub fn runs_in(worker: &str, class: DeployClass) -> bool {
    if class.rank < min_rank(worker) {
        return false;
    }
    match required_capability(worker) {
        None => true,
        Some(Capability::Media) => class.media,
    }
}

/// Every worker a box at `role_rank` runs — the rank-gated subset (plan §12).
///
/// Capability-gated workers (the media worker) are EXCLUDED here (a rank alone
/// can't satisfy a tag gate); use [`workers_for_class`] for the full set on a
/// tagged box. Order follows the tier census (lowest tier first). This is the
/// static counterpart to `run_serve`'s live `worker_names` listing, surfaced by
/// `mackesd role-workers`.
#[must_use]
pub fn workers_for_rank(role_rank: u8) -> Vec<&'static str> {
    workers_for_class(DeployClass::plain(role_rank))
}

/// MEDIA-1 — every worker a box of `class` runs, including capability-gated ones.
///
/// The capability workers its tags unlock (a `Lighthouse_Media` class adds the
/// media worker on top of its lighthouse rank set). Rank workers first (tier
/// census order), then the capability workers the box's tags satisfy.
#[must_use]
pub fn workers_for_class(class: DeployClass) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = WORKER_TIERS
        .iter()
        .filter(|(_, tier)| class.rank >= *tier)
        .map(|(name, _)| *name)
        .collect();
    out.extend(
        WORKER_CAPABILITIES
            .iter()
            .filter(|(name, _)| runs_in(name, class))
            .map(|(name, _)| *name),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // ARCH-5 — the drift guard between the two worker registries.
    //
    // Worker registration is split across TWO registries that have drifted before
    // (BUG-STORAGE-1): this static role census (`WORKER_TIERS` + `WORKER_CAPABILITIES`)
    // vs. the ~136 imperative `sup.spawn(...)` / `worker_names.push(...)` sites in
    // `run_serve` (`bin/mackesd.rs`) plus the role gates (`worker_role::runs(...)`)
    // scattered across the crate. Nothing at runtime enforces that the two agree, so a
    // worker could be:
    //   • spawned-but-uncensused → its `runs(...)` gate silently resolves to
    //     `min_rank => 0` (runs on every role, possibly the wrong tier) AND it is
    //     hidden from `mackesd role-workers` (the exact BUG-STORAGE-1 failure), or
    //   • censused-but-never-spawned → a phantom row that makes `role-workers` lie.
    //
    // `worker_spawns_and_the_census_do_not_drift` READS the crate source at test time
    // and reconciles the registries so any FUTURE drift fails the build with the
    // offending worker named. Airgapped-safe: pure source parsing, no live env.
    // (This does NOT unify the two registries — that is a larger refactor; it only
    // makes their divergence detectable and forces every spawn to be a deliberate,
    // classified decision.)

    /// Workers spawned in `run_serve` that are deliberately NOT in the role-tier
    /// census: they spawn UNCONDITIONALLY on every role (mesh/nebula control plane,
    /// bus responders, datacenter/compute workers that self-gate on a runtime marker),
    /// or are capability-gated under the `navidrome` key (`media_registry` /
    /// `navidrome_supervisor`). None of them consult `WORKER_TIERS`, so they cannot
    /// mis-tier — but listing them here keeps the full-roster reconcile honest: every
    /// spawned worker must be classified as EITHER a deliberate tier entry OR an
    /// explicit not-tier-gated entry, so a NEW spawn that is neither fails the guard.
    /// A future tiering pass may promote an entry from here into `WORKER_TIERS`.
    const NON_TIERED_WORKERS: &[&str] = &[
        "action",
        "alert_relay",
        "apps_bus_responder",
        "apps_installed",
        "apps_running",
        "bus_retention_gc",
        "cert_authority",
        "clipboard_bus_responder",
        "compute_event_toast",
        "compute_expose",
        "compute_migrate",
        "compute_provision",
        "compute_registry",
        "connect_bus_responder",
        "connect_firewall",
        "copilot",
        "cups_sync",
        "datacenter_orchestrator",
        "dc_auditor",
        "dc_bus_responder",
        "dc_health",
        "dc_jobs",
        "dc_power_bus_responder",
        "dc_promote",
        "dc_snap_scheduler",
        "ddns_bus_responder",
        "ddns_reconcile",
        "directory_bus_responder",
        "dr_scheduler",
        "farm_orchestrator",
        "files_bus_responder",
        "firewall_monitor",
        "fleet_bus_responder",
        "host_ops_bus_responder",
        "host_state",
        "jobs_bus_responder",
        "leader_election",
        "media_registry",
        "mesh_firewall",
        "mirror_syncd",
        "navidrome_supervisor",
        "nebula_bus_responder",
        "nebula_ca_backup",
        "nebula_csr_watcher",
        "nebula_enroll_listener",
        "nebula_https_listener",
        "netassess",
        "netdata_aggregator",
        "peer-cap",
        "probe",
        "route_bus_responder",
        "router_registry",
        "selinux_monitor",
        "settings_bus_responder",
        "shell_bus_responder",
        "surface_enable",
        "surface_firmware",
        "surface_verify",
        "surrounding_hosts",
        "tofu_bus_responder",
        "upgrade_intent_watcher",
        "voice_provision",
        "voip_bus_responder",
        "voip_rtt",
        "vpn_bus_responder",
        "xcp_provision",
    ];

    /// Worker(s) whose `worker_names.push(...)` uses a runtime-computed name rather
    /// than a string literal, so the source scan cannot see them. Currently only the
    /// LIGHTHOUSE-8 probe, spawned via `Supervisor::spawn_lighthouse_probe()` which
    /// returns the worker's own `name()` (`"lighthouse_probe"`). Listed so the phantom
    /// guard does not false-flag its (deliberate) census entry.
    const DYNAMIC_SPAWNS: &[&str] = &["lighthouse_probe"];

    fn crate_src_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn read_source(rel: &str) -> String {
        let p = crate_src_dir().join(rel);
        std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("ARCH-5 drift guard: cannot read {}: {e}", p.display()))
    }

    /// Every `*.rs` under the crate `src/` tree.
    fn rust_sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let rd = std::fs::read_dir(dir).unwrap_or_else(|e| {
            panic!("ARCH-5 drift guard: cannot read dir {}: {e}", dir.display())
        });
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(rust_sources(&p));
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(p);
            }
        }
        out
    }

    /// Drop whole-line comments so doc/inline mentions of `runs(...)` don't register
    /// as gate sites (e.g. the `//!` module docs in `media_registry.rs`).
    fn strip_line_comments(src: &str) -> String {
        src.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Extract each worker-name literal that immediately follows `needle`
    /// (e.g. `.push("` or `runs("`), reading up to the closing quote. Only lowercase
    /// worker tokens (`[a-z0-9_-]`) are accepted, so multi-word / non-worker strings
    /// are ignored.
    fn scan_names(src: &str, needle: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = src.as_bytes();
        let mut i = 0usize;
        while i < src.len() {
            let Some(pos) = src[i..].find(needle) else {
                break;
            };
            let start = i + pos + needle.len();
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            let tok = &src[start..j];
            if !tok.is_empty()
                && tok
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            {
                out.push(tok.to_string());
            }
            i = j + 1;
        }
        out
    }

    /// Every worker name passed to a PRODUCTION `worker_role::runs(...)` /
    /// `runs_in(...)` gate anywhere in the crate. Skips this module (its `runs(...)`
    /// calls are test fixtures like `"some-future-worker"`) and comment lines.
    fn collect_gate_names() -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        let mut n_files = 0usize;
        for path in rust_sources(&crate_src_dir()) {
            if path.file_name().and_then(|s| s.to_str()) == Some("worker_role.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read rs source");
            let code = strip_line_comments(&src);
            for name in scan_names(&code, "runs(\"") {
                set.insert(name);
            }
            for name in scan_names(&code, "runs_in(\"") {
                set.insert(name);
            }
            n_files += 1;
        }
        assert!(
            n_files >= 3,
            "ARCH-5 drift guard: only scanned {n_files} source files — the walker is broken"
        );
        set
    }

    #[test]
    fn worker_spawns_and_the_census_do_not_drift() {
        use std::collections::BTreeSet;

        let census: BTreeSet<&str> = WORKER_TIERS.iter().map(|(n, _)| *n).collect();
        let caps: BTreeSet<&str> = WORKER_CAPABILITIES.iter().map(|(n, _)| *n).collect();
        let non_tiered: BTreeSet<&str> = NON_TIERED_WORKERS.iter().copied().collect();
        let dynamic: BTreeSet<&str> = DYNAMIC_SPAWNS.iter().copied().collect();

        // The definitive spawn roster: every `worker_names.push("X")` literal in
        // run_serve (`bin/mackesd.rs`), plus the runtime-named dynamic spawns.
        let bin = read_source("bin/mackesd.rs");
        let pushed: BTreeSet<String> = scan_names(&bin, ".push(\"").into_iter().collect();
        assert!(
            pushed.len() >= 120,
            "ARCH-5 drift guard: only {} `.push(\"…\")` worker sites found — the source scan \
             is broken (expected ~136)",
            pushed.len()
        );

        // Every role gate (`runs`/`runs_in`) across the crate.
        let gated = collect_gate_names();
        assert!(
            gated.len() >= 40,
            "ARCH-5 drift guard: only {} `runs(…)` gate sites found — the source scan is \
             broken (expected ~60)",
            gated.len()
        );

        // (1) Silent-default guard — the BUG-STORAGE-1 root cause. Every worker GATED
        //     on the role census must actually BE in the census (or capabilities). A
        //     gate on an uncensused name silently resolves `min_rank => 0`, so the
        //     worker runs on every role AND is hidden from `mackesd role-workers`.
        let mut gated_uncensused: Vec<&str> = gated
            .iter()
            .map(String::as_str)
            .filter(|n| !census.contains(n) && !caps.contains(n))
            .collect();
        gated_uncensused.sort_unstable();
        assert!(
            gated_uncensused.is_empty(),
            "ARCH-5 DRIFT: these workers are gated on `worker_role::runs(…)` but are MISSING \
             from WORKER_TIERS/WORKER_CAPABILITIES, so they silently default to rank 0 and \
             never appear in `mackesd role-workers` (the BUG-STORAGE-1 bug). Add each to the \
             census with a deliberate tier: {gated_uncensused:?}"
        );

        // (2) Phantom guard — every census entry must actually be spawned in run_serve
        //     (a literal push, or a known runtime-named dynamic spawn). Catches a
        //     census row whose spawn was renamed/deleted (a lie in `role-workers`).
        let mut phantom: Vec<&str> = census
            .iter()
            .copied()
            .filter(|n| !pushed.contains(*n) && !dynamic.contains(n))
            .collect();
        phantom.sort_unstable();
        assert!(
            phantom.is_empty(),
            "ARCH-5 DRIFT: these WORKER_TIERS entries are never spawned in run_serve \
             (renamed/deleted spawn — remove the census row, or add to DYNAMIC_SPAWNS if it \
             is spawned under a runtime-computed name): {phantom:?}"
        );

        // (3) Full-roster accountability — every spawned worker must be classified:
        //     a deliberate tier entry (WORKER_TIERS), a capability worker, or an
        //     explicit not-tier-gated entry (NON_TIERED_WORKERS). A brand-new spawn
        //     that is none of these fails here, forcing a deliberate decision.
        let mut unaccounted: Vec<&str> = pushed
            .iter()
            .map(String::as_str)
            .filter(|n| !census.contains(n) && !caps.contains(n) && !non_tiered.contains(n))
            .collect();
        unaccounted.sort_unstable();
        assert!(
            unaccounted.is_empty(),
            "ARCH-5 DRIFT: these workers are spawned in run_serve but classified NOWHERE. Add \
             each to WORKER_TIERS (if role-tiered) or NON_TIERED_WORKERS (if it spawns \
             unconditionally on every role): {unaccounted:?}"
        );

        // (4) Allowlist hygiene — no stale NON_TIERED_WORKERS entry (each must still be
        //     spawned), and it stays disjoint from the tier census.
        let mut stale: Vec<&str> = non_tiered
            .iter()
            .copied()
            .filter(|n| !pushed.contains(*n))
            .collect();
        stale.sort_unstable();
        assert!(
            stale.is_empty(),
            "ARCH-5: these NON_TIERED_WORKERS entries are no longer spawned in run_serve — \
             remove them: {stale:?}"
        );
        let mut both: Vec<&str> = non_tiered
            .iter()
            .copied()
            .filter(|n| census.contains(n))
            .collect();
        both.sort_unstable();
        assert!(
            both.is_empty(),
            "ARCH-5: these workers are in BOTH WORKER_TIERS and NON_TIERED_WORKERS — pick one: \
             {both:?}"
        );
    }

    #[test]
    fn the_table_is_the_full_17_worker_census() {
        // Guards against a worker added to run_serve without a deliberate tier
        // (it would silently default to Lighthouse). 31 originally; -1 redundant
        // python `clipboard` (RETIRE-PY.3), -1 broken python
        // `mdns` relay (RETIRE-PY.1), +1 native `mdns_relay` (MESH-MDNS-RELAY,
        // the real Rust cross-segment relay), -1 dead python `fs_sync` GVFS
        // worker (RETIRE-PY.4, mesh storage is Syncthing under SUBSTRATE-V2).
        // -12 sway/desktop workers (E11 'Cosmic owns the desktop' — the
        // labwc/sway worker stack deleted). +1 ssh_pubkey_gossip (SVC-2),
        // +1 fleet_reconcile (PD-9), +1 presence_watch (PD-13),
        // +1 lifecycle_exec (PD-11), +1 job_exec (PLANES-9),
        // +1 mesh_dns (PLANES-18), +1 netstate_apply (PLANES-15),
        // +1 validation_suite (PLANES-19), +1 metrics_exporter (EFF-9),
        // +1 hardware_probe (SUBAUDIT-D2 — the PeerProbe producer).
        // +1 clipboard_sync (CLIP-SYNC-1 — the mesh clipboard watcher; it is the
        // SOLE clipboard capturer, spawning `wl-paste --watch` directly. The
        // never-built `mde-clipd` daemon + its `clipd_supervisor` worker were
        // removed in CLIP-SYNC-2: that binary never existed in the workspace).
        // +1 etcd_watch (SUBSTRATE-10 — the coordination-plane WATCH worker that
        // pushes instant peer-down / leader-change alerts off etcd watch streams).
        // +1 music_autoconfig (MEDIA-8 — Workstation music birthright: writes the
        // desktop user's airsonic-creds.json from the published mesh shared account).
        // +1 link-traffic (MESHMAP-6 — per-link byte-counter collector, rank 0).
        // +1 mesh_mount (FILEMGR-5 — the Files-surface sshfs mesh-mount worker,
        // Workstation-tier: a seated-user desktop feature).
        // +1 bookmarks (BOOKMARKS-2 — the mesh-synced bookmarks worker,
        // Workstation-tier: a seated-user desktop feature).
        // +1 desktop_sources (CHOOSER-1 — the desktop-source discovery
        // aggregator, Workstation-tier: a seated-user desktop feature).
        // +1 media_sources (MEDIA-14 — the mesh media-source discovery
        // aggregator, Workstation-tier: a seated-user desktop feature).
        // +1 media_server (MEDIA-15 — the mesh media server + DLNA + aggregation,
        // the PRODUCER half; Workstation-tier: a seated-user desktop feature).
        // +1 pty_broker (TERM-7 — the mesh PTY-broker opening remote shells over
        // the overlay, Workstation-tier: a seated-user desktop feature).
        // +1 adfilter (BOOKMARKS-7 — the mesh-wide ad-blocker worker replicating the
        // filter-store blob + leader-compiling the engine, Workstation-tier: a
        // seated-user desktop feature backing the mde-web-preview browser).
        // +1 browser_policy (BOOKMARKS-8 — the mesh-wide browser/ad-blocker POLICY
        // worker: reads the synced fleet policy doc + enforces at the browser
        // launch seam, Workstation-tier: a seated-user desktop-governance feature).
        // +1 browser_session_sync (BROWSER-DD-7 — Browser follow-me/startup-restore
        // session snapshots mirrored onto the Syncthing file plane,
        // Workstation-tier browser feature).
        // +1 browser_read_aloud (BROWSER-DD-11 — Browser read-aloud/TTS owner,
        // Workstation-tier accessibility feature).
        // +1 storage (BUG-STORAGE-1 — the E12-20 universal per-node topology mirror,
        // pinned at rank 0 so it is a deliberate census entry on every role instead
        // of riding the silent unknown-worker default that hid it from role-workers).
        // +1 openstack (QC-2 — the QUASAR-CLOUD Kolla-service supervision worker,
        // pinned at rank 0: the universal-node premise (quasar-cloud.md Q1/Q5/Q22)
        // puts cloud duties on any role; the fleet doctrine, not the rank, decides
        // which services a node hosts).
        // +1 unit_aggregator (EXPLORER-1 — the Hero unit-explorer daemon spine,
        // pinned at rank 0: every node folds + publishes its OWN unit view
        // (state/units/<node>), no center; the BUG-STORAGE-1 deliberate-entry lesson).
        // +1 notify (CHAT-FIX-2 — the local-notification producer, pinned at rank 0:
        // every node reports its own peer/service/disk/journal events into the Chat
        // feed the chat worker folds; the real empty-Chat fix).
        // KDC-MESH-3 (#15) — kdc_host MOVED from rank 1 → rank 0 (universal KDE
        // Connect host: every node recognizes the phone, overlay-only so no public
        // port opens). A tier move, not an add, so the total is unchanged; the
        // rank split shifts 26/16 → 27/15 (see `tier_counts_match_the_two_role_split`).
        // +1 chat (CHAT-FIX-1 — the universal mesh chat worker, pinned at rank 0:
        // it already ran on every node via the silent unknown-worker default; now
        // it is an EXPLICIT census entry so `mackesd role-workers` lists it. The
        // rank split shifts 27/15 → 28/15, len 42 → 43).
        // +1 node_grade (NODE-GRADE-1 — the universal per-node self-grade worker,
        // pinned at rank 0: every node computes + publishes its own A–F capability
        // grade. The rank split shifts 28/15 → 29/15, len 43 → 44).
        // +1 device_control (DEVMGR-8 — the universal per-node device-control
        // executor, pinned at rank 0: every node can be a device-action target and
        // drains its own fleet/device-control/<self> dir. Split 29/15 → 30/15, len
        // 44 → 45).
        // +1 transfers (TRANSFERS-1 — the Workstation-tier transfers queue/ledger/
        // verb spine, sibling of pty_broker/mesh_mount. Split 30/15 → 30/16, len
        // 45 → 46). +1 browser_session_sync shifts split 30/16 → 30/17, len 46 → 47.
        // +1 browser_read_aloud shifts split 30/17 → 30/18, len 47 → 48.
        // +1 browser_voice_command shifts split 30/18 → 30/19, len 48 → 49.
        // +1 browser_translate shifts split 30/19 → 30/20, len 49 → 50.
        // +1 browser_offline_cache shifts split 30/20 → 30/21, len 50 → 51.
        // +1 browser_security_update shifts split 30/21 → 30/22, len 51 → 52.
        // +1 browser_tab_suspend shifts split 30/22 → 30/23, len 52 → 53.
        // +1 browser_protocol shifts split 30/23 → 30/24, len 53 → 54.
        // +1 browser_share shifts split 30/24 → 30/25, len 54 → 55.
        // +1 seat_remote_input shifts split 30/25 → 30/26, len 55 → 56.
        // +1 browser_passkeys shifts split 30/26 → 30/27, len 56 → 57.
        // ARCH-5 (drift guard) +14 universal rank-0 workers that were riding the
        // silent "unknown worker ⇒ rank 0" default (spawned + `runs(...)`-gated but
        // uncensused → hidden from `mackesd role-workers`, the BUG-STORAGE-1 class):
        // boot_readiness, xcp_host, kvm_health, vm_lifecycle, container, scheduler,
        // session_broker, session_roaming, console_broker, clipboard_bridge,
        // service_onboard, spawn_lighthouse_onboard, onboard_apply, lighthouse_probe.
        // All rank 0 (behavior-preserving), so the split shifts 30/27 → 44/27,
        // len 57 → 71. The `worker_spawns_and_the_census_do_not_drift` test now keeps
        // the census + the run_serve spawn sites from silently diverging again.
        assert_eq!(WORKER_TIERS.len(), 71);
    }

    #[test]
    fn strict_resolver_error_names_the_fix() {
        // ENT-2 — we can't unpin the dev box's real role.toml from a
        // test, but the error contract is pure: both failure arms
        // must name `mackesd role pin`. Pin the strings.
        // (The fail-closed behavior itself is smoked in CI via
        // `mackesd serve` on a roleless container — OBS-2 scope.)
        let unpinned_msg =
            match mde_role::load_from(std::path::Path::new("/nonexistent/ent2/role.toml")) {
                Err(mde_role::LoadError::NotPinned) => true,
                _ => false,
            };
        assert!(unpinned_msg, "absent file reads NotPinned");
    }

    #[test]
    fn tier_counts_match_the_two_role_split() {
        let count = |rank: u8| WORKER_TIERS.iter().filter(|(_, r)| *r == rank).count();
        assert_eq!(
            count(0),
            44,
            "Lighthouse control plane (+gossip/reconcile/presence/etcd_watch/lifecycle/mesh_dns/netstate_apply/validation_suite/metrics_exporter/hardware_probe/link-traffic) + storage (BUG-STORAGE-1, universal per-node mirror) + openstack (QC-2, universal Kolla-service supervision) + unit_aggregator (EXPLORER-1, universal per-node unit view) + notify (CHAT-FIX-2, universal local-notification producer) + node_grade (NODE-GRADE-1, universal per-node self-grade) + kdc_host (KDC-MESH-3 #15, universal KDE Connect host — overlay-only, opens no public port) + chat (CHAT-FIX-1, universal mesh chat worker — was on the silent unknown-worker default, now an explicit census entry) + device_control (DEVMGR-8, universal per-node device-control executor) + ARCH-5 (drift guard) 14 universal rank-0 workers that were riding the silent unknown-worker default: boot_readiness/xcp_host/kvm_health/vm_lifecycle/container/scheduler/session_broker/session_roaming/console_broker/clipboard_bridge/service_onboard/spawn_lighthouse_onboard/onboard_apply/lighthouse_probe"
        );
        assert_eq!(
            count(1),
            27,
            "Workstation = fleet (ansible-pull/app-sync/job_exec) + voice/clipboard_sync/remmina + music_autoconfig (MEDIA-8) + mesh_mount (FILEMGR-5) + bookmarks (BOOKMARKS-2) + adfilter (BOOKMARKS-7) + browser_policy (BOOKMARKS-8) + browser_passkeys (BROWSER-DD-6) + browser_session_sync (BROWSER-DD-7) + browser_read_aloud/browser_voice_command (BROWSER-DD-11) + browser_protocol/browser_share/browser_translate/browser_offline_cache/browser_security_update/browser_tab_suspend (BROWSER-DD-12) + seat_remote_input (KDC-MESH-6) + desktop_sources (CHOOSER-1) + media_sources (MEDIA-14) + media_server (MEDIA-15) + pty_broker (TERM-7) + transfers (TRANSFERS-1) — kdc moved to rank 0 (KDC-MESH-3)"
        );
        // No middle tier in the 2-role model — Workstation is the top rank.
        assert_eq!(
            count(2),
            0,
            "the retired Server/XCP-NG tier (rank 2) is gone"
        );
    }

    #[test]
    fn lighthouse_runs_only_the_control_plane() {
        let r = Role::Lighthouse.rank();
        for w in [
            "nebula_supervisor",
            "heartbeat",
            "health_reconciler",
            "mesh_router",
            "bus_supervisor",
        ] {
            assert!(runs(w, r), "Lighthouse must run {w}");
        }
        // KDC-MESH-3 (#15) — kdc_host is NO LONGER in this list: it is now a
        // universal rank-0 worker that DOES run on a Lighthouse (see
        // `kdc_host_runs_on_every_role`). Overlay-only, so it opens no public port.
        for w in ["ansible-pull", "app-sync", "voice_config"] {
            assert!(!runs(w, r), "Lighthouse must NOT run {w}");
        }
    }

    #[test]
    fn workstation_adds_fleet_and_desktop() {
        // The retired Server tier folded into Workstation: it now runs BOTH the
        // fleet workers AND the desktop stack (a headless box runs them too — the
        // desktop workers idle without a display).
        let r = Role::Workstation.rank();
        for w in [
            "ansible-pull",
            "app-sync",
            "voice_config",
            "clipboard_sync",
            "kdc_host",
        ] {
            assert!(runs(w, r), "Workstation must run {w}");
        }
    }

    #[test]
    fn workstation_runs_every_worker() {
        let r = Role::Workstation.rank();
        for (name, _) in WORKER_TIERS {
            assert!(runs(name, r), "Workstation must run {name}");
        }
    }

    #[test]
    fn storage_mirror_publishes_on_every_role_including_workstation() {
        // BUG-STORAGE-1 — the storage worker is a universal per-node topology
        // mirror. It MUST spawn (and thus publish `state/storage/<node>`) on a
        // Workstation — a seated user manages their local disks — and still on a
        // Lighthouse (an honest, often-Unavailable mirror). Pinned at rank 0.
        assert_eq!(
            min_rank("storage"),
            0,
            "storage is a universal (rank-0) worker"
        );
        assert!(
            runs("storage", Role::Workstation.rank()),
            "the storage mirror MUST run on a Workstation (the live BUG-STORAGE-1)"
        );
        assert!(
            runs("storage", Role::Lighthouse.rank()),
            "the storage mirror still runs on a Lighthouse"
        );
        // ...and it is a DELIBERATE census entry now, so the `mackesd role-workers`
        // diagnostic (workers_for_rank) lists it on both roles instead of silently
        // omitting it (the omission that read as "storage doesn't run here").
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"storage"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"storage"));
        // The read/publish eligibility carries no capability gate — a plain rank
        // is enough (the live UDisks2 executor is gated inside the worker, not here).
        assert_eq!(required_capability("storage"), None);
    }

    #[test]
    fn openstack_worker_runs_on_every_role() {
        // QC-2 — the QUASAR-CLOUD universal-node premise (Q1/Q5/Q22: any-role
        // node, APIs on every node, no controller box). The worker MUST spawn on
        // every role — the fleet doctrine, not the rank, decides which Kolla
        // services a node hosts — and a node assigned none (or a pre-doctrine
        // mesh) still publishes an honest `state/openstack/<node>` mirror.
        assert_eq!(
            min_rank("openstack"),
            0,
            "openstack is a universal (rank-0) worker"
        );
        assert!(
            runs("openstack", Role::Workstation.rank()),
            "a Workstation carries cloud duties (universal node)"
        );
        assert!(
            runs("openstack", Role::Lighthouse.rank()),
            "a Lighthouse carries cloud duties too — no controller box, no exempt role"
        );
        // A DELIBERATE census entry (like storage's BUG-STORAGE-1 lesson), so
        // `mackesd role-workers` lists it on both roles rather than riding the
        // silent unknown-worker default.
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"openstack"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"openstack"));
        // No capability tag — the doctrine gate lives inside the worker's fleet
        // seam, not in the role census.
        assert_eq!(required_capability("openstack"), None);
    }

    #[test]
    fn unit_aggregator_runs_on_every_role() {
        // EXPLORER-1 — the Hero unit-explorer daemon spine is universal (#18/#20:
        // every node folds + publishes its OWN unit view, no center). It MUST spawn
        // on every role — a lighthouse publishes an honest units view too — and it
        // is a DELIBERATE rank-0 census entry (the BUG-STORAGE-1 lesson), never the
        // silent unknown-worker default.
        assert_eq!(
            min_rank("unit_aggregator"),
            0,
            "unit_aggregator is a universal (rank-0) worker"
        );
        assert!(runs("unit_aggregator", Role::Workstation.rank()));
        assert!(runs("unit_aggregator", Role::Lighthouse.rank()));
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"unit_aggregator"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"unit_aggregator"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("unit_aggregator"), None);
    }

    #[test]
    fn notify_producer_runs_on_every_role() {
        // CHAT-FIX-2 — the local-notification producer is universal (rank 0): every
        // node has its own services / disks / journal / peers to report into the
        // Chat feed the chat worker folds. A DELIBERATE rank-0 census entry (the
        // BUG-STORAGE-1 lesson), never the silent unknown-worker default — so
        // `mackesd role-workers` lists it on both roles.
        assert_eq!(
            min_rank("notify"),
            0,
            "notify is a universal (rank-0) worker"
        );
        assert!(runs("notify", Role::Workstation.rank()));
        assert!(runs("notify", Role::Lighthouse.rank()));
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"notify"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"notify"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("notify"), None);
    }

    #[test]
    fn node_grade_runs_on_every_role() {
        // NODE-GRADE-1 (node-grade.md #11) — the per-node self-grade worker is
        // UNIVERSAL (rank 0): every node computes + publishes its OWN A–F capability
        // grade, so a lighthouse grades itself too (its own headroom/health/reach
        // matters to the dock's grade list). A DELIBERATE rank-0 census entry (the
        // BUG-STORAGE-1 lesson), never the silent unknown-worker default.
        assert_eq!(
            min_rank("node_grade"),
            0,
            "node_grade is a universal (rank-0) worker"
        );
        assert!(runs("node_grade", Role::Workstation.rank()));
        assert!(runs("node_grade", Role::Lighthouse.rank()));
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"node_grade"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"node_grade"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("node_grade"), None);
    }

    #[test]
    fn kdc_host_runs_on_every_role() {
        // KDC-MESH-3 (kdc-mesh.md #15) — the KDE Connect host is UNIVERSAL (rank 0):
        // it MUST spawn on EVERY node incl. a headless Lighthouse, so the mesh-wide
        // "every node recognizes the phone" (#5) + "all nodes serve at once" (#6)
        // goals hold. It was Workstation-only (rank 1) before; the move is safe
        // because KDC-MESH-1's transport is overlay-only (binds 1716 on the Nebula
        // overlay IP, never the public NIC — so a lighthouse opens no public port).
        assert_eq!(
            min_rank("kdc_host"),
            0,
            "kdc_host is a universal (rank-0) worker"
        );
        assert!(
            runs("kdc_host", Role::Workstation.rank()),
            "a Workstation still runs the KDE Connect host"
        );
        assert!(
            runs("kdc_host", Role::Lighthouse.rank()),
            "a Lighthouse now runs the KDE Connect host too (overlay-only, no public port)"
        );
        // A DELIBERATE census entry, so `mackesd role-workers` lists it on both roles.
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"kdc_host"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"kdc_host"));
        // No capability tag — every node runs it (the overlay-only transport is the
        // gate that keeps it safe on a headless/relay node, not a role tag).
        assert_eq!(required_capability("kdc_host"), None);
    }

    #[test]
    fn chat_runs_on_every_role() {
        // CHAT-FIX-1 — the mesh chat worker is UNIVERSAL (rank 0): it MUST spawn on
        // EVERY node incl. a headless Lighthouse (live-verified on Eagle: boot log
        // `starting worker: chat`). It always ran everywhere via the silent
        // "unknown worker ⇒ rank 0" default; this pins it as an EXPLICIT census
        // entry so `mackesd role-workers` honestly lists it on both roles.
        assert_eq!(min_rank("chat"), 0, "chat is a universal (rank-0) worker");
        assert!(
            runs("chat", Role::Workstation.rank()),
            "a Workstation runs the mesh chat worker"
        );
        assert!(
            runs("chat", Role::Lighthouse.rank()),
            "a Lighthouse runs the mesh chat worker too (it always did, now explicit)"
        );
        // Present in the census table now, not riding the unknown-worker default.
        assert!(WORKER_TIERS.iter().any(|(n, _)| *n == "chat"));
        // A DELIBERATE census entry, so `mackesd role-workers` lists it on both roles.
        assert!(workers_for_rank(Role::Workstation.rank()).contains(&"chat"));
        assert!(workers_for_rank(Role::Lighthouse.rank()).contains(&"chat"));
        // No capability tag — every node runs it.
        assert_eq!(required_capability("chat"), None);
    }

    #[test]
    fn role_name_maps_each_rank_to_its_canonical_name() {
        // BOOKMARKS-8 — the browser-policy worker folds its per-role policy by
        // this name, so it MUST match the role.toml canonical names.
        assert_eq!(role_name(Role::Lighthouse.rank()), "lighthouse");
        assert_eq!(role_name(Role::Workstation.rank()), "workstation");
        // An out-of-range rank falls back to the top tier (tolerant posture).
        assert_eq!(role_name(9), "workstation");
    }

    #[test]
    fn unknown_worker_defaults_to_lighthouse() {
        assert_eq!(min_rank("some-future-worker"), 0);
        assert!(runs("some-future-worker", Role::Lighthouse.rank()));
    }

    #[test]
    fn workers_for_rank_is_a_growing_superset() {
        let lh = workers_for_rank(Role::Lighthouse.rank());
        let ws = workers_for_rank(Role::Workstation.rank());
        // 30 lighthouse-tier workers (22 control-plane + the BUG-STORAGE-1 universal
        // storage mirror + the QC-2 universal openstack worker + the EXPLORER-1
        // universal unit_aggregator + the CHAT-FIX-2 universal notify producer + the
        // NODE-GRADE-1 universal node_grade self-grade + the KDC-MESH-3 universal
        // kdc_host + the CHAT-FIX-1 universal chat worker + the DEVMGR-8 universal
        // device_control executor at rank 0); Workstation adds the 22 fleet + desktop
        // workers (incl. the TRANSFERS-1 transfers worker, BROWSER-DD-6
        // browser_passkeys owner, BROWSER-DD-7 browser_session_sync owner,
        // BROWSER-DD-11 browser read-aloud +
        // voice-command owners, and BROWSER-DD-12 browser_protocol +
        // browser_share + browser_translate + browser_offline_cache +
        // browser_security_update + browser_tab_suspend owners, plus the
        // KDC-MESH-6 seat_remote_input consumer) for the full 57 (the retired
        // Server tier folded into Workstation in the 2-role model).
        // ARCH-5 (drift guard) +14 universal rank-0 workers censused (30 → 44),
        // so both roles grow by 14: lh 30 → 44, ws 57 → 71.
        assert_eq!(lh.len(), 44);
        assert_eq!(ws.len(), 71);
        // The universal storage mirror is now a listed census entry on BOTH roles
        // (it previously ran but was omitted from this diagnostic listing).
        assert!(
            lh.contains(&"storage"),
            "Lighthouse lists the storage mirror"
        );
        assert!(
            ws.contains(&"storage"),
            "Workstation lists the storage mirror"
        );
        // Strict superset: every lighthouse worker is also in the workstation set.
        assert!(lh.iter().all(|w| ws.contains(w)));
    }

    // ── MEDIA-1: the Lighthouse_Media capability gate ──

    #[test]
    fn navidrome_gates_to_the_media_lighthouse_class() {
        // The media worker requires the Media capability, not just a rank.
        assert_eq!(required_capability("navidrome"), Some(Capability::Media));
        // A media-lighthouse (rank 0 + media tag) runs it...
        let media_lh = DeployClass {
            rank: Role::Lighthouse.rank(),
            media: true,
        };
        assert!(
            runs_in("navidrome", media_lh),
            "media-lighthouse runs navidrome"
        );
        // ...but a stock lighthouse / workstation WITHOUT the tag does NOT
        // (acceptance: container absent on a non-media node), even at higher rank.
        for rank in [Role::Lighthouse.rank(), Role::Workstation.rank()] {
            assert!(
                !runs_in("navidrome", DeployClass::plain(rank)),
                "rank {rank} without the media tag must NOT run navidrome"
            );
        }
        // The rank-only `runs` never starts a capability worker (it has no tag).
        assert!(!runs("navidrome", Role::Workstation.rank()));
    }

    #[test]
    fn media_tag_only_unlocks_the_media_worker_not_the_tier() {
        // The media tag adds the media worker WITHOUT changing the rank set:
        // a media-lighthouse runs the lighthouse control plane + navidrome,
        // never a fleet/desktop (Workstation-tier) worker.
        let media_lh = DeployClass {
            rank: Role::Lighthouse.rank(),
            media: true,
        };
        assert!(runs_in("nebula_supervisor", media_lh), "still a lighthouse");
        assert!(
            !runs_in("ansible-pull", media_lh),
            "media ≠ workstation (fleet) tier"
        );
        assert!(
            !runs_in("voice_config", media_lh),
            "media ≠ workstation tier"
        );
        let set = workers_for_class(media_lh);
        // = the 30 lighthouse-tier workers (incl. link-traffic MESHMAP-6, the
        // BUG-STORAGE-1 universal storage mirror, the QC-2 universal openstack
        // worker, the EXPLORER-1 universal unit_aggregator, the CHAT-FIX-2
        // universal notify producer, the NODE-GRADE-1 universal node_grade
        // self-grade, the KDC-MESH-3 universal kdc_host, the CHAT-FIX-1 universal
        // chat worker + the DEVMGR-8 universal device_control executor + the ARCH-5
        // 14 universal rank-0 workers) + navidrome.
        assert_eq!(set.len(), 45);
        assert!(set.contains(&"navidrome"));
        assert!(set.contains(&"nebula_supervisor"));
        assert!(!set.contains(&"ansible-pull"));
        // A plain lighthouse class never includes the media worker.
        let plain_lh = DeployClass::plain(Role::Lighthouse.rank());
        assert!(!workers_for_class(plain_lh).contains(&"navidrome"));
        assert_eq!(workers_for_class(plain_lh).len(), 44);
    }

    #[test]
    fn deploy_class_from_role_class_carries_the_media_tag() {
        let media = DeployClass::from_role_class(&RoleClass {
            role: Role::Lighthouse,
            media: true,
        });
        assert_eq!(media.rank, 0);
        assert!(media.media);
        // A non-lighthouse role can't be a media class (RoleClass enforces it).
        let ws = DeployClass::from_role_class(&RoleClass::plain(Role::Workstation));
        assert_eq!(ws.rank, 1);
        assert!(!ws.media);
    }
}
