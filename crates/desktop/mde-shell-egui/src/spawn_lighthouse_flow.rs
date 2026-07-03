//! OW-7 (shell half) — the Workbench **Spawn Lighthouse** flow.
//!
//! The operator-facing face of `onboard spawn-lighthouse`: promote a lone
//! Workstation's LAN-only mesh by standing up its first always-on **lighthouse**
//! (and migrating the CA to it). Pick where to provision — a **cloud** droplet
//! (`zone1-do`) or a **local** cloud-hypervisor VM — optionally as an HA
//! **pair**, **Preview** the plan (dry-run), and **Spawn** it — all over the Bus,
//! against the `mackesd` `spawn_lighthouse_onboard` worker that runs the one
//! existing onboard engine (the CLI and this panel drive the SAME core, §6).
//!
//! ## One wire contract, no daemon dependency (§6 glue)
//!
//! Exactly as [`crate::services_flow`] mirrors the `service_onboard` worker, this
//! module leans inward only on `mde-bus` and mirrors the worker's wire contract
//! with local serde structs: the [`SpawnLighthouseAction`] it publishes on
//! `action/onboard/spawn-lighthouse` serialises to the identical body the
//! worker's `parse_action` decodes (a byte-pinned test on BOTH sides keeps the
//! mirrors from drifting), and the [`SpawnLighthouseEvent`] it renders is the
//! worker's typed result off `event/onboard/spawn-lighthouse`.
//!
//! ## Honest by construction (§7)
//!
//! The daemon's answer is rendered as-is: a dry-run shows the real plan summary +
//! the ordered CA-migration steps, a **LAN-only** outcome (no cloud token / no
//! local virt / not founded) shows the retry hint, and a Spawn that hits the live
//! seam's typed `IntegrationGated` error is shown as exactly that — this panel
//! never fakes a successful spawn. With no Bus on the box it renders the shared
//! EmptyState idiom instead of a dead form.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, RichText};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

/// The worker's request topic — the exact wire topic
/// `mackesd::workers::spawn_lighthouse_onboard::ACTION_TOPIC` drains.
const ACTION_TOPIC: &str = "action/onboard/spawn-lighthouse";

/// The worker's result topic — `spawn_lighthouse_onboard::EVENT_TOPIC`, tailed
/// for the event echoing our request id.
const EVENT_TOPIC: &str = "event/onboard/spawn-lighthouse";

/// Result-poll cadence while a request is in flight — the worker answers on its
/// own 2 s drain tick, so polling faster only spins.
const REFRESH: Duration = Duration::from_secs(2);

// ───────────────────────── the wire mirrors (§6) ─────────────────────────

/// The provision target — the shell's mirror of the daemon's `SpawnTargetKind`
/// (`cloud` | `local` on the wire). The wire carries only the discriminant; the
/// daemon maps it to the shared `do-lighthouse-join` defaults (region/size,
/// vCPU/mem), so no off-policy shape can be named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TargetPick {
    /// A cloud droplet — `DigitalOcean`, the `zone1-do` `IaC`.
    Cloud,
    /// A local cloud-hypervisor VM on this host.
    Local,
}

impl TargetPick {
    /// The choices in presentation order (cloud first — the durable off-desktop
    /// CA home the verb exists to create).
    const ALL: [TargetPick; 2] = [TargetPick::Cloud, TargetPick::Local];

    /// The row label.
    const fn label(self) -> &'static str {
        match self {
            Self::Cloud => "Cloud droplet",
            Self::Local => "Local VM",
        }
    }

    /// The one-line description — honestly stated (cloud = a DO droplet off the
    /// `zone1-do` IaC; local = a cloud-hypervisor VM on this host).
    const fn blurb(self) -> &'static str {
        match self {
            Self::Cloud => {
                "A DigitalOcean droplet on the zone1-do IaC \u{2014} a durable, always-on CA home \
                 off the desktop."
            }
            Self::Local => {
                "A cloud-hypervisor VM on this host \u{2014} an always-on lighthouse without a \
                 cloud account."
            }
        }
    }

    /// The lowercase wire tag (for the minted request id).
    const fn wire(self) -> &'static str {
        match self {
            Self::Cloud => "cloud",
            Self::Local => "local",
        }
    }
}

