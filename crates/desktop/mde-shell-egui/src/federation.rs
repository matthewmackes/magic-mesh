//! WL-SEC-002 — the shell **Federation** panel: see a pending cross-mesh
//! federation request and Accept / Refuse it.
//!
//! Cross-mesh federation trust is an EXPLICIT local act (a foreign mesh cannot
//! connect or route until accepted — the load-bearing security property). The root
//! `mackesd` `federation_enforcer` worker owns the privileged acts (consume the
//! single-use mint, write the pair, install/remove the cross-mesh Nebula trust
//! cert); this desktop-tier panel is the operator surface over the mesh Bus (§6):
//!
//!  * It READS the retained `state/federation/<node>` mirror the worker publishes —
//!    the accepted meshes + the pending outbound offers — never the root-owned
//!    `federation.yaml` directly.
//!  * It WRITES `action/federation/{accept,revoke,refuse-mint}` requests the worker
//!    drains. **Accept** establishes the pair + installs the trust cert; **Refuse**
//!    on an accepted mesh revokes it (default-deny restored); **Cancel** withdraws a
//!    pending outbound offer.
//!
//! The security anchor mirrors [`crate::seat_remote_input_consent`]: nothing here
//! auto-accepts — an accept is only ever reachable from the explicit "Accept" button
//! after the operator types the peer's passcode.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mackes_mesh_types::cloud::CLOUD_ACTION_SCHEMA_VERSION;
use mde_egui::egui::{self, RichText};
use mde_egui::Style;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;

/// The worker's retained status mirror prefix (`state/federation/<node>`).
const STATUS_PREFIX: &str = "state/federation/";
/// The accept request lane the worker drains (`{passcode, label}`).
const ACCEPT_TOPIC: &str = "action/federation/accept";
/// The revoke request lane the worker drains (`{peer-mesh-id}`).
const REVOKE_TOPIC: &str = "action/federation/revoke";
/// The cancel-a-pending-offer lane the worker drains (`{ulid}`).
const REFUSE_MINT_TOPIC: &str = "action/federation/refuse-mint";
/// The federation worker's closed capability node scope.
const ACTION_NODE_SCOPE: &str = "federation";

// ── status mirror (local serde, the shell-tier read pattern) ─────────────────────

/// The worker's `state/federation/<node>` mirror, reflected read-only. Only the
/// fields this panel renders are kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
struct FederationStatus {
    #[serde(default)]
    enforced: bool,
    #[serde(default)]
    accepted: Vec<AcceptedPair>,
    #[serde(rename = "pending-mints", default)]
    pending_mints: Vec<PendingMint>,
}

/// One accepted foreign mesh + its grant summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
struct AcceptedPair {
    #[serde(rename = "peer-mesh-id", default)]
    peer_mesh_id: String,
    #[serde(rename = "peer-mesh-label", default)]
    peer_mesh_label: String,
    #[serde(default)]
    established: String,
    #[serde(rename = "subscribe-count", default)]
    subscribe_count: usize,
    #[serde(rename = "publish-count", default)]
    publish_count: usize,
    #[serde(rename = "excluded-count", default)]
    excluded_count: usize,
}

/// One pending outbound offer (a minted passcode awaiting the peer).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
struct PendingMint {
    #[serde(default)]
    ulid: String,
    #[serde(rename = "expires-at-unix-ms", default)]
    expires_at_unix_ms: i64,
}

// ── UI state ─────────────────────────────────────────────────────────────────────

/// The seated-operator cross-mesh federation control (WL-SEC-002).
pub(crate) struct FederationPanel {
    /// The desktop-client bus spool (resolved once; overridable in tests).
    bus_root: Option<PathBuf>,
    /// This node's id — the `state/federation/<node>` mirror key.
    node_id: String,
    /// The latest reflected status, if any.
    status: Option<FederationStatus>,
    /// The passcode the operator is typing to accept an incoming request.
    passcode_input: String,
    /// The label to record for a newly accepted mesh.
    label_input: String,
    /// The last honest one-line note `(message, is_error)`.
    note: Option<(String, bool)>,
}

