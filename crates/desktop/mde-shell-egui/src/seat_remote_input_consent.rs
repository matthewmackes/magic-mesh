//! WL-SEC-004 — the seated-user **arm/disarm consent publisher** for phone
//! remote input.
//!
//! The `mackesd` worker
//! [`mackesd_core::workers::seat_remote_input::SeatRemoteInputWorker`] refuses to
//! inject a paired phone's keyboard/mouse events into the local seat unless the
//! seated user has **armed** the seat for a bounded window (security-7 consent
//! gate). That consumer was complete; the one missing link was a shell control
//! that lets the person actually sitting at the desk grant — and revoke — that
//! consent. This module is that publisher.
//!
//! ## One wire contract (§6 glue)
//!
//! The worker drains its arm-control topic with a plain
//! `Persist::list_since(ARM_TOPIC, …)` cursor (NOT the request/reply RPC path),
//! so this module publishes with a plain `Persist::write` — exactly as
//! [`crate::discovery`] mints the broker `Open` body. The bytes we emit here are
//! the shape the worker's `parse_arm` decodes:
//!
//! * **arm** — `{"op":"arm","source":"<label>","ttl_ms":<u64>}` (plus an optional
//!   `phone` binding, unused by this control). `ttl_ms` is the bounded window; the
//!   worker clamps it to its own [`MAX_ARM_TTL_MS`] hard ceiling and auto-disarms
//!   when it lapses.
//! * **disarm** — `{"op":"disarm","source":"<label>"}` — revoke immediately.
//!
//! ## The security property
//!
//! Arming is **only ever** an explicit seated-user act: the sole caller is the
//! "Arm" button in [`RemoteInputConsent::body`]. Nothing here auto-arms, and a
//! "Disarm now" button is always rendered so revocation is one click away
//! regardless of the reflected indicator's freshness. The seat starts un-armed
//! and, per the worker, fails closed on daemon restart.

use std::path::Path;

use mde_egui::egui::{self, RichText};
use mde_egui::Style;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;

/// The seated-user arm/disarm consent topic — the exact wire topic
/// `mackesd_core::workers::seat_remote_input::ARM_TOPIC` drains. A plain retained
/// topic (not an RPC request), folded by the worker's `drain_arm` cursor.
const ARM_TOPIC: &str = "action/seat/remote-input-arm";

/// The retained-latest "is this seat being remotely driven" indicator prefix
/// (mirrors `seat_remote_input::INDICATOR_PREFIX`); the worker publishes the live
/// arm state on `{INDICATOR_PREFIX}{node}` so this control can reflect it.
const INDICATOR_PREFIX: &str = "state/seat/remote-input/";

/// The worker's hard ceiling on a single arm grant (mirrors
/// `seat_remote_input::MAX_ARM_TTL_MS` = 5 min). The UI never offers a longer
/// window, so the visible countdown never silently disagrees with the clamp.
const MAX_ARM_MINUTES: u32 = 5;

/// The explicit arm windows the seated user can pick, in minutes. All are within
/// [`MAX_ARM_MINUTES`] so the granted TTL is honest (never silently clamped).
const ARM_WINDOWS_MIN: [u32; 3] = [1, 2, 5];

/// The default selected window — the worker's own `DEFAULT_ARM_TTL_MS` (2 min).
const DEFAULT_ARM_MINUTES: u32 = 2;

// ── indicator mirror (local serde, the shell-tier pattern) ───────────────────

/// The worker's `RemoteInputIndicator`, mirrored locally so the shell stays in
/// the desktop tier (never depends on the heavy `async-services` daemon crate,
/// §6). Only the fields this read-only reflection renders are kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
struct RemoteInputIndicator {
    /// True while a live arm grant permits phone injection into this seat.
    #[serde(default)]
    armed: bool,
    /// True while armed AND a phone injected within the worker's active window.
    #[serde(default)]
    active: bool,
    /// Controlling source label from the arm grant, when armed.
    #[serde(default)]
    source: Option<String>,
    /// Wall-clock ms at which the current arm auto-disarms, when armed.
    #[serde(default)]
    armed_until_ms: Option<u64>,
}

// ── UI state ─────────────────────────────────────────────────────────────────

