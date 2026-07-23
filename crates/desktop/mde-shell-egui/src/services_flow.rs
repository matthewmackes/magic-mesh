//! OW-11 (shell half) — the Workbench **Services** flow.
//!
//! The operator-facing face of `onboard service-add`: pick one of the three
//! curated services, **Preview** the plan (dry-run), and **Apply** it — all over
//! the Bus, against the `mackesd` `service_onboard` worker that runs the one
//! existing onboard engine (the CLI and this panel drive the SAME core, §6).
//!
//! ## One wire contract, no daemon dependency (§6 glue)
//!
//! Exactly as [`crate::discovery`] mirrors the broker's `SessionRequest`, this
//! module leans inward only on `mde-bus` and mirrors the worker's wire contract
//! with local serde structs: the [`ServiceAddAction`] it publishes on
//! `action/onboard/service-add` serialises to the identical body the worker's
//! `parse_action` decodes (a byte-pinned test on BOTH sides keeps the mirrors
//! from drifting), and the [`ServiceAddEvent`] it renders is the worker's typed
//! result off `event/onboard/service-add`.
//!
//! ## Honest by construction (§7)
//!
//! The daemon's answer is rendered as-is: a dry-run shows the real plan steps,
//! a blocked outcome shows the retry hint, and an apply that hits the live
//! seam's typed `IntegrationGated` error is shown as exactly that — this panel
//! never fakes a success. With no Bus on the box it renders the shared
//! EmptyState idiom instead of a dead form.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, RichText};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;

/// The worker's request topic — the exact wire topic
/// `mackesd::workers::service_onboard::ACTION_TOPIC` drains.
const ACTION_TOPIC: &str = "action/onboard/service-add";

/// The worker's result topic — `service_onboard::EVENT_TOPIC`, tailed for the
/// event echoing our request id.
const EVENT_TOPIC: &str = "event/onboard/service-add";

/// Result-poll cadence while a request is in flight — the worker answers on its
/// own 2 s drain tick, so polling faster only spins.
const REFRESH: Duration = Duration::from_secs(2);

// ───────────────────────── the wire mirrors (§6) ─────────────────────────

/// The curated service catalog — the shell's mirror of the daemon's
/// `ServiceKind` (`music` | `files` | `voice` on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ServicePick {
    /// Navidrome on a media lighthouse reading DO Spaces.
    Music,
    /// P2P `mde-files` Send-To — honestly nothing to provision.
    Files,
    /// Registration to an external SIP provider.
    Voice,
}

impl ServicePick {
    /// The catalog in presentation order.
    const ALL: [ServicePick; 3] = [ServicePick::Music, ServicePick::Files, ServicePick::Voice];

    /// The row label.
    const fn label(self) -> &'static str {
        match self {
            Self::Music => "Music",
            Self::Files => "Files",
            Self::Voice => "Voice",
        }
    }

    /// The one-line description — the worklist semantics, honestly stated
    /// (Files provisions nothing; Voice registers to an EXTERNAL provider).
    const fn blurb(self) -> &'static str {
        match self {
            Self::Music => {
                "Navidrome on a media lighthouse reading the shared DO Spaces bucket \u{2014} \
                 published at music.mesh."
            }
            Self::Files => {
                "Already peer-to-peer \u{2014} mde-files Send-To over the Bus; nothing to \
                 provision."
            }
            Self::Voice => {
                "Register to an external SIP provider \u{2014} never a PBX the mesh spawns."
            }
        }
    }

    /// The lowercase wire tag (for the minted request id).
    const fn wire(self) -> &'static str {
        match self {
            Self::Music => "music",
            Self::Files => "files",
            Self::Voice => "voice",
        }
    }
}

/// Mirror of the worker's `SipParams` — the operator's external-SIP registration
/// parameters. The secret-store `creds_ref` is deliberately NOT here: the daemon
/// derives it (single source of truth), and no password is ever typed into this
/// panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SipParams {
    /// The SIP registrar host (e.g. `sip.provider.net`).
    registrar: String,
    /// The SIP address-of-record domain.
    domain: String,
    /// The SIP account username.
    username: String,
}