/// Mirror of the worker's `SpawnLighthouseAction` — the ONE verb this panel
/// emits. Serialises to the identical `action/onboard/spawn-lighthouse` body the
/// worker's `parse_action` decodes (byte-pinned by a test on each side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SpawnLighthouseAction {
    /// Shell-minted correlation id — the result event echoes it.
    id: String,
    /// Where to provision (cloud droplet vs local CH VM).
    target: TargetPick,
    /// Provision two lighthouses for quorum/HA.
    pair: bool,
    /// `true` ⇒ preview the plan only (the seam is never touched).
    dry_run: bool,
}

impl SpawnLighthouseAction {
    /// Serialise to the request body. A fixed derive-backed shape ⇒
    /// serialisation can't realistically fail (the services-flow idiom).
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Mirror of the worker's typed `WireProvisionError` — gated vs failed stays a
/// TYPED distinction so the render is honest about which it is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireError {
    /// The live path is honestly gated on a named prerequisite (cloud token /
    /// live SSH / the CA signer).
    IntegrationGated {
        /// Which seam step is gated.
        step: String,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// A step failed for a concrete runtime reason.
    Failed {
        /// Which seam step failed.
        step: String,
        /// The failure detail.
        reason: String,
    },
}

/// Mirror of the worker's `SpawnLighthouseEvent` — the typed result this panel
/// renders as-is (plan summary / CA-migration steps / LAN-only hint / typed
/// error).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct SpawnLighthouseEvent {
    /// The echoed correlation id.
    id: String,
    /// The echoed target.
    target: TargetPick,
    /// Whether a pair was requested, echoed.
    #[serde(default)]
    pair: bool,
    /// Whether this answers a dry-run preview or a spawn.
    dry_run: bool,
    /// The daemon's one-line summary (plan / outcome / error).
    summary: String,
    /// The ordered CA-migration step descriptions (empty for LAN-only).
    #[serde(default)]
    steps: Vec<String>,
    /// How many lighthouses this plan stands up (0 for LAN-only, 1, or 2).
    #[serde(default)]
    lighthouse_count: usize,
    /// `true` for the honest retryable LAN-only outcome.
    #[serde(default)]
    retry_available: bool,
    /// What the operator must fix before a retry succeeds (LAN-only only).
    #[serde(default)]
    lan_only_hint: Option<String>,
    /// The typed seam error, when a spawn couldn't run.
    #[serde(default)]
    error: Option<WireError>,
}

/// Wall-clock milliseconds since the Unix epoch (saturated) — the request-id
/// entropy. Passed in on the pure path so the wire shape stays testable.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

// ──────────────────────────── the flow state ────────────────────────────

/// The Spawn Lighthouse flow state: the picked target, the HA-pair toggle, the
/// in-flight request id, and the daemon's last typed answer.
pub(crate) struct SpawnLighthouseFlowState {
    /// Desktop-client Bus spool (resolved once). `None` on a box with no Bus dir
    /// — the panel then renders the shared EmptyState, never a dead form.
    bus_root: Option<PathBuf>,
    /// The picked provision target.
    target: TargetPick,
    /// Provision an HA pair (two lighthouses) rather than a lone one.
    pair: bool,
    /// The request id awaiting its result event, if one is in flight.
    pending: Option<String>,
    /// The daemon's last answer, rendered until the next request.
    result: Option<SpawnLighthouseEvent>,
    /// The last publish error, surfaced inline (honest; never a panic).
    last_error: Option<String>,
    /// Incremental cursor into [`EVENT_TOPIC`] (only new events are scanned).
    event_cursor: Option<String>,
    /// When the event lane was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for SpawnLighthouseFlowState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            target: TargetPick::Cloud,
            pair: false,
            pending: None,
            result: None,
            last_error: None,
            event_cursor: None,
            last_poll: None,
        }
    }
}