/// The seated-user remote-input consent control (WL-SEC-004). A thin publisher:
/// it renders the current arm state (reflected read-only from the worker's
/// indicator) and drives arm/disarm by writing [`ARM_TOPIC`].
pub(crate) struct RemoteInputConsent {
    /// The currently selected arm window in minutes (an explicit, bounded choice).
    window_min: u32,
    /// The latest indicator read from the local worker, if any.
    indicator: Option<RemoteInputIndicator>,
    /// The last honest one-line note `(message, is_error)`.
    note: Option<(String, bool)>,
}

impl Default for RemoteInputConsent {
    fn default() -> Self {
        Self {
            window_min: DEFAULT_ARM_MINUTES,
            indicator: None,
            note: None,
        }
    }
}

impl RemoteInputConsent {
    /// Re-read the worker's retained arm indicator for THIS node. Called on the
    /// surface's poll cadence; a missing/unopenable Bus leaves the last state
    /// (honest off-mesh no-op, §7). Read-only — never publishes.
    pub(crate) fn refresh(&mut self, bus_root: Option<&Path>) {
        if let Some(found) = read_indicator(bus_root, &local_node_id()) {
            self.indicator = Some(found);
        }
    }

    /// Draw the consent control's body (the caller wraps it in a card frame) and
    /// drive arm/disarm on click. Pure over `self` + the polled indicator apart
    /// from the explicit publish a button press triggers.
    pub(crate) fn body(&mut self, ui: &mut egui::Ui, bus_root: Option<&Path>) {
        ui.label(
            RichText::new("Remote input")
                .size(Style::TITLE)
                .color(Style::TEXT_STRONG),
        );
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(
                "Let a paired phone drive this seat's keyboard and mouse for a short, consented \
                 window. Off unless you arm it; it auto-disarms when the window ends.",
            )
            .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        self.status_line(ui);
        ui.add_space(Style::SP_S);

        // Arm row — the ONE explicit seated-user grant. Nothing else arms.
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Arm for")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            for min in ARM_WINDOWS_MIN {
                let selected = self.window_min == min;
                if ui
                    .selectable_label(
                        selected,
                        RichText::new(minutes_label(min)).size(Style::SMALL),
                    )
                    .clicked()
                {
                    self.window_min = min;
                }
            }
            if ui
                .button(RichText::new("Arm").color(Style::ACCENT))
                .clicked()
            {
                self.arm(bus_root);
            }
        });
        ui.add_space(Style::SP_XS);

        // Disarm — always available, so revocation is one click away regardless
        // of the reflected indicator's freshness.
        if ui
            .button(RichText::new("Disarm now").color(Style::DANGER))
            .clicked()
        {
            self.disarm(bus_root);
        }

        if let Some((msg, is_err)) = &self.note {
            ui.add_space(Style::SP_XS);
            let color = if *is_err { Style::DANGER } else { Style::OK };
            ui.colored_label(color, RichText::new(msg).size(Style::SMALL));
        }
    }

    /// The read-only reflection of the worker's current arm state.
    fn status_line(&self, ui: &mut egui::Ui) {
        match self.indicator.as_ref().filter(|i| i.armed) {
            Some(ind) => {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(Style::OK, "\u{25CF}");
                    let remaining = ind
                        .armed_until_ms
                        .map(|until| fmt_remaining(until, now_ms()))
                        .unwrap_or_default();
                    let armed = if remaining.is_empty() {
                        "Armed for remote input".to_string()
                    } else {
                        format!("Armed \u{00B7} auto-disarms in {remaining}")
                    };
                    ui.label(
                        RichText::new(armed)
                            .size(Style::SMALL)
                            .color(Style::TEXT_STRONG),
                    );
                    if ind.active {
                        ui.colored_label(
                            Style::WARN,
                            RichText::new("\u{00B7} a phone is driving this seat now")
                                .size(Style::SMALL),
                        );
                    }
                });
                if let Some(src) = ind.source.as_deref().filter(|s| !s.is_empty()) {
                    ui.colored_label(
                        Style::TEXT_DIM,
                        RichText::new(format!("granted by {src}")).size(Style::SMALL),
                    );
                }
            }
            None => {
                ui.horizontal(|ui| {
                    ui.colored_label(Style::TEXT_DIM, "\u{25CB}");
                    ui.colored_label(
                        Style::TEXT_DIM,
                        RichText::new("Not armed \u{2014} no phone can drive this seat.")
                            .size(Style::SMALL),
                    );
                });
            }
        }
    }

    /// Publish an explicit arm grant for the selected window. The security anchor:
    /// this is only reachable from the "Arm" button.
    fn arm(&mut self, bus_root: Option<&Path>) {
        let ttl_ms = u64::from(self.window_min.min(MAX_ARM_MINUTES)) * 60_000;
        let mut last_error = None;
        publish_arm(bus_root, &mut last_error, ttl_ms, &arm_source(), None);
        self.note = Some(match last_error {
            Some(e) => (e, true),
            None => (
                format!("Armed for {}.", minutes_label(self.window_min)),
                false,
            ),
        });
    }

    /// Publish an immediate disarm (revoke consent now).
    fn disarm(&mut self, bus_root: Option<&Path>) {
        let mut last_error = None;
        publish_disarm(bus_root, &mut last_error, &arm_source());
        self.note = Some(match last_error {
            Some(e) => (e, true),
            None => ("Disarmed \u{2014} remote input revoked.".to_string(), false),
        });
    }
}