impl Default for FederationPanel {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            node_id: local_node_id(),
            status: None,
            passcode_input: String::new(),
            label_input: String::new(),
            note: None,
        }
    }
}

impl FederationPanel {
    /// Re-read the worker's retained federation mirror for THIS node. Read-only —
    /// never publishes. A missing/unopenable Bus keeps the last state (§7).
    pub(crate) fn refresh(&mut self) {
        if let Some(found) = read_status(self.bus_root.as_deref(), &self.node_id) {
            self.status = Some(found);
        }
    }

    /// Draw the panel body. Pure over `self` apart from the explicit publish a button
    /// press triggers.
    pub(crate) fn body(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let _ = mde_egui::carbon_icon(ui, "security-high", Style::TITLE);
            ui.label(
                RichText::new("Cross-mesh federation")
                    .size(Style::TITLE)
                    .color(Style::TEXT_STRONG),
            );
        });
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(
                "A foreign mesh cannot connect or route into this mesh until you accept it. \
                 Accepting establishes the cross-mesh trust; refusing revokes it.",
            )
            .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        self.enforced_line(ui);
        ui.add_space(Style::SP_S);

        self.accept_row(ui);
        ui.add_space(Style::SP_S);
        self.accepted_section(ui);
        ui.add_space(Style::SP_S);
        self.pending_section(ui);

        if let Some((msg, is_err)) = &self.note {
            ui.add_space(Style::SP_XS);
            let color = if *is_err { Style::DANGER } else { Style::OK };
            ui.colored_label(color, RichText::new(msg).size(Style::SMALL));
        }
    }

    /// The one-line "enforcement is live" reflection.
    fn enforced_line(&self, ui: &mut egui::Ui) {
        let enforced = self.status.as_ref().is_some_and(|s| s.enforced);
        ui.horizontal(|ui| {
            if enforced {
                ui.colored_label(Style::OK, "\u{25CF}");
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(
                        "Runtime enforcement active — default-deny at the mesh boundary.",
                    )
                    .size(Style::SMALL),
                );
            } else {
                ui.colored_label(Style::TEXT_DIM, "\u{25CB}");
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new("Enforcement status not yet reported by this node.")
                        .size(Style::SMALL),
                );
            }
        });
    }

    /// The accept-an-incoming-passcode row — the ONE explicit accept path.
    fn accept_row(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Accept an incoming request")
                .size(Style::SMALL)
                .color(Style::TEXT_STRONG),
        );
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new("Paste the 6-word passcode the peer mesh's operator gave you.")
                .size(Style::SMALL),
        );
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.passcode_input)
                    .hint_text("six word passcode")
                    .desired_width(220.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.label_input)
                    .hint_text("label (optional)")
                    .desired_width(140.0),
            );
            let can_accept = self.passcode_input.split_whitespace().count() == 6;
            if ui
                .add_enabled(
                    can_accept,
                    egui::Button::new(RichText::new("  Accept").color(Style::ACCENT)),
                )
                .clicked()
            {
                self.accept();
            }
        });
    }

    /// The accepted-meshes list, each with a Refuse (revoke) button.
    fn accepted_section(&mut self, ui: &mut egui::Ui) {
        let pairs = self
            .status
            .as_ref()
            .map(|s| s.accepted.clone())
            .unwrap_or_default();
        ui.label(
            RichText::new("Accepted meshes")
                .size(Style::SMALL)
                .color(Style::TEXT_STRONG),
        );
        if pairs.is_empty() {
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new("None — no foreign mesh is currently federated.").size(Style::SMALL),
            );
            return;
        }
        let mut revoke: Option<String> = None;
        for p in &pairs {
            ui.horizontal_wrapped(|ui| {
                let _ = mde_egui::carbon_icon(ui, "globe", Style::SMALL);
                let name = if p.peer_mesh_label.is_empty() {
                    p.peer_mesh_id.as_str()
                } else {
                    p.peer_mesh_label.as_str()
                };
                ui.label(
                    RichText::new(name)
                        .size(Style::SMALL)
                        .color(Style::TEXT_STRONG),
                );
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(format!(
                        "\u{00B7} {} sub / {} pub / {} excluded",
                        p.subscribe_count, p.publish_count, p.excluded_count
                    ))
                    .size(Style::SMALL),
                );
                if ui
                    .button(
                        RichText::new("Refuse")
                            .color(Style::DANGER)
                            .size(Style::SMALL),
                    )
                    .clicked()
                {
                    revoke = Some(p.peer_mesh_id.clone());
                }
            });
        }
        if let Some(id) = revoke {
            self.revoke(&id);
        }
    }

    /// The pending-outbound-offers list, each with a Cancel button.
    fn pending_section(&mut self, ui: &mut egui::Ui) {
        let mints = self
            .status
            .as_ref()
            .map(|s| s.pending_mints.clone())
            .unwrap_or_default();
        if mints.is_empty() {
            return;
        }
        ui.label(
            RichText::new("Pending offers (awaiting the peer)")
                .size(Style::SMALL)
                .color(Style::TEXT_STRONG),
        );
        let now_unix_ms = unix_now_ms();
        let mut cancel: Option<String> = None;
        for m in &mints {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(format!("offer {}", short_id(&m.ulid))).size(Style::SMALL),
                );
                let expiry_state = pending_expiry_state(m.expires_at_unix_ms, now_unix_ms);
                let expiry_color = match expiry_state {
                    PendingExpiry::Active => Style::TEXT_DIM,
                    PendingExpiry::Expired | PendingExpiry::Unknown => Style::WARN,
                };
                ui.colored_label(
                    expiry_color,
                    RichText::new(pending_expiry_label(m.expires_at_unix_ms, now_unix_ms))
                        .size(Style::SMALL),
                );
                if ui
                    .button(
                        RichText::new("Cancel")
                            .color(Style::WARN)
                            .size(Style::SMALL),
                    )
                    .clicked()
                {
                    cancel = Some(m.ulid.clone());
                }
            });
        }
        if let Some(id) = cancel {
            self.refuse_mint(&id);
        }
    }

    // ── explicit publishes (the security-anchored acts) ─────────────────────────

    /// Publish an explicit accept. The security anchor: reachable ONLY from the
    /// "Accept" button, and only with a 6-word passcode.
    fn accept(&mut self) {
        let passcode = normalize_passcode(&self.passcode_input);
        if passcode.split_whitespace().count() != 6 {
            self.note = Some(("A passcode is exactly 6 words.".to_string(), true));
            return;
        }
        let label = {
            let l = self.label_input.trim();
            if l.is_empty() {
                "Remote mesh".to_string()
            } else {
                l.to_string()
            }
        };
        let body = accept_body(&passcode, &label);
        match publish(self.bus_root.as_deref(), ACCEPT_TOPIC, &body) {
            Ok(()) => {
                self.passcode_input.clear();
                self.note = Some((
                    "Accept requested — establishing the cross-mesh trust.".to_string(),
                    false,
                ));
            }
            Err(e) => self.note = Some((e, true)),
        }
    }

    /// Publish an explicit revoke (refuse) for an accepted mesh.
    fn revoke(&mut self, peer_mesh_id: &str) {
        let body = revoke_body(peer_mesh_id);
        match publish(self.bus_root.as_deref(), REVOKE_TOPIC, &body) {
            Ok(()) => {
                self.note = Some((
                    "Refuse requested — revoking the cross-mesh trust.".to_string(),
                    false,
                ))
            }
            Err(e) => self.note = Some((e, true)),
        }
    }

    /// Publish a cancel for a pending outbound offer.
    fn refuse_mint(&mut self, ulid: &str) {
        let body = refuse_mint_body(ulid);
        match publish(self.bus_root.as_deref(), REFUSE_MINT_TOPIC, &body) {
            Ok(()) => self.note = Some(("Pending offer cancelled.".to_string(), false)),
            Err(e) => self.note = Some((e, true)),
        }
    }
}