impl SpawnLighthouseFlowState {
    /// The bus-poll seam: while a request is in flight, tail the event lane on
    /// the fixed cadence and keep the repaint heartbeat alive so the daemon's
    /// answer surfaces without operator input. Cheap per frame — it self-gates
    /// and is a no-op with nothing pending.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() {
            return;
        }
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.check_events();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Read new result events (advancing the cursor); the one echoing the
    /// pending id resolves the wait. Ids are unique per request, so a stale
    /// event can never satisfy a new wait.
    fn check_events(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(msgs) = persist.list_since(EVENT_TOPIC, self.event_cursor.as_deref()) else {
            return;
        };
        for msg in msgs {
            self.event_cursor = Some(msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            if let Ok(ev) = serde_json::from_str::<SpawnLighthouseEvent>(body) {
                if self.pending.as_deref() == Some(ev.id.as_str()) {
                    self.pending = None;
                    self.result = Some(ev);
                }
            }
        }
    }

    /// Build the wire action for the current pick. Pure + deterministic given
    /// `now_ms` (its only entropy) so the emitted shape is unit-testable —
    /// mirroring `services_flow::build_action`.
    fn build_action(&self, dry_run: bool, now_ms: u64) -> SpawnLighthouseAction {
        SpawnLighthouseAction {
            id: format!("lh-{now_ms}-{}", self.target.wire()),
            target: self.target,
            pair: self.pair,
            dry_run,
        }
    }

    /// Publish the request and arm the result wait. Returns the published wire
    /// body (for the test to assert the shape); `None` when the publish failed.
    fn submit(&mut self, dry_run: bool, now_ms: u64) -> Option<String> {
        let action = self.build_action(dry_run, now_ms);
        let body = action.to_body();
        let Some(root) = self.bus_root.clone() else {
            self.last_error =
                Some("No mesh Bus directory \u{2014} can't request the spawn.".to_string());
            return None;
        };
        match Persist::open(root)
            .and_then(|p| p.write(ACTION_TOPIC, Priority::Default, None, Some(&body)))
        {
            Ok(_) => {
                self.last_error = None;
                self.pending = Some(action.id);
                self.result = None;
                self.last_poll = None;
                Some(body)
            }
            Err(e) => {
                self.last_error = Some(format!("Couldn't request the spawn: {e}"));
                None
            }
        }
    }

    /// Render the Spawn Lighthouse flow into `ui`: the cloud/local target
    /// catalog, the HA-pair toggle, Preview/Spawn, and the daemon's typed answer.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("SPAWN LIGHTHOUSE")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(
            ui,
            "Promote this LAN-only mesh: stand up its first always-on lighthouse and migrate the \
             CA to it.",
        );
        ui.add_space(Style::SP_XS);

        // No Bus on this box ⇒ the shared honest EmptyState, not a dead form.
        if self.bus_root.is_none() {
            crate::session::empty_state(
                ui,
                "No mesh Bus",
                "A lighthouse is spawned over the Bus \u{2014} join this node to a mesh first.",
            );
            return;
        }

        if let Some(err) = self.last_error.as_deref() {
            ui.colored_label(Style::DANGER, err);
            ui.add_space(Style::SP_S);
        }

        // ── the target catalog ──
        for pick in TargetPick::ALL {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        self.target == pick,
                        RichText::new(pick.label()).size(Style::BODY),
                    )
                    .clicked()
                {
                    self.target = pick;
                }
                ui.add_space(Style::SP_S);
                mde_egui::muted_note(ui, pick.blurb());
            });
            ui.add_space(Style::SP_XS);
        }

        // ── the HA-pair option ──
        ui.add_space(Style::SP_XS);
        ui.checkbox(
            &mut self.pair,
            RichText::new("Spawn an HA pair (two lighthouses for etcd quorum)").size(Style::BODY),
        );

        // ── Preview / Spawn ──
        ui.add_space(Style::SP_S);
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("Preview (dry run)").size(Style::BODY))
                .clicked()
            {
                self.submit(true, now_ms());
            }
            ui.add_space(Style::SP_S);
            if ui
                .button(RichText::new("Spawn").size(Style::BODY))
                .clicked()
            {
                self.submit(false, now_ms());
            }
            if self.pending.is_some() {
                ui.add_space(Style::SP_S);
                mde_egui::muted_note(ui, "Waiting for the mesh to answer\u{2026}");
            }
        });

        // ── the daemon's typed answer, rendered as-is (§7) ──
        if let Some(ev) = self.result.clone() {
            ui.add_space(Style::SP_M);
            result_section(ui, &ev);
        }

        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(
            ui,
            "Preview shows the daemon's real plan; Spawn asks the mesh to run it \u{2014} the live \
             cloud/SSH provision + CA migration is reported as gated when it can't run, never faked.",
        );
    }
}