// ── wire builders (unit-tested — the shape the worker's `parse_arm` decodes) ──

/// The arm-grant body: `{"op":"arm","source":…,"ttl_ms":…}` plus an optional
/// `phone` binding. Decoded by `seat_remote_input::parse_arm` (op `"arm"`,
/// non-empty `source`, `ttl_ms` > 0, optional valid `phone`).
fn arm_body(ttl_ms: u64, source: &str, phone: Option<&str>) -> String {
    let mut obj = serde_json::json!({
        "op": "arm",
        "source": source,
        "ttl_ms": ttl_ms,
    });
    if let Some(phone) = phone.filter(|p| !p.is_empty()) {
        obj["phone"] = serde_json::Value::String(phone.to_string());
    }
    obj.to_string()
}

/// The disarm body: `{"op":"disarm","source":…}` — decoded by
/// `seat_remote_input::parse_arm` (op `"disarm"`, non-empty `source`).
fn disarm_body(source: &str) -> String {
    serde_json::json!({ "op": "disarm", "source": source }).to_string()
}

/// Publish an arm grant to [`ARM_TOPIC`] via the persist-first write path (the
/// same path `mde-bus publish` uses); records any failure in `last_error` and
/// never panics. A missing Bus is the honest off-mesh error, not a fake success.
fn publish_arm(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    ttl_ms: u64,
    source: &str,
    phone: Option<&str>,
) -> String {
    let body = arm_body(ttl_ms, source, phone);
    publish(bus_root, last_error, &body);
    body
}

/// Publish a disarm to [`ARM_TOPIC`] via the same write path.
fn publish_disarm(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    source: &str,
) -> String {
    let body = disarm_body(source);
    publish(bus_root, last_error, &body);
    body
}

/// The one write seam. arch-11: publishers keep `Persist::open` (not the
/// read-only [`BusReader`] seam) because they need the write `Result` to set an
/// honest `last_error`.
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, body: &str) {
    let Some(root) = bus_root else {
        *last_error = Some(
            "No mesh Bus directory — can't reach the seat's remote-input control.".to_string(),
        );
        return;
    };
    match Persist::open(root.to_path_buf())
        .and_then(|p| p.write(ARM_TOPIC, Priority::Default, None, Some(body)))
    {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't reach the mesh Bus: {e}")),
    }
}

/// Read the worker's retained arm indicator for `node` off `{INDICATOR_PREFIX}{node}`
/// through the shared read-only seam. A missing Bus / row / malformed body is the
/// honest "unknown" (`None`), never a panic.
fn read_indicator(bus_root: Option<&Path>, node: &str) -> Option<RemoteInputIndicator> {
    let persist = BusReader::new(bus_root.map(Path::to_path_buf)).open()?;
    let topic = format!("{INDICATOR_PREFIX}{node}");
    let msgs = persist.list_since(&topic, None).ok()?;
    let body = msgs.last()?.body.as_deref()?;
    serde_json::from_str(body).ok()
}

// ── pure helpers ─────────────────────────────────────────────────────────────

