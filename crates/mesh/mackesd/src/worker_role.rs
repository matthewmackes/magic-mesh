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
    ("reconcile", 0),
    ("netstate_apply", 0),
    ("validation_suite", 0),
    ("metrics_exporter", 0),
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
    // FILEMGR-5 — the Files-surface sshfs mesh-mount worker. A desktop feature
    // (the seated user browses peers), so Workstation-tier; it idles gracefully
    // with no mount requests on a headless box.
    ("mesh_mount", 1),
    // CHOOSER-1 — the desktop-source discovery aggregator behind the Chooser
    // surface. A desktop feature (the seated user picks a desktop to connect
    // to), so Workstation-tier; it idles gracefully on a headless box (the
    // aggregation is cheap and the verbs simply never arrive).
    ("desktop_sources", 1),
    ("kdc_host", 1),
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
        assert_eq!(WORKER_TIERS.len(), 35);
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
            22,
            "Lighthouse control plane (+gossip/reconcile/presence/etcd_watch/lifecycle/mesh_dns/netstate_apply/validation_suite/metrics_exporter/hardware_probe/link-traffic)"
        );
        assert_eq!(
            count(1),
            13,
            "Workstation = fleet (ansible-pull/app-sync/job_exec) + voice/clipboard_sync/kdc/remmina + music_autoconfig (MEDIA-8) + mesh_mount (FILEMGR-5) + bookmarks (BOOKMARKS-2) + desktop_sources (CHOOSER-1) + media_sources (MEDIA-14) + media_server (MEDIA-15)"
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
        for w in ["ansible-pull", "app-sync", "voice_config", "kdc_host"] {
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
    fn unknown_worker_defaults_to_lighthouse() {
        assert_eq!(min_rank("some-future-worker"), 0);
        assert!(runs("some-future-worker", Role::Lighthouse.rank()));
    }

    #[test]
    fn workers_for_rank_is_a_growing_superset() {
        let lh = workers_for_rank(Role::Lighthouse.rank());
        let ws = workers_for_rank(Role::Workstation.rank());
        // 22 lighthouse control-plane workers; Workstation adds the 13 fleet +
        // desktop workers for the full 35 (the retired Server tier folded into
        // Workstation in the 2-role model).
        assert_eq!(lh.len(), 22);
        assert_eq!(ws.len(), 35);
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
        // = the 22 lighthouse-tier workers (incl. link-traffic, MESHMAP-6) + navidrome.
        assert_eq!(set.len(), 23);
        assert!(set.contains(&"navidrome"));
        assert!(set.contains(&"nebula_supervisor"));
        assert!(!set.contains(&"ansible-pull"));
        // A plain lighthouse class never includes the media worker.
        let plain_lh = DeployClass::plain(Role::Lighthouse.rank());
        assert!(!workers_for_class(plain_lh).contains(&"navidrome"));
        assert_eq!(workers_for_class(plain_lh).len(), 22);
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