// ── wire builders (unit-tested — the shapes the worker decodes) ──────────────────

/// The accept body: `{"passcode":…,"label":…}` — decoded by
/// `federation_enforcer::handle_accept`.
fn accept_body(passcode: &str, label: &str) -> String {
    serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "passcode": passcode,
        "label": label,
    })
    .to_string()
}

/// The revoke body: `{"peer-mesh-id":…}` — decoded by
/// `federation_enforcer::handle_revoke`.
fn revoke_body(peer_mesh_id: &str) -> String {
    serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "peer-mesh-id": peer_mesh_id,
    })
    .to_string()
}

/// The refuse-mint body: `{"ulid":…}` — decoded by
/// `federation_enforcer::handle_refuse_mint`.
fn refuse_mint_body(ulid: &str) -> String {
    serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "ulid": ulid,
    })
    .to_string()
}

/// Derive the same closed capability context as the root federation worker.
fn action_context(topic: &str, body: &str) -> Result<(&'static str, String), String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| "federation request body must be JSON".to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "federation request body must be an object".to_string())?;
    match topic {
        ACCEPT_TOPIC => Ok(("federation-accept", "pair".to_string())),
        REVOKE_TOPIC => {
            let id = object
                .get("peer-mesh-id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| is_safe_id(id))
                .ok_or_else(|| "federation revoke needs a safe peer-mesh-id".to_string())?;
            Ok(("federation-revoke", format!("peer:{id}")))
        }
        REFUSE_MINT_TOPIC => {
            let id = object
                .get("ulid")
                .and_then(serde_json::Value::as_str)
                .filter(|id| is_safe_id(id))
                .ok_or_else(|| "federation refuse-mint needs a safe ulid".to_string())?;
            Ok(("federation-refuse-mint", format!("mint:{id}")))
        }
        _ => Err("unknown federation mutation topic".to_string()),
    }
}

/// Match the worker's path/topic-safe identifier contract before minting a
/// capability target. The passcode itself is deliberately never a target.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The one write seam — publishers keep `Persist::open` (not the read-only
/// [`BusReader`]) because they need the write `Result` for an honest error note.
fn publish(bus_root: Option<&Path>, topic: &str, body: &str) -> Result<(), String> {
    let Some(root) = bus_root else {
        return Err("No mesh Bus directory — can't reach the federation enforcer.".to_string());
    };
    let (verb, target) = action_context(topic, body)?;
    let body = crate::iac::authorize_root_mutation_body(body, verb, ACTION_NODE_SCOPE, &target)?;
    Persist::open(root.to_path_buf())
        .and_then(|p| p.write(topic, Priority::Default, None, Some(&body)))
        .map(|_| ())
        .map_err(|e| format!("Couldn't reach the mesh Bus: {e}"))
}

/// Read the worker's retained federation mirror for `node` through the shared
/// read-only seam. A missing Bus / row / malformed body is the honest `None`.
fn read_status(bus_root: Option<&Path>, node: &str) -> Option<FederationStatus> {
    let persist = BusReader::new(bus_root.map(Path::to_path_buf)).open()?;
    let topic = format!("{STATUS_PREFIX}{node}");
    let msgs = persist.list_since(&topic, None).ok()?;
    let body = msgs.last()?.body.as_deref()?;
    serde_json::from_str(body).ok()
}

// ── pure helpers ─────────────────────────────────────────────────────────────────

/// This node's id as the worker computes it (`MACKESD_NODE_ID` → `peer:<hostname>`),
/// so the reflected mirror topic matches the worker's publication.
fn local_node_id() -> String {
    if let Ok(v) = std::env::var("MACKESD_NODE_ID") {
        if !v.is_empty() {
            return v;
        }
    }
    format!("peer:{}", local_hostname())
}

/// `$HOSTNAME` → `hostname(1)` → `"unknown"` (the desktop-tier idiom).
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

/// Normalize a typed passcode: trim, lowercase, collapse internal whitespace — the
/// exact normalization the worker's mint consumer applies, so a stray double-space
/// doesn't cause a false mismatch.
fn normalize_passcode(s: &str) -> String {
    s.split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A short id for display (first 8 chars).
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// The UI must not make an expired or malformed expiry look like a live offer.
/// `None` is also treated as degraded: a clock failure cannot turn an offer into
/// an apparently usable capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingExpiry {
    Active,
    Expired,
    Unknown,
}

fn pending_expiry_state(expires_at_unix_ms: i64, now_unix_ms: Option<i64>) -> PendingExpiry {
    let Some(now) = now_unix_ms else {
        return PendingExpiry::Unknown;
    };
    if expires_at_unix_ms <= 0 {
        return PendingExpiry::Unknown;
    }
    if expires_at_unix_ms <= now {
        PendingExpiry::Expired
    } else {
        PendingExpiry::Active
    }
}

fn pending_expiry_label(expires_at_unix_ms: i64, now_unix_ms: Option<i64>) -> String {
    match pending_expiry_state(expires_at_unix_ms, now_unix_ms) {
        PendingExpiry::Expired => "expired — no longer usable".to_string(),
        PendingExpiry::Unknown => "expiry unavailable — degraded".to_string(),
        PendingExpiry::Active => {
            let remaining = expires_at_unix_ms - now_unix_ms.expect("active has a clock");
            format!("expires in {}", format_duration_ms(remaining))
        }
    }
}

fn format_duration_ms(milliseconds: i64) -> String {
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    let hours = milliseconds / HOUR;
    let minutes = (milliseconds % HOUR) / MINUTE;
    let seconds = (milliseconds % MINUTE) / 1_000;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{}s", seconds.max(1))
    }
}