/// This node's id as the seat worker computes it (mirrors
/// `mackesd::default_node_id`: `MACKESD_NODE_ID` → `peer:<hostname>`), so the
/// reflected indicator topic matches the worker's publication.
fn local_node_id() -> String {
    if let Ok(v) = std::env::var("MACKESD_NODE_ID") {
        if !v.is_empty() {
            return v;
        }
    }
    format!("peer:{}", local_hostname())
}

/// The seated user's control label stamped on every grant, e.g.
/// `shell:seat-user@eagle`. Recorded by the worker as the arm `source` for the
/// indicator + audit. Kept well within the worker's 128-char `source` bound.
fn arm_source() -> String {
    format!("shell:seat-user@{}", local_hostname())
}

/// `$HOSTNAME` → `hostname(1)` → `"unknown"` (the desktop-tier idiom, shared with
/// `phones_hub`).
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// A human window label: `"1 min"` / `"5 min"`.
fn minutes_label(min: u32) -> String {
    format!("{min} min")
}

/// `M:SS` remaining until `until_ms`, saturating at `0:00` once elapsed.
fn fmt_remaining(until_ms: u64, now_ms: u64) -> String {
    let remaining_s = until_ms.saturating_sub(now_ms) / 1000;
    format!("{}:{:02}", remaining_s / 60, remaining_s % 60)
}