/// Mirror of the worker's `ServiceAddAction` — the ONE verb this panel emits.
/// Serialises to the identical `action/onboard/service-add` body the worker's
/// `parse_action` decodes (byte-pinned by a test on each side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ServiceAddAction {
    /// Shell-minted correlation id — the result event echoes it.
    id: String,
    /// Which curated service to add.
    kind: ServicePick,
    /// The external SIP account params (Voice only; omitted otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    sip: Option<SipParams>,
    /// `true` ⇒ preview the plan only (the seam is never touched).
    dry_run: bool,
}

impl ServiceAddAction {
    /// Serialise to the request body. A fixed derive-backed shape ⇒
    /// serialisation can't realistically fail (the discovery idiom).
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Mirror of the worker's typed `WireServiceError` — gated vs failed stays a
/// TYPED distinction so the render is honest about which it is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireError {
    /// The live path is honestly gated on a named prerequisite.
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

/// Mirror of the worker's `ServiceAddEvent` — the typed result this panel
/// renders as-is (plan steps / outcome summary / typed error).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ServiceAddEvent {
    /// The echoed correlation id.
    id: String,
    /// The echoed service kind.
    kind: ServicePick,
    /// Whether this answers a dry-run preview or an apply.
    dry_run: bool,
    /// The daemon's one-line summary (plan / outcome / error).
    summary: String,
    /// The plan's ordered step descriptions (empty for blocked / no-op).
    #[serde(default)]
    steps: Vec<String>,
    /// `true` for the honest retryable blocked outcomes.
    #[serde(default)]
    retry_available: bool,
    /// The typed seam error, when an apply couldn't run.
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

/// The Services flow state: the picked service, the Voice SIP account fields,
/// the in-flight request id, and the daemon's last typed answer.
pub(crate) struct ServicesFlowState {
    /// Desktop-client Bus spool (resolved once). `None` on a box with no Bus dir
    /// — the panel then renders the shared EmptyState, never a dead form.
    bus_root: Option<PathBuf>,
    /// The picked catalog entry.
    selected: ServicePick,
    /// Voice: the external provider's registrar host.
    sip_registrar: String,
    /// Voice: the SIP address-of-record domain.
    sip_domain: String,
    /// Voice: the SIP account username.
    sip_username: String,
    /// The request id awaiting its result event, if one is in flight.
    pending: Option<String>,
    /// The daemon's last answer, rendered until the next request.
    result: Option<ServiceAddEvent>,
    /// The last publish error, surfaced inline (honest; never a panic).
    last_error: Option<String>,
    /// Incremental cursor into [`EVENT_TOPIC`] (only new events are scanned).
    event_cursor: Option<String>,
    /// When the event lane was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ServicesFlowState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            selected: ServicePick::Music,
            sip_registrar: String::new(),
            sip_domain: String::new(),
            sip_username: String::new(),
            pending: None,
            result: None,
            last_error: None,
            event_cursor: None,
            last_poll: None,
        }
    }
}

impl ServicesFlowState {
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
        // arch-11: open through the shared BusReader seam.
        let Some(persist) = BusReader::new(self.bus_root.clone()).open() else {
            return;
        };
        let Ok(msgs) = persist.list_since(EVENT_TOPIC, self.event_cursor.as_deref()) else {
            return;
        };
        for msg in msgs {
            self.event_cursor = Some(msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            if let Ok(ev) = serde_json::from_str::<ServiceAddEvent>(body) {
                if self.pending.as_deref() == Some(ev.id.as_str()) {
                    self.pending = None;
                    self.result = Some(ev);
                }
            }
        }
    }

    /// The Voice SIP params, when all three fields are filled. `None` otherwise
    /// — the daemon then answers with the honest retryable "no SIP account"
    /// outcome (which names the missing pieces), not a fabricated account.
    fn sip_params(&self) -> Option<SipParams> {
        let registrar = self.sip_registrar.trim();
        let domain = self.sip_domain.trim();
        let username = self.sip_username.trim();
        (!registrar.is_empty() && !domain.is_empty() && !username.is_empty()).then(|| SipParams {
            registrar: registrar.to_string(),
            domain: domain.to_string(),
            username: username.to_string(),
        })
    }