fn unix_now_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_body_is_the_shape_the_worker_decodes() {
        let body = accept_body("mesh node link mint mode myth", "Mesh A");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["passcode"], "mesh node link mint mode myth");
        assert_eq!(v["label"], "Mesh A");
    }

    #[test]
    fn revoke_and_refuse_mint_bodies_are_the_shapes_the_worker_decodes() {
        let r: serde_json::Value = serde_json::from_str(&revoke_body("MESH-A")).unwrap();
        assert_eq!(r["peer-mesh-id"], "MESH-A");
        let m: serde_json::Value = serde_json::from_str(&refuse_mint_body("01ABC")).unwrap();
        assert_eq!(m["ulid"], "01ABC");
    }

    #[test]
    fn accept_publishes_only_from_the_explicit_grant_with_six_words() {
        // The security property: reflecting the mirror (refresh) must never publish;
        // only the explicit accept act writes, and only with a 6-word passcode.
        let bus = tempfile::tempdir().unwrap();
        let root = bus.path().to_path_buf();
        let mut panel = FederationPanel::default();
        panel.bus_root = Some(root.clone());
        panel.node_id = "peer:test".into();

        panel.refresh();
        let persist = Persist::open(root.clone()).unwrap();
        assert!(
            persist.list_since(ACCEPT_TOPIC, None).unwrap().is_empty(),
            "reflecting the mirror must never publish an accept"
        );

        // A non-6-word passcode is refused locally (no publish, an error note).
        panel.passcode_input = "too few words".into();
        panel.accept();
        assert!(persist.list_since(ACCEPT_TOPIC, None).unwrap().is_empty());
        assert_eq!(panel.note.as_ref().map(|n| n.1), Some(true));

        // A 6-word passcode publishes exactly one accept request the worker decodes.
        panel.passcode_input = "mesh node link mint mode myth".into();
        panel.label_input = "Mesh A".into();
        panel.accept();
        let rows = persist.list_since(ACCEPT_TOPIC, None).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the explicit Accept act publishes the request"
        );
        let v: serde_json::Value = serde_json::from_str(rows[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(v["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
        assert!(v["armed_token"].as_str().is_some());
        assert_eq!(v["passcode"], "mesh node link mint mode myth");
        assert_eq!(v["label"], "Mesh A");
        assert_eq!(panel.note.as_ref().map(|n| n.1), Some(false));
        assert!(
            panel.passcode_input.is_empty(),
            "the field clears on success"
        );
    }

    #[test]
    fn revoke_publishes_the_refuse_request() {
        let bus = tempfile::tempdir().unwrap();
        let root = bus.path().to_path_buf();
        let mut panel = FederationPanel::default();
        panel.bus_root = Some(root.clone());
        panel.revoke("MESH-A");
        let persist = Persist::open(root).unwrap();
        let rows = persist.list_since(REVOKE_TOPIC, None).unwrap();
        assert_eq!(rows.len(), 1);
        let v: serde_json::Value = serde_json::from_str(rows[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(v["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
        assert!(v["armed_token"].as_str().is_some());
        assert_eq!(v["peer-mesh-id"], "MESH-A");
    }

    #[test]
    fn action_context_rejects_unsafe_targets_before_minting() {
        let body = revoke_body("peer/../../root");
        assert!(action_context(REVOKE_TOPIC, &body).is_err());
        let body = refuse_mint_body("mint/escape");
        assert!(action_context(REFUSE_MINT_TOPIC, &body).is_err());
    }

    #[test]
    fn refresh_reflects_a_published_mirror_read_only() {
        let bus = tempfile::tempdir().unwrap();
        let root = bus.path().to_path_buf();
        let node = "peer:test";
        let persist = Persist::open(root.clone()).unwrap();
        persist
            .write(
                &format!("{STATUS_PREFIX}{node}"),
                Priority::Min,
                None,
                Some(
                    &serde_json::json!({
                        "node": node,
                        "enforced": true,
                        "accepted": [{
                            "peer-mesh-id": "MESH-A",
                            "peer-mesh-label": "Mesh A",
                            "established": "now",
                            "subscribe-count": 1,
                            "publish-count": 0,
                            "excluded-count": 5,
                        }],
                        "pending-mints": [],
                    })
                    .to_string(),
                ),
            )
            .unwrap();

        let mut panel = FederationPanel::default();
        panel.bus_root = Some(root);
        panel.node_id = node.into();
        panel.refresh();

        let s = panel.status.expect("mirror folded");
        assert!(s.enforced);
        assert_eq!(s.accepted.len(), 1);
        assert_eq!(s.accepted[0].peer_mesh_label, "Mesh A");
        assert_eq!(s.accepted[0].excluded_count, 5);
    }

    #[test]
    fn publish_without_a_bus_is_an_honest_error_not_a_panic() {
        let mut panel = FederationPanel::default();
        panel.bus_root = None;
        panel.passcode_input = "mesh node link mint mode myth".into();
        panel.accept();
        assert!(panel
            .note
            .as_ref()
            .is_some_and(|(m, is_err)| *is_err && m.contains("No mesh Bus")));
    }

    #[test]
    fn normalize_passcode_lowercases_and_collapses() {
        assert_eq!(normalize_passcode("  Mesh   NODE link "), "mesh node link");
    }

    #[test]
    fn pending_expiry_is_visible_and_fails_closed_when_unknown() {
        assert_eq!(
            pending_expiry_state(1_000, Some(900)),
            PendingExpiry::Active
        );
        assert_eq!(
            pending_expiry_label(91_000, Some(1_000)),
            "expires in 1m 30s"
        );
        assert_eq!(
            pending_expiry_label(1_000, Some(1_000)),
            "expired — no longer usable"
        );
        assert_eq!(
            pending_expiry_label(0, Some(1_000)),
            "expiry unavailable — degraded"
        );
        assert_eq!(
            pending_expiry_state(2_000, None),
            PendingExpiry::Unknown
        );
    }
}