/// Wall-clock milliseconds since the Unix epoch (saturated, never panicking).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_body_is_the_shape_the_worker_parse_arm_decodes() {
        // Pin the arm wire contract: `op:"arm"`, a non-empty `source`, and a
        // non-zero `ttl_ms` — byte-for-byte what
        // `seat_remote_input::parse_arm` accepts. No `phone` key when unbound
        // (the worker reads that as "any paired phone may inject this window").
        let body = arm_body(120_000, "shell:seat-user@eagle", None);
        let v: serde_json::Value = serde_json::from_str(&body).expect("arm JSON");
        assert_eq!(v["op"], "arm");
        assert_eq!(v["source"], "shell:seat-user@eagle");
        assert_eq!(v["ttl_ms"], 120_000);
        assert!(
            v.get("phone").is_none(),
            "an unbound arm omits the phone key entirely"
        );
    }

    #[test]
    fn arm_body_carries_an_optional_phone_binding() {
        let body = arm_body(60_000, "shell:seat-user@eagle", Some("phone-1"));
        let v: serde_json::Value = serde_json::from_str(&body).expect("arm JSON");
        assert_eq!(v["op"], "arm");
        assert_eq!(v["phone"], "phone-1");
        assert_eq!(v["ttl_ms"], 60_000);
    }

    #[test]
    fn disarm_body_is_the_shape_the_worker_parse_arm_decodes() {
        // `op:"disarm"` + a non-empty `source` — the worker's revoke shape.
        let body = disarm_body("shell:seat-user@eagle");
        let v: serde_json::Value = serde_json::from_str(&body).expect("disarm JSON");
        assert_eq!(v["op"], "disarm");
        assert_eq!(v["source"], "shell:seat-user@eagle");
        assert!(v.get("ttl_ms").is_none(), "disarm carries no ttl");
    }

    #[test]
    fn publish_arm_writes_the_grant_to_arm_topic_and_reports_no_error() {
        // End to end over a real temp Bus: publish lands one arm row on the exact
        // topic the worker drains, decodable as an arm command.
        let bus = tempfile::tempdir().expect("bus");
        let root = bus.path().to_path_buf();
        let mut last_error = None;

        let body = publish_arm(
            Some(root.as_path()),
            &mut last_error,
            120_000,
            "shell:seat-user@eagle",
            None,
        );

        assert!(last_error.is_none(), "a live Bus publishes cleanly");
        let persist = Persist::open(root).expect("reopen");
        let rows = persist.list_since(ARM_TOPIC, None).expect("arm rows");
        assert_eq!(rows.len(), 1, "exactly one arm grant landed");
        let landed = rows[0].body.clone().expect("body");
        assert_eq!(landed, body);
        let v: serde_json::Value = serde_json::from_str(&landed).expect("arm JSON");
        assert_eq!(v["op"], "arm");
        assert_eq!(v["ttl_ms"], 120_000);
    }

    #[test]
    fn publish_disarm_writes_the_revoke_to_arm_topic() {
        let bus = tempfile::tempdir().expect("bus");
        let root = bus.path().to_path_buf();
        let mut last_error = None;

        publish_disarm(
            Some(root.as_path()),
            &mut last_error,
            "shell:seat-user@eagle",
        );

        assert!(last_error.is_none());
        let persist = Persist::open(root).expect("reopen");
        let rows = persist.list_since(ARM_TOPIC, None).expect("arm rows");
        assert_eq!(rows.len(), 1);
        let v: serde_json::Value =
            serde_json::from_str(rows[0].body.as_deref().expect("body")).expect("disarm JSON");
        assert_eq!(v["op"], "disarm");
    }

    #[test]
    fn publish_without_a_bus_is_an_honest_error_not_a_panic() {
        // No Bus dir → the honest off-mesh error (never a fake success), matching
        // the discovery.rs publish discipline.
        let mut last_error = None;
        publish_arm(
            None,
            &mut last_error,
            120_000,
            "shell:seat-user@eagle",
            None,
        );
        assert!(
            last_error
                .as_deref()
                .is_some_and(|e| e.contains("No mesh Bus")),
            "a missing Bus surfaces an error, not a panic"
        );
    }

    #[test]
    fn arm_publishes_only_from_the_explicit_grant() {
        // The security property: arming is reachable ONLY through `arm()`, which
        // the "Arm" button calls. `refresh()` (the poll path) must never write —
        // reflecting the indicator can't arm the seat. Proven here: a refresh
        // against an empty Bus leaves the topic empty; only `arm()` writes.
        let bus = tempfile::tempdir().expect("bus");
        let root = bus.path().to_path_buf();
        let mut consent = RemoteInputConsent::default();

        consent.refresh(Some(root.as_path()));
        let persist = Persist::open(root.clone()).expect("reopen");
        assert!(
            persist
                .list_since(ARM_TOPIC, None)
                .expect("rows")
                .is_empty(),
            "reflecting the indicator must never publish an arm"
        );

        consent.arm(Some(root.as_path()));
        assert!(
            !persist
                .list_since(ARM_TOPIC, None)
                .expect("rows")
                .is_empty(),
            "the explicit Arm act publishes the grant"
        );
        assert_eq!(consent.note.as_ref().map(|n| n.1), Some(false));
    }

    #[test]
    fn refresh_reflects_a_published_indicator_read_only() {
        // The read-only reflection folds the worker's retained indicator for THIS
        // node without ever writing.
        let bus = tempfile::tempdir().expect("bus");
        let root = bus.path().to_path_buf();
        let node = local_node_id();
        let persist = Persist::open(root.clone()).expect("persist");
        persist
            .write(
                &format!("{INDICATOR_PREFIX}{node}"),
                Priority::Min,
                None,
                Some(
                    &serde_json::json!({
                        "node": node,
                        "armed": true,
                        "active": false,
                        "source": "shell:seat-user@eagle",
                        "armed_until_ms": 9_000_000_000_000_u64,
                        "updated_ms": 1_u64,
                    })
                    .to_string(),
                ),
            )
            .expect("write indicator");

        let mut consent = RemoteInputConsent::default();
        consent.refresh(Some(root.as_path()));

        let ind = consent.indicator.expect("indicator folded");
        assert!(ind.armed);
        assert_eq!(ind.source.as_deref(), Some("shell:seat-user@eagle"));
    }

    #[test]
    fn fmt_remaining_counts_down_and_saturates_at_zero() {
        assert_eq!(fmt_remaining(120_000, 0), "2:00");
        assert_eq!(fmt_remaining(65_000, 0), "1:05");
        assert_eq!(
            fmt_remaining(0, 5_000),
            "0:00",
            "elapsed saturates, no wrap"
        );
    }

    #[test]
    fn arm_windows_never_exceed_the_workers_hard_ceiling() {
        // The UI never offers a window the worker would silently clamp, so the
        // visible countdown never disagrees with the granted TTL.
        for min in ARM_WINDOWS_MIN {
            assert!(min <= MAX_ARM_MINUTES, "{min} min exceeds the clamp");
        }
    }
}