/// Render one typed [`SpawnLighthouseEvent`]: the status dot + summary, the
/// ordered CA-migration steps, the lighthouse count, the LAN-only hint, and the
/// typed error. Tones are `Style` palette tokens keyed to the answer's honesty
/// class: gated/LAN-only = WARN, failed = DANGER, clean = OK.
fn result_section(ui: &mut egui::Ui, ev: &SpawnLighthouseEvent) {
    let tone = match &ev.error {
        Some(WireError::IntegrationGated { .. }) => Style::WARN,
        Some(WireError::Failed { .. }) => Style::DANGER,
        None if ev.retry_available => Style::WARN,
        None => Style::OK,
    };
    ui.horizontal(|ui| {
        mde_egui::status_dot(ui, tone);
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(if ev.dry_run { "Plan" } else { "Result" })
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        mde_egui::muted_note(
            ui,
            if ev.pair {
                format!("{} \u{00B7} HA pair", ev.target.label())
            } else {
                ev.target.label().to_string()
            },
        );
    });
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new(&ev.summary)
            .color(Style::TEXT)
            .size(Style::BODY),
    );

    if ev.lighthouse_count > 0 {
        ui.add_space(Style::SP_XS);
        mde_egui::field(
            ui,
            "Stands up",
            &format!(
                "{} lighthouse{}",
                ev.lighthouse_count,
                if ev.lighthouse_count == 1 { "" } else { "s" }
            ),
            Style::ACCENT,
        );
    }

    if !ev.steps.is_empty() {
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(ui, "CA migration:");
        for (i, step) in ev.steps.iter().enumerate() {
            mde_egui::muted_note(ui, format!("{}. {step}", i + 1));
        }
    }

    if let Some(err) = &ev.error {
        let (class, step, reason, err_tone) = match err {
            WireError::IntegrationGated { step, reason } => {
                ("Integration-gated", step, reason, Style::WARN)
            }
            WireError::Failed { step, reason } => ("Failed", step, reason, Style::DANGER),
        };
        ui.add_space(Style::SP_XS);
        mde_egui::field(ui, class, step, err_tone);
        ui.label(
            RichText::new(reason)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    }

    if let Some(hint) = &ev.lan_only_hint {
        ui.add_space(Style::SP_XS);
        mde_egui::field(ui, "Retry once you", hint, Style::WARN);
    }

    if ev.retry_available {
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(
            ui,
            "Retry available \u{2014} the mesh keeps running LAN-only; clear the blocker and run \
             it again.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    fn state_with_bus(bus_root: Option<PathBuf>) -> SpawnLighthouseFlowState {
        SpawnLighthouseFlowState {
            bus_root,
            ..SpawnLighthouseFlowState::default()
        }
    }

    /// Drive one headless 960×640 frame of the panel and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives
    /// minus the GPU (the services-flow test idiom). Returns whether it produced
    /// any draw primitives.
    fn run_panel(state: &mut SpawnLighthouseFlowState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn the_action_serialises_to_the_worker_wire_shape() {
        // Pin the wire contract byte-for-byte — the identical string the worker
        // module pins, so the two mirrors can't silently drift (§6).
        let mut s = state_with_bus(None);
        s.target = TargetPick::Cloud;
        assert_eq!(
            s.build_action(true, 42).to_body(),
            r#"{"id":"lh-42-cloud","target":"cloud","pair":false,"dry_run":true}"#
        );
        // A local spawn with the HA pair set.
        s.target = TargetPick::Local;
        s.pair = true;
        assert_eq!(
            s.build_action(false, 7).to_body(),
            r#"{"id":"lh-7-local","target":"local","pair":true,"dry_run":false}"#
        );
    }

    #[test]
    fn no_bus_renders_the_honest_empty_state() {
        let mut s = state_with_bus(None);
        assert!(
            run_panel(&mut s),
            "the no-Bus EmptyState produced no draw primitives"
        );
        // No form was armed — nothing pending, nothing to render as a result.
        assert!(s.pending.is_none());
        assert!(s.result.is_none());
    }

    #[test]
    fn the_catalog_and_a_typed_gated_result_tessellate() {
        // A gated spawn answer — the panel must render the typed error honestly
        // (summary + step + reason), plus the catalog and the pair toggle.
        let mut s = state_with_bus(Some(PathBuf::from("/nonexistent-bus")));
        s.target = TargetPick::Cloud;
        s.result = Some(SpawnLighthouseEvent {
            id: "lh-1-cloud".to_string(),
            target: TargetPick::Cloud,
            pair: false,
            dry_run: false,
            summary: "provision: integration-gated \u{2014} needs a cloud token".to_string(),
            steps: vec![],
            lighthouse_count: 1,
            retry_available: false,
            lan_only_hint: None,
            error: Some(WireError::IntegrationGated {
                step: "provision".to_string(),
                reason: "needs a cloud token (DIGITALOCEAN_ACCESS_TOKEN)".to_string(),
            }),
        });
        assert!(
            run_panel(&mut s),
            "the catalog + typed gated result produced no draw primitives"
        );
    }

    #[test]
    fn a_lan_only_result_renders_the_retry_hint() {
        // The honest LAN-only outcome: a retry hint + the retry note, no error.
        let mut s = state_with_bus(Some(PathBuf::from("/nonexistent-bus")));
        s.result = Some(SpawnLighthouseEvent {
            id: "lh-2-cloud".to_string(),
            target: TargetPick::Cloud,
            pair: false,
            dry_run: true,
            summary: "stays LAN-only (no cloud token) \u{2014} retry available".to_string(),
            steps: vec![],
            lighthouse_count: 0,
            retry_available: true,
            lan_only_hint: Some("set a cloud token, then retry".to_string()),
            error: None,
        });
        assert!(
            run_panel(&mut s),
            "the LAN-only result produced no draw primitives"
        );
    }

    #[test]
    fn submit_publishes_the_request_and_arms_the_wait() {
        // A real temp-dir Bus: submit writes the action body onto the request
        // topic and arms the pending wait for the echoed id.
        let dir = std::env::temp_dir().join(format!("mde-lhflow-{}", now_ms()));
        let mut s = state_with_bus(Some(dir.clone()));
        s.target = TargetPick::Cloud;
        let body = s.submit(true, 99).expect("publish succeeds");
        assert_eq!(s.pending.as_deref(), Some("lh-99-cloud"));
        assert!(s.result.is_none());

        let persist = Persist::open(dir.clone()).expect("open bus");
        let msgs = persist.list_since(ACTION_TOPIC, None).expect("list");
        assert_eq!(msgs.len(), 1, "one submit ⇒ one queued action");
        assert_eq!(msgs[0].body.as_deref(), Some(body.as_str()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_matching_result_event_resolves_the_wait() {
        // The daemon's answer (matching id) lands on the event lane ⇒ the wait
        // resolves and the typed event is held for render; a mismatched id is
        // ignored (ids are unique per request).
        let dir = std::env::temp_dir().join(format!("mde-lhflow-ev-{}", now_ms()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        persist
            .write(
                EVENT_TOPIC,
                Priority::Default,
                None,
                Some(
                    r#"{"id":"lh-5-cloud","target":"cloud","pair":false,"dry_run":true,"summary":"spawn a lighthouse for mesh `home-x`","steps":["mint a lighthouse-scoped join token"],"lighthouse_count":1,"retry_available":false}"#,
                ),
            )
            .expect("write event");

        let mut s = state_with_bus(Some(dir.clone()));
        s.pending = Some("lh-OTHER".to_string());
        s.check_events();
        assert!(
            s.result.is_none(),
            "a mismatched id never resolves the wait"
        );

        // Re-scan from the top for the matching id (fresh state, cursor unset).
        let mut s2 = state_with_bus(Some(dir.clone()));
        s2.pending = Some("lh-5-cloud".to_string());
        s2.check_events();
        assert!(s2.pending.is_none(), "the echoed id resolves the wait");
        let ev = s2.result.expect("the typed event is held for render");
        assert_eq!(ev.target, TargetPick::Cloud);
        assert!(ev.dry_run);
        assert_eq!(ev.lighthouse_count, 1);
        assert_eq!(ev.steps.len(), 1);
        assert!(ev.error.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn catalog_blurbs_state_the_honest_semantics() {
        // Cloud names the zone1-do IaC + a durable CA home; Local names
        // cloud-hypervisor + no cloud account.
        assert!(TargetPick::Cloud.blurb().contains("zone1-do"));
        assert!(TargetPick::Cloud.blurb().contains("CA home"));
        assert!(TargetPick::Local.blurb().contains("cloud-hypervisor"));
        for p in TargetPick::ALL {
            assert!(!p.label().is_empty());
            assert!(p.blurb().len() > p.label().len());
        }
    }
}