    /// Build the wire action for the current pick. Pure + deterministic given
    /// `now_ms` (its only entropy) so the emitted shape is unit-testable —
    /// mirroring `discovery::build_open`.
    fn build_action(&self, dry_run: bool, now_ms: u64) -> ServiceAddAction {
        ServiceAddAction {
            id: format!("svc-{now_ms}-{}", self.selected.wire()),
            kind: self.selected,
            sip: (self.selected == ServicePick::Voice)
                .then(|| self.sip_params())
                .flatten(),
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
                Some("No mesh Bus directory \u{2014} can't request the service.".to_string());
            return None;
        };
        // arch-11: writer — the shared BusReader seam is read-only; publishes keep
        // Persist::open because they need the write Result to set `last_error`.
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
                self.last_error = Some(format!("Couldn't request the service: {e}"));
                None
            }
        }
    }

    /// Render the Services flow into `ui`: the three-service catalog, the Voice
    /// SIP account fields, Preview/Apply, and the daemon's typed answer.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("SERVICES")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);

        // No Bus on this box ⇒ the shared honest EmptyState, not a dead form.
        if self.bus_root.is_none() {
            crate::empty_state::show(
                ui,
                "No mesh Bus",
                "Services are added over the Bus \u{2014} join this node to a mesh first.",
            );
            return;
        }

        if let Some(err) = self.last_error.as_deref() {
            ui.colored_label(Style::DANGER, err);
            ui.add_space(Style::SP_S);
        }

        // ── the catalog ──
        for pick in ServicePick::ALL {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        self.selected == pick,
                        RichText::new(pick.label()).size(Style::BODY),
                    )
                    .clicked()
                {
                    self.selected = pick;
                }
                ui.add_space(Style::SP_S);
                mde_egui::muted_note(ui, pick.blurb());
            });
            ui.add_space(Style::SP_XS);
        }

        // ── Voice: the external SIP account (params only — the password rides
        // the mesh secret store, derived daemon-side, never typed here) ──
        if self.selected == ServicePick::Voice {
            ui.add_space(Style::SP_XS);
            sip_input(ui, "Registrar", &mut self.sip_registrar, "sip.provider.net");
            sip_input(ui, "Domain", &mut self.sip_domain, "provider.net");
            sip_input(ui, "Username", &mut self.sip_username, "alice");
        }

        // ── Preview / Apply ──
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
                .button(RichText::new("Apply").size(Style::BODY))
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
            "Preview shows the daemon's real plan; Apply asks the mesh to run it \u{2014} a \
             gated step is reported as gated, never faked.",
        );
    }
}

/// A labelled single-line SIP input on the spacing grid: a dim small label (the
/// shared `field` row's label half) beside a hinted `TextEdit`.
fn sip_input(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(Style::SP_XL * 6.0),
        );
    });
    ui.add_space(Style::SP_XS);
}

/// Render one typed [`ServiceAddEvent`]: the status dot + summary, the ordered
/// plan steps, the typed error detail, and the retry hint. Tones are `Style`
/// palette tokens keyed to the answer's honesty class: gated/blocked = WARN,
/// failed = DANGER, clean = OK.
fn result_section(ui: &mut egui::Ui, ev: &ServiceAddEvent) {
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
        mde_egui::muted_note(ui, ev.kind.label());
    });
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new(&ev.summary)
            .color(Style::TEXT)
            .size(Style::BODY),
    );

    if !ev.steps.is_empty() {
        ui.add_space(Style::SP_XS);
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

    if ev.retry_available {
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(
            ui,
            "Retry available \u{2014} the mesh keeps running; clear the blocker and run it again.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    fn state_with_bus(bus_root: Option<PathBuf>) -> ServicesFlowState {
        ServicesFlowState {
            bus_root,
            ..ServicesFlowState::default()
        }
    }

    /// Drive one headless 960×640 frame of the panel and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives
    /// minus the GPU (the discovery test idiom). Returns whether it produced
    /// any draw primitives.
    fn run_panel(state: &mut ServicesFlowState) -> bool {
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
        s.selected = ServicePick::Voice;
        s.sip_registrar = "sip.provider.net".to_string();
        s.sip_domain = "provider.net".to_string();
        s.sip_username = "alice".to_string();
        assert_eq!(
            s.build_action(true, 42).to_body(),
            r#"{"id":"svc-42-voice","kind":"voice","sip":{"registrar":"sip.provider.net","domain":"provider.net","username":"alice"},"dry_run":true}"#
        );
        // Music omits `sip` entirely (the worker's serde default fills None).
        s.selected = ServicePick::Music;
        assert_eq!(
            s.build_action(false, 7).to_body(),
            r#"{"id":"svc-7-music","kind":"music","dry_run":false}"#
        );
    }

    #[test]
    fn an_incomplete_sip_account_is_omitted_not_fabricated() {
        // Voice with a partial account sends NO sip params — the daemon then
        // answers the honest retryable "no SIP account" outcome (§7), rather
        // than this panel inventing a registrar.
        let mut s = state_with_bus(None);
        s.selected = ServicePick::Voice;
        s.sip_registrar = "sip.provider.net".to_string();
        let a = s.build_action(true, 1);
        assert!(a.sip.is_none());
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
    fn the_catalog_and_a_typed_result_tessellate() {
        // A gated apply answer — the panel must render the typed error honestly
        // (summary + step + reason), plus the catalog and the Voice fields.
        let mut s = state_with_bus(Some(PathBuf::from("/nonexistent-bus")));
        s.selected = ServicePick::Voice;
        s.result = Some(ServiceAddEvent {
            id: "svc-1-music".to_string(),
            kind: ServicePick::Music,
            dry_run: false,
            summary: "provision-music: integration-gated \u{2014} needs the live push".to_string(),
            steps: vec!["seal the DO Spaces creds".to_string()],
            retry_available: false,
            error: Some(WireError::IntegrationGated {
                step: "provision-music".to_string(),
                reason: "needs the live remote push".to_string(),
            }),
        });
        assert!(
            run_panel(&mut s),
            "the catalog + typed result produced no draw primitives"
        );
    }

    #[test]
    fn submit_publishes_the_request_and_arms_the_wait() {
        // A real temp-dir Bus: submit writes the action body onto the request
        // topic and arms the pending wait for the echoed id.
        let dir = std::env::temp_dir().join(format!("mde-svcflow-{}", now_ms()));
        let mut s = state_with_bus(Some(dir.clone()));
        s.selected = ServicePick::Files;
        let body = s.submit(true, 99).expect("publish succeeds");
        assert_eq!(s.pending.as_deref(), Some("svc-99-files"));
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
        let dir = std::env::temp_dir().join(format!("mde-svcflow-ev-{}", now_ms()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        persist
            .write(
                EVENT_TOPIC,
                Priority::Default,
                None,
                Some(
                    r#"{"id":"svc-5-files","kind":"files","dry_run":true,"summary":"Files is already peer-to-peer","steps":[],"retry_available":false}"#,
                ),
            )
            .expect("write event");

        let mut s = state_with_bus(Some(dir.clone()));
        s.pending = Some("svc-OTHER".to_string());
        s.check_events();
        assert!(
            s.result.is_none(),
            "a mismatched id never resolves the wait"
        );

        // Re-scan from the top for the matching id (fresh state, cursor unset).
        let mut s2 = state_with_bus(Some(dir.clone()));
        s2.pending = Some("svc-5-files".to_string());
        s2.check_events();
        assert!(s2.pending.is_none(), "the echoed id resolves the wait");
        let ev = s2.result.expect("the typed event is held for render");
        assert_eq!(ev.kind, ServicePick::Files);
        assert!(ev.dry_run);
        assert!(ev.summary.contains("peer-to-peer"));
        assert!(ev.error.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn catalog_blurbs_state_the_worklist_semantics() {
        // The one-line descriptions carry the honest semantics: Music names
        // Navidrome + DO Spaces + music.mesh, Files says nothing is
        // provisioned, Voice says external provider / no PBX.
        assert!(ServicePick::Music.blurb().contains("Navidrome"));
        assert!(ServicePick::Music.blurb().contains("music.mesh"));
        assert!(ServicePick::Files.blurb().contains("nothing to provision"));
        assert!(ServicePick::Voice.blurb().contains("external SIP provider"));
        for p in ServicePick::ALL {
            assert!(!p.label().is_empty());
            assert!(p.blurb().len() > p.label().len());
        }
    }
}
