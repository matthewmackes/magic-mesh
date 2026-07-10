//! The **Phones hub** surface (KDC-MESH-9, design `docs/design/kdc-mesh.md` #13).
//!
//! The desktop-side management surface for the mesh's paired phone(s). It is a
//! **thin client** of the `kdc_host` worker (§6 — "render the worker's published
//! state + drive its Bus verbs, don't reimplement"): it reads the live device
//! roster off `action/connect/devices`, the mesh **service directory** off the
//! replicated substrate (`<workgroup>/kdc-services/*.json`, KDC-MESH-7), and drives
//! the operator verbs (`unpair` / `ring` / `clipboard` / `sftp` / `browse`) over the
//! same Bus RPC path the `IaC` + Fleet surfaces use. Nothing here reimplements the
//! host, the pairing store, or the transport — the surface only presents + drives.
//!
//! Four tabs (Carbon-inspired Quasar-dark cards, §4):
//! * **Phones** — the paired-phone roster: mesh identity, live signal + battery,
//!   the per-feature toggles, and the per-phone actions (ring · send clipboard ·
//!   browse the phone · unpair fast + mesh-wide).
//! * **Files** — the node-targeted file browser (KDC-MESH-7): every node's shared
//!   roots + a shallow snapshot off the service directory, plus a live deeper browse
//!   of THIS node over `action/connect/browse`.
//! * **Commands** — the run-command catalog the phone can trigger (the mesh-ops
//!   bundle + the KDC-MESH-8 `OpenStack` lifecycle set, with the #16 blast-radius
//!   flag) + a composer that emits a `runcommands.toml` stanza to drop on a node.
//! * **Pair** — the pair-a-phone flow: the KDE Connect device name the phone sees
//!   (the "Quasar Mesh" endpoint name when this node is the mesh-fanout endpoint,
//!   #8), the reachable overlay address, and the scannable KDC-MESH-4 QR payload.
//!   The payload carries both the daemon-published mesh enroll token and the KDC
//!   pairing token when available; until a fresh token is published, the QR
//!   honestly carries `enroll:null` and labels that leg pending (§7).

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use mde_egui::egui::{self, RichText};
use mde_egui::Style;
use qrcode::{types::Color, EcLevel, QrCode};

use mackes_mesh_types::peers::default_workgroup_root;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

/// How often the hub refreshes the roster + the service directory while in view.
const REFRESH: Duration = Duration::from_secs(2);

/// How long a verb request waits for its reply before the note goes honest-timeout.
const VERB_TIMEOUT: Duration = Duration::from_secs(6);

/// The KDC overlay port every host binds (mirrors `kdc_host::KDC_PORT`) — shown in
/// the pairing address.
const KDC_PORT: u16 = 1716;

/// The `MDE-MESH`-prefixed name a non-endpoint node advertises (mirrors
/// `kdc_host::MESH_NAME_PREFIX`); the endpoint advertises [`MESH_ENDPOINT_NAME`].
const MESH_NAME_PREFIX: &str = "MDE-MESH";

/// The single device name the designated mesh-fanout endpoint advertises to stock
/// KDE Connect (mirrors `mde_kdc_host::fanout::MESH_ENDPOINT_NAME`, #8).
const MESH_ENDPOINT_NAME: &str = "Quasar Mesh";

/// KDC-MESH-4 — latest-wins short-TTL mesh enroll token for the Pair QR. Minting
/// stays in the onboard path; the hub only consumes the worker-published state.
const PAIR_ENROLL_TOKEN_TOPIC: &str = "state/connect/mesh-enroll-token";

/// KDC-MESH-4 — request the daemon to mint + publish a fresh short-TTL enroll
/// token. The shell never mints enrollment credentials itself.
const PAIR_ENROLL_TOKEN_ACTION: &str = "action/connect/mesh-enroll-token";

/// Refresh a daemon token before its expiry reaches the visible Pair QR.
const PAIR_ENROLL_REFRESH_LEAD_MS: i64 = 60_000;

// ── Bus payload mirrors (local serde, the shell-tier pattern) ────────────────

/// One roster row as `action/connect/devices` publishes it (mirrors the worker's
/// `WireDevice`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct WireDevice {
    id: String,
    name: String,
    online: bool,
    battery: Option<u8>,
}

/// A directory entry off `action/connect/browse` / the service directory snapshot
/// (mirrors `mde_kdc_host::file_browse::FileEntry`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct EntryMirror {
    name: String,
    is_dir: bool,
    #[serde(default)]
    size: u64,
}

/// A node's published shared root + its shallow snapshot (mirrors
/// `service_directory::PublishedRoot`, whose `SharedRoot` is flattened).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct RootMirror {
    #[serde(default)]
    label: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    entries: Vec<EntryMirror>,
}

/// One node's published service set (mirrors `service_directory::NodeServices`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
struct NodeMirror {
    #[serde(default)]
    node_host: String,
    #[serde(default)]
    node_device_id: String,
    #[serde(default)]
    overlay_ip: Option<String>,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    shared_roots: Vec<RootMirror>,
    #[serde(default)]
    updated_ms: i64,
}

/// The `action/connect/browse` reply (mirrors the worker's `serve_browse` JSON).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct BrowseReply {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    path: String,
    #[serde(default)]
    entries: Vec<EntryMirror>,
    #[serde(default)]
    error: Option<String>,
}

/// Worker-published short-TTL mesh enroll token for the Pair QR.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct PairEnrollToken {
    token: String,
    #[serde(default)]
    expires_at_ms: Option<i64>,
    #[serde(default)]
    source: Option<String>,
}

// ── UI model ─────────────────────────────────────────────────────────────────

/// Which hub tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HubTab {
    #[default]
    Phones,
    Files,
    Commands,
    Pair,
}

impl HubTab {
    const ALL: [Self; 4] = [Self::Phones, Self::Files, Self::Commands, Self::Pair];

    const fn label(self) -> &'static str {
        match self {
            Self::Phones => "Phones",
            Self::Files => "Files",
            Self::Commands => "Commands",
            Self::Pair => "Pair",
        }
    }
}

/// The desktop-side per-feature enablement (design #13). Because pairing is the auth
/// (#16), a paired phone can trigger anything at the daemon; these toggles gate the
/// hub's own affordances — a feature switched off removes its control from THIS
/// surface (honest desktop-side scoping, not a daemon-level ACL).
// Four independent feature switches — a bool per feature reads clearer here than a
// bitflag set (they map 1:1 onto the checkboxes), so the excessive-bools lint is
// deliberately allowed for this pure toggle record.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FeatureToggles {
    ring: bool,
    clipboard: bool,
    files: bool,
    commands: bool,
}

impl Default for FeatureToggles {
    fn default() -> Self {
        // Everything on by default — the hub surfaces every capability the pairing
        // already grants; the operator narrows it here.
        Self {
            ring: true,
            clipboard: true,
            files: true,
            commands: true,
        }
    }
}

/// One in-flight verb request awaiting its reply, plus the human label for the note.
#[derive(Debug, Clone)]
struct PendingVerb {
    ulid: String,
    sent: Instant,
    label: String,
    /// `true` for the `browse` verb, whose reply feeds the file browser rather than
    /// just the status note.
    is_browse: bool,
}

/// A live deeper browse of THIS node's shared files (the `action/connect/browse`
/// leg of KDC-MESH-7).
#[derive(Debug, Clone, Default)]
struct LiveBrowse {
    path: String,
    entries: Vec<EntryMirror>,
    error: Option<String>,
}

/// The run-command composer draft (emits a `runcommands.toml` stanza to copy).
#[derive(Debug, Clone, Default)]
struct CmdDraft {
    key: String,
    name: String,
    command: String,
}

/// One entry in the phone-triggerable run-command catalog.
struct CatalogCmd {
    key: &'static str,
    name: &'static str,
    /// A short human description of what the phone triggers.
    what: &'static str,
    /// Fleet-scope blast-radius flag (#16) — the bulk `OpenStack` ops + a service
    /// restart carry it; the reads don't.
    danger: bool,
}

/// The Phones hub surface state — a pure consumer of the Bus + the substrate.
///
/// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form for a
/// crate-visible item in a private module — the shell's convention, per `dock.rs`.)
pub struct PhonesHubState {
    /// The Bus mirror dir for the verb RPC (`mde_bus::client_data_dir`); tests point
    /// it at a tempdir. `None` when the Bus is unreachable (honest-gated).
    bus_root: Option<PathBuf>,
    /// The replicated workgroup root the service directory lives under; tests point
    /// it at a tempdir.
    workgroup_root: PathBuf,
    /// This host's shunt name (for the endpoint election + the pairing address);
    /// tests override it.
    self_host: String,
    /// The last time the roster + directory were refreshed (poll self-gates on it).
    last_refresh: Option<Instant>,
    /// The live device roster, folded from `action/connect/devices`.
    devices: Vec<WireDevice>,
    /// The in-flight roster request, if any.
    roster_pending: Option<PendingVerb>,
    /// The mesh service directory, folded from `<workgroup>/kdc-services/*.json`.
    nodes: Vec<NodeMirror>,
    /// Which tab is showing.
    tab: HubTab,
    /// The desktop-side feature toggles (design #13).
    features: FeatureToggles,
    /// A per-phone clipboard draft (keyed by device id) for the Send-clipboard action.
    clip_draft: std::collections::HashMap<String, String>,
    /// The Files tab's selected node host (defaults to this node).
    browse_node: Option<String>,
    /// The live deeper browse of THIS node (the `browse` verb leg).
    live_browse: LiveBrowse,
    /// The single in-flight verb (unpair/ring/clipboard/sftp/browse), if any.
    pending: Option<PendingVerb>,
    /// The last honest one-line action note `(message, is_error)`.
    note: Option<(String, bool)>,
    /// The Commands tab composer draft.
    draft: CmdDraft,
    /// KDC-MESH-4 — operator-provided short-TTL mesh enroll token to include in
    /// the Pair QR. Manual paste overrides daemon-published state; minting remains
    /// owned by the onboard path.
    pair_enroll_token: String,
    /// KDC-MESH-4 — worker-published short-TTL token, read from
    /// [`PAIR_ENROLL_TOKEN_TOPIC`]. Manual paste above wins when non-empty.
    published_pair_enroll: Option<PairEnrollToken>,
    /// The in-flight daemon mint request for [`PAIR_ENROLL_TOKEN_ACTION`].
    pair_enroll_pending: Option<PendingVerb>,
}

impl Default for PhonesHubState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            workgroup_root: default_workgroup_root(),
            self_host: hostname(),
            last_refresh: None,
            devices: Vec::new(),
            roster_pending: None,
            nodes: Vec::new(),
            tab: HubTab::default(),
            features: FeatureToggles::default(),
            clip_draft: std::collections::HashMap::new(),
            browse_node: None,
            live_browse: LiveBrowse::default(),
            pending: None,
            note: None,
            draft: CmdDraft::default(),
            pair_enroll_token: String::new(),
            published_pair_enroll: None,
            pair_enroll_pending: None,
        }
    }
}

impl PhonesHubState {
    /// Poll the Bus + the substrate on the shared cadence — the shell calls this each
    /// frame while the surface is in view (the Chat/Storage/`IaC` tail idiom). Resolves
    /// any in-flight reply, then refreshes the roster + directory when due. No
    /// blocking await: requests are published sync and read on a later tick (§7).
    pub fn poll(&mut self, ctx: &egui::Context) {
        // 1) Resolve the roster request → fold the device list.
        if let Some((ulid, sent)) = self
            .roster_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent))
        {
            if let Some(body) = self.read_reply(&ulid) {
                self.devices = fold_devices(&body);
                self.roster_pending = None;
            } else if sent.elapsed() >= VERB_TIMEOUT {
                self.roster_pending = None; // honest miss; keep the last roster
            }
        }

        // 2) Resolve a pending verb → its note (or the browser).
        if let Some(p) = self.pending.clone() {
            if let Some(body) = self.read_reply(&p.ulid) {
                self.resolve_verb(&p, &body);
                self.pending = None;
            } else if p.sent.elapsed() >= VERB_TIMEOUT {
                self.note = Some((
                    format!(
                        "{}: no response — the KDE Connect host may be offline",
                        p.label
                    ),
                    true,
                ));
                self.pending = None;
            }
        }

        // 3) Resolve the daemon mesh-enroll mint request. The daemon also writes
        // state, but folding the reply avoids waiting one more refresh tick.
        if let Some(p) = self.pair_enroll_pending.clone() {
            if let Some(body) = self.read_reply(&p.ulid) {
                self.resolve_pair_enroll_token(&body);
                self.pair_enroll_pending = None;
            } else if p.sent.elapsed() >= VERB_TIMEOUT {
                self.pair_enroll_pending = None;
            }
        }

        // 4) Refresh the roster + the directory when due.
        let due = self.last_refresh.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_refresh = Some(Instant::now());
            self.refresh_directory();
            self.refresh_pair_enroll_token();
            if self.roster_pending.is_none() {
                self.request_roster();
            }
            ctx.request_repaint_after(REFRESH);
        }
    }

    /// Re-read the mesh service directory off the replicated substrate (a cheap local
    /// scan, KDC-MESH-7) — every node's published KDC services + shared-roots snapshot.
    fn refresh_directory(&mut self) {
        self.nodes = collect_nodes(&self.workgroup_root);
    }

    /// Re-read the daemon-minted short-TTL mesh enroll token for the Pair QR.
    /// Expired/malformed state is treated as absent so the QR stays honest.
    fn refresh_pair_enroll_token(&mut self) {
        let now = now_ms();
        self.published_pair_enroll = self
            .persist()
            .and_then(|persist| latest_pair_enroll_token(&persist, now));
        if pair_enroll_token_needs_refresh(
            &self.pair_enroll_token,
            self.published_pair_enroll.as_ref(),
            now,
        ) {
            self.request_pair_enroll_token();
        }
    }

    /// Publish an (empty) `action/connect/devices` request; the reply lands the live
    /// roster on a later tick.
    fn request_roster(&mut self) {
        if let Some(ulid) = self.publish("action/connect/devices", None) {
            self.roster_pending = Some(PendingVerb {
                ulid,
                sent: Instant::now(),
                label: "roster".to_string(),
                is_browse: false,
            });
        }
    }

    /// Ask `mackesd` to mint + publish a fresh short-TTL mesh invite for the Pair
    /// QR. Manual paste wins, so this is only called when the manual field is empty.
    fn request_pair_enroll_token(&mut self) {
        if self.pair_enroll_pending.is_some() {
            return;
        }
        if let Some(ulid) = self.publish(PAIR_ENROLL_TOKEN_ACTION, None) {
            self.pair_enroll_pending = Some(PendingVerb {
                ulid,
                sent: Instant::now(),
                label: "mesh enroll token".to_string(),
                is_browse: false,
            });
        }
    }

    fn resolve_pair_enroll_token(&mut self, body: &str) {
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            return;
        }
        let Some(token) = v
            .get("token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        else {
            return;
        };
        self.published_pair_enroll = Some(PairEnrollToken {
            token,
            expires_at_ms: v.get("expires_at_ms").and_then(serde_json::Value::as_i64),
            source: v
                .get("source")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        });
    }

    /// Publish one `action/connect/<verb>` request, recording it as the pending verb
    /// so its reply becomes the honest note. `is_browse` routes the reply to the file
    /// browser instead.
    fn drive(&mut self, verb: &str, body: &serde_json::Value, label: String, is_browse: bool) {
        let topic = format!("action/connect/{verb}");
        match self.publish(&topic, Some(&body.to_string())) {
            Some(ulid) => {
                self.note = Some((format!("{label}\u{2026}"), false));
                self.pending = Some(PendingVerb {
                    ulid,
                    sent: Instant::now(),
                    label,
                    is_browse,
                });
            }
            None => self.note = Some(("the mesh Bus is unavailable".to_string(), true)),
        }
    }

    /// Fold a resolved verb reply into the note (or the browser).
    fn resolve_verb(&mut self, p: &PendingVerb, body: &str) {
        if p.is_browse {
            self.live_browse = fold_browse(body);
            return;
        }
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            self.note = Some((format!("{}: done", p.label), false));
        } else {
            let err = v
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("failed");
            self.note = Some((format!("{}: {err}", p.label), true));
        }
    }

    // ── Bus plumbing ─────────────────────────────────────────────────────────

    fn persist(&self) -> Option<Persist> {
        Persist::open(self.bus_root.clone()?).ok()
    }

    fn publish(&self, topic: &str, body: Option<&str>) -> Option<String> {
        let persist = self.persist()?;
        publish_request(&persist, topic, Priority::Default, None, body).ok()
    }

    fn read_reply(&self, ulid: &str) -> Option<String> {
        let persist = self.persist()?;
        let msgs = persist.list_since(&reply_topic(ulid), None).ok()?;
        msgs.first()?.body.clone()
    }

    // ── Render ───────────────────────────────────────────────────────────────

    /// Draw the whole hub. Pure over `self` + the polled state (no I/O here — poll
    /// does the reads); the shell mounts it under a `push_id` like every surface.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.header(ui);
        ui.separator();
        ui.horizontal(|ui| {
            for tab in HubTab::ALL {
                let selected = self.tab == tab;
                let text = RichText::new(tab.label())
                    .size(Style::BODY)
                    .color(if selected {
                        Style::ACCENT
                    } else {
                        Style::TEXT_DIM
                    });
                if ui.selectable_label(selected, text).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(Style::SP_S);
        if let Some((msg, is_err)) = &self.note {
            let color = if *is_err { Style::DANGER } else { Style::OK };
            ui.colored_label(color, RichText::new(msg).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                HubTab::Phones => self.phones_tab(ui),
                HubTab::Files => self.files_tab(ui),
                HubTab::Commands => self.commands_tab(ui),
                HubTab::Pair => self.pair_tab(ui),
            });
    }

    /// The shared header: the mesh KDC identity + the paired/online counts.
    fn header(&self, ui: &mut egui::Ui) {
        let name = self.endpoint_name();
        let paired = self.devices.len();
        let online = self.devices.iter().filter(|d| d.online).count();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Phones")
                    .size(Style::HEADING)
                    .color(Style::TEXT_STRONG)
                    .strong(),
            );
            ui.add_space(Style::SP_M);
            ui.colored_label(
                Style::ACCENT_COMMS,
                RichText::new(format!(
                    "this mesh appears to your phone as \u{201C}{name}\u{201D}"
                ))
                .size(Style::SMALL),
            );
        });
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(format!("{paired} paired \u{00B7} {online} online")).size(Style::SMALL),
        );
    }

    /// The KDE Connect device name the phone sees: the "Quasar Mesh" endpoint name
    /// when THIS node is the designated mesh-fanout endpoint (#8), else the
    /// `MDE-MESH <host>` name.
    fn endpoint_name(&self) -> String {
        let hosts: Vec<String> = self.nodes.iter().map(|n| n.node_host.clone()).collect();
        if is_designated_endpoint(&self.self_host, &hosts) {
            MESH_ENDPOINT_NAME.to_string()
        } else {
            format!("{MESH_NAME_PREFIX} {}", self.self_host)
        }
    }

    // ── Phones tab ───────────────────────────────────────────────────────────

    fn phones_tab(&mut self, ui: &mut egui::Ui) {
        self.feature_card(ui);
        ui.add_space(Style::SP_S);
        if self.devices.is_empty() {
            empty_state(
                ui,
                "No phones paired yet",
                "Open the Pair tab to add your phone over the mesh.",
            );
            return;
        }
        // Snapshot the roster so the per-card action drives don't hold a `self` borrow.
        let devices = self.devices.clone();
        for d in &devices {
            self.phone_card(ui, d);
            ui.add_space(Style::SP_S);
        }
    }

    /// The per-feature toggle card (design #13).
    fn feature_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new("Features")
                    .size(Style::TITLE)
                    .color(Style::TEXT_STRONG),
            );
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(
                    "Desktop controls for what this hub offers. Pairing itself grants the phone \
                     full access (audited) — these gate the actions shown here.",
                )
                .size(Style::SMALL),
            );
            ui.add_space(Style::SP_XS);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.features.ring, "Find my device");
                ui.checkbox(&mut self.features.clipboard, "Clipboard");
                ui.checkbox(&mut self.features.files, "Files");
                ui.checkbox(&mut self.features.commands, "Run-commands");
            });
        });
    }

    /// One paired-phone card: identity, live signal + battery, and the gated actions.
    fn phone_card(&mut self, ui: &mut egui::Ui, d: &WireDevice) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                // Signal dot (the live overlay link) + name.
                let (dot, dot_color) = if d.online {
                    ("\u{25CF}", Style::OK)
                } else {
                    ("\u{25CB}", Style::TEXT_DIM)
                };
                ui.colored_label(dot_color, dot);
                ui.label(
                    RichText::new(&d.name)
                        .size(Style::TITLE)
                        .color(Style::TEXT_STRONG),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(
                        battery_color(d.battery),
                        RichText::new(battery_label(d.battery)).size(Style::SMALL),
                    );
                });
            });
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!(
                    "{} \u{00B7} {}",
                    signal_label(d.online),
                    short_id(&d.id)
                ))
                .size(Style::SMALL),
            );
            ui.add_space(Style::SP_XS);
            // Actions — each gated by its feature toggle (design #13).
            ui.horizontal_wrapped(|ui| {
                if self.features.ring && ui.button("Ring").clicked() {
                    self.drive(
                        "ring",
                        &serde_json::json!({ "device_id": d.id }),
                        format!("Ring {}", d.name),
                        false,
                    );
                }
                if self.features.files && ui.button("Browse phone").clicked() {
                    self.drive(
                        "sftp",
                        &serde_json::json!({ "device_id": d.id }),
                        format!("Browse {}", d.name),
                        false,
                    );
                }
                // Unpair — fast + mesh-wide (design risk: a lost phone = fleet control
                // until unpaired). The store replicates, so one unpair drops the phone
                // everywhere.
                if ui
                    .button(RichText::new("Unpair").color(Style::DANGER))
                    .on_hover_text("Removes this phone from the whole mesh")
                    .clicked()
                {
                    self.drive(
                        "unpair",
                        &serde_json::json!({ "device_id": d.id }),
                        format!("Unpair {}", d.name),
                        false,
                    );
                }
            });
            if self.features.clipboard {
                ui.add_space(Style::SP_XS);
                ui.horizontal(|ui| {
                    let draft = self.clip_draft.entry(d.id.clone()).or_default();
                    ui.add(
                        egui::TextEdit::singleline(draft)
                            .hint_text("Send clipboard text\u{2026}")
                            .desired_width(220.0),
                    );
                    let content = draft.clone();
                    if ui.button("Send").clicked() && !content.is_empty() {
                        self.drive(
                            "clipboard",
                            &serde_json::json!({ "device_id": d.id, "content": content }),
                            format!("Clipboard \u{2192} {}", d.name),
                            false,
                        );
                        self.clip_draft.insert(d.id.clone(), String::new());
                    }
                });
            }
        });
    }

    // ── Files tab (KDC-MESH-7 node-targeted browse) ──────────────────────────

    fn files_tab(&mut self, ui: &mut egui::Ui) {
        if !self.features.files {
            empty_state(
                ui,
                "Files are switched off",
                "Enable Files in the Phones tab.",
            );
            return;
        }
        if self.nodes.is_empty() {
            empty_state(
                ui,
                "No shared files published yet",
                "Each node publishes its shared roots to the mesh directory; none has synced.",
            );
            return;
        }
        // Node picker (any-node reach, #7).
        let selected = self
            .browse_node
            .clone()
            .unwrap_or_else(|| self.self_host.clone());
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Node")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            for n in &self.nodes {
                let is_sel = n.node_host == selected;
                if ui.selectable_label(is_sel, &n.node_host).clicked() {
                    self.browse_node = Some(n.node_host.clone());
                    self.live_browse = LiveBrowse::default();
                }
            }
        });
        ui.add_space(Style::SP_S);
        let node = self.nodes.iter().find(|n| n.node_host == selected).cloned();
        let Some(node) = node else {
            return;
        };
        let is_local = node.node_host == self.self_host;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new(format!("{} \u{00B7} shared files", node.node_host))
                    .size(Style::TITLE)
                    .color(Style::TEXT_STRONG),
            );
            if node.shared_roots.is_empty() {
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new("nothing shared on this node").size(Style::SMALL),
                );
            }
            for root in &node.shared_roots {
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(format!("{}  ({})", root.label, root.path))
                        .size(Style::SMALL)
                        .color(Style::ACCENT),
                );
                for e in &root.entries {
                    ui.horizontal(|ui| {
                        ui.label(if e.is_dir { "\u{1F4C1}" } else { "\u{1F4C4}" });
                        ui.label(RichText::new(&e.name).size(Style::SMALL).color(Style::TEXT));
                        if !e.is_dir {
                            ui.colored_label(
                                Style::TEXT_DIM,
                                RichText::new(human_size(e.size)).size(Style::SMALL),
                            );
                        }
                    });
                }
            }
        });
        // Local deeper browse — the live `action/connect/browse` leg (this node only;
        // the substrate snapshot above serves every node's top level, design #11b).
        if is_local {
            ui.add_space(Style::SP_S);
            self.local_browse_card(ui, &node);
        } else {
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(
                    "Top level shown from the mesh directory. Open this node's hub for a live \
                     deeper browse.",
                )
                .size(Style::SMALL),
            );
        }
    }

    /// The live deeper browse of THIS node over `action/connect/browse` (#11b).
    fn local_browse_card(&mut self, ui: &mut egui::Ui, node: &NodeMirror) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Live browse (this node)")
                        .size(Style::TITLE)
                        .color(Style::TEXT_STRONG),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let default_path = node
                        .shared_roots
                        .first()
                        .map(|r| r.path.clone())
                        .unwrap_or_default();
                    if ui.button("Refresh").clicked() {
                        let path = if self.live_browse.path.is_empty() {
                            default_path
                        } else {
                            self.live_browse.path.clone()
                        };
                        self.drive(
                            "browse",
                            &serde_json::json!({ "path": path }),
                            "Browse".to_string(),
                            true,
                        );
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.live_browse.path)
                        .hint_text("path within a shared root\u{2026}")
                        .desired_width(320.0),
                );
                if ui.button("Open").clicked() {
                    let path = self.live_browse.path.clone();
                    self.drive(
                        "browse",
                        &serde_json::json!({ "path": path }),
                        "Browse".to_string(),
                        true,
                    );
                }
            });
            if let Some(err) = &self.live_browse.error {
                ui.colored_label(Style::DANGER, RichText::new(err).size(Style::SMALL));
            }
            for e in &self.live_browse.entries {
                ui.horizontal(|ui| {
                    ui.label(if e.is_dir { "\u{1F4C1}" } else { "\u{1F4C4}" });
                    ui.label(RichText::new(&e.name).size(Style::SMALL).color(Style::TEXT));
                });
            }
        });
    }

    // ── Commands tab (KDC-MESH-8 catalog + composer) ─────────────────────────

    fn commands_tab(&mut self, ui: &mut egui::Ui) {
        if !self.features.commands {
            empty_state(
                ui,
                "Run-commands are switched off",
                "Enable Run-commands in the Phones tab.",
            );
            return;
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new("Commands your phone can trigger")
                    .size(Style::TITLE)
                    .color(Style::TEXT_STRONG),
            );
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(
                    "The curated run-commands the KDE Connect host offers the phone (mesh-ops + \
                     the fleet OpenStack lifecycle). The phone triggers these; there is no \
                     arbitrary shell.",
                )
                .size(Style::SMALL),
            );
            for c in runcommand_catalog() {
                ui.add_space(Style::SP_XS);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(c.name).size(Style::BODY).color(if c.danger {
                        Style::WARN
                    } else {
                        Style::TEXT
                    }));
                    if c.danger {
                        ui.colored_label(
                            Style::WARN,
                            RichText::new("fleet-wide").size(Style::SMALL),
                        );
                    }
                });
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(format!("{}  \u{2014}  {}", c.key, c.what)).size(Style::SMALL),
                );
            }
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::WARN,
                RichText::new(
                    "Blast radius (#16): a paired phone can drive fleet-wide OpenStack lifecycle \
                     with no per-command confirm — every action is in the audit log.",
                )
                .size(Style::SMALL),
            );
        });
        ui.add_space(Style::SP_S);
        self.composer_card(ui);
    }

    /// The custom-command composer: emits a `runcommands.toml` stanza to drop on a
    /// node's `<config>/runcommands.toml` (which the host reads, `load_runcommands`).
    /// Honest — it produces the config text to copy, never a faked cross-node write.
    fn composer_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new("Add a custom command")
                    .size(Style::TITLE)
                    .color(Style::TEXT_STRONG),
            );
            egui::Grid::new("phones-cmd-composer")
                .num_columns(2)
                .spacing([Style::SP_S, Style::SP_XS])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Key")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    ui.add(egui::TextEdit::singleline(&mut self.draft.key).desired_width(300.0));
                    ui.end_row();
                    ui.label(
                        RichText::new("Name")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    ui.add(egui::TextEdit::singleline(&mut self.draft.name).desired_width(300.0));
                    ui.end_row();
                    ui.label(
                        RichText::new("Command")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.command).desired_width(300.0),
                    );
                    ui.end_row();
                });
            let stanza = toml_stanza(&self.draft);
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("runcommands.toml")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            let mut shown = stanza.clone();
            ui.add(
                egui::TextEdit::multiline(&mut shown)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            if ui.button("Copy stanza").clicked() {
                ui.ctx().copy_text(stanza);
                self.note = Some(("copied the runcommands.toml stanza".to_string(), false));
            }
        });
    }

    // ── Pair tab ─────────────────────────────────────────────────────────────

    fn pair_tab(&mut self, ui: &mut egui::Ui) {
        let name = self.endpoint_name();
        let local = self.nodes.iter().find(|n| n.node_host == self.self_host);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new("Pair a phone over the mesh")
                    .size(Style::TITLE)
                    .color(Style::TEXT_STRONG),
            );
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(
                    "On the phone: run the Nebula client (join the mesh), then open KDE Connect. \
                     This node appears as the device below — tap it and confirm to pair. Pairing \
                     is recognized mesh-wide (pair once, reach every node).",
                )
                .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            labeled(ui, "Device name", &name);
            let addr = local.and_then(|n| n.overlay_ip.clone()).map_or_else(
                || "overlay IP not resolved yet (node not on the mesh)".to_string(),
                |ip| format!("{ip}:{KDC_PORT}"),
            );
            labeled(ui, "Reachable at", &addr);
            let code = pairing_code(
                local.map(|n| n.node_device_id.as_str()),
                local.and_then(|n| n.overlay_ip.as_deref()),
            );
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("Mesh enroll token")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.pair_enroll_token)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("mesh:... or mde-invite:...")
                    .desired_width(f32::INFINITY),
            );
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(
                    "Optional override: paste a fresh short-TTL token from `mackesd found` \
                     or `mackesd onboard invite-issue`. Empty uses an automatically \
                     daemon-published token when available.",
                )
                .size(Style::SMALL),
            );
            let selected_enroll = selected_pair_enroll_token(
                &self.pair_enroll_token,
                self.published_pair_enroll.as_ref(),
                now_ms(),
            );
            let qr_payload = pair_qr_payload(selected_enroll, &code);
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("Pairing QR")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            render_pair_qr(ui, &qr_payload);
            ui.add_space(Style::SP_XS);
            let mut shown = qr_payload.clone();
            ui.add(
                egui::TextEdit::multiline(&mut shown)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.button("Copy QR payload").clicked() {
                    ui.ctx().copy_text(qr_payload.clone());
                    self.note = Some(("copied the pairing QR payload".to_string(), false));
                }
            });
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(pair_qr_status_text(
                    normalized_enroll_token(&self.pair_enroll_token),
                    self.published_pair_enroll.as_ref(),
                    now_ms(),
                ))
                .size(Style::SMALL),
            );
        });
    }
}

// ── pure helpers (unit-tested) ───────────────────────────────────────────────

/// This host's shunt name (mirrors `kdc_host::hostname_for_shunt`).
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// The mesh service directory dir under the workgroup root (mirrors
/// `service_directory::services_dir`).
fn services_dir(workgroup_root: &std::path::Path) -> PathBuf {
    workgroup_root.join("kdc-services")
}

/// Read every node's published service set off the substrate (mirrors
/// `service_directory::collect_all_services`). Junk / half-replicated files are
/// skipped; sorted by hostname.
fn collect_nodes(workgroup_root: &std::path::Path) -> Vec<NodeMirror> {
    let Ok(entries) = std::fs::read_dir(services_dir(workgroup_root)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(node) = serde_json::from_str::<NodeMirror>(&raw) {
            out.push(node);
        }
    }
    out.sort_by(|a, b| a.node_host.cmp(&b.node_host));
    out
}

/// Fold an `action/connect/devices` reply body into the roster (mirrors the worker's
/// sorted `WireDevice` array). A malformed reply yields an empty roster (§7).
fn fold_devices(body: &str) -> Vec<WireDevice> {
    serde_json::from_str::<Vec<WireDevice>>(body).unwrap_or_default()
}

/// Fold an `action/connect/browse` reply into the live-browse view.
fn fold_browse(body: &str) -> LiveBrowse {
    match serde_json::from_str::<BrowseReply>(body) {
        Ok(r) if r.ok => LiveBrowse {
            path: r.path,
            entries: r.entries,
            error: None,
        },
        Ok(r) => LiveBrowse {
            path: r.path,
            entries: Vec::new(),
            error: Some(r.error.unwrap_or_else(|| "browse refused".to_string())),
        },
        Err(_) => LiveBrowse {
            path: String::new(),
            entries: Vec::new(),
            error: Some("could not read the browse reply".to_string()),
        },
    }
}

/// The deterministic mesh-fanout endpoint election (mirrors
/// `mde_kdc_host::fanout::is_designated_endpoint` — the lexicographically-lowest
/// hostname, "a stable primary", #8), so the hub shows the SAME "Quasar Mesh" name
/// the worker advertises.
fn is_designated_endpoint(self_host: &str, hosts: &[String]) -> bool {
    if self_host.is_empty() {
        return false;
    }
    let mut all: Vec<&str> = hosts
        .iter()
        .map(String::as_str)
        .filter(|h| !h.is_empty())
        .collect();
    if !all.contains(&self_host) {
        all.push(self_host);
    }
    all.into_iter().min() == Some(self_host)
}

/// A human battery label — `"—"` for the "not a battery"/unknown sentinel (a phone
/// always reports; a desktop peer wouldn't be in this roster).
fn battery_label(pct: Option<u8>) -> String {
    pct.map_or_else(|| "\u{2014}".to_string(), |p| format!("{p}%"))
}

/// The battery color ramp — red below 15%, amber below 35%, else green.
const fn battery_color(pct: Option<u8>) -> egui::Color32 {
    match pct {
        Some(p) if p < 15 => Style::DANGER,
        Some(p) if p < 35 => Style::WARN,
        Some(_) => Style::OK,
        None => Style::TEXT_DIM,
    }
}

/// The signal (live overlay link) label.
const fn signal_label(online: bool) -> &'static str {
    if online {
        "signal: online"
    } else {
        "signal: offline"
    }
}

/// A short device id for the card subtitle (the first segment of a KDC UUID).
fn short_id(id: &str) -> String {
    let head: String = id.chars().take(8).collect();
    if id.len() > 8 {
        format!("{head}\u{2026}")
    } else {
        head
    }
}

/// A compact human byte size.
#[allow(clippy::cast_precision_loss)] // display-only; a file-size rounding error is invisible
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// The phone-triggerable run-command catalog (mirrors `kdc_host::default_runcommands`
/// + `cloud_command_entries`, KDC-MESH-8). Danger = fleet-wide blast radius (#16).
fn runcommand_catalog() -> Vec<CatalogCmd> {
    vec![
        CatalogCmd {
            key: "mesh-health",
            name: "Mesh health check",
            what: "healthy nodes, audit chain, version",
            danger: false,
        },
        CatalogCmd {
            key: "mesh-status",
            name: "Mesh status (peers)",
            what: "the live peer list",
            danger: false,
        },
        CatalogCmd {
            key: "disk-headroom",
            name: "Disk headroom",
            what: "free space on / and the mesh store",
            danger: false,
        },
        CatalogCmd {
            key: "restart-mesh",
            name: "Restart mesh service",
            what: "restarts mackesd on the target node",
            danger: true,
        },
        CatalogCmd {
            key: "cloud-list",
            name: "Cloud: list instances",
            what: "every Nova instance + status",
            danger: false,
        },
        CatalogCmd {
            key: "cloud-status",
            name: "Cloud: status",
            what: "instance counts by status",
            danger: false,
        },
        CatalogCmd {
            key: "cloud-start-all",
            name: "Cloud: start all instances",
            what: "starts every SHUTOFF instance",
            danger: true,
        },
        CatalogCmd {
            key: "cloud-stop-all",
            name: "Cloud: stop all instances",
            what: "stops every ACTIVE instance",
            danger: true,
        },
        CatalogCmd {
            key: "cloud-reboot-all",
            name: "Cloud: reboot all instances",
            what: "reboots every ACTIVE instance",
            danger: true,
        },
    ]
}

/// Render the composer draft as a `runcommands.toml` `[[command]]` stanza. Blank
/// fields fall back to honest placeholders so the shape is always valid TOML.
fn toml_stanza(d: &CmdDraft) -> String {
    let key = if d.key.is_empty() {
        "my-command"
    } else {
        &d.key
    };
    let name = if d.name.is_empty() {
        "My command"
    } else {
        &d.name
    };
    let command = if d.command.is_empty() {
        "echo hello from the mesh"
    } else {
        &d.command
    };
    format!(
        "[[command]]\nkey = \"{}\"\nname = \"{}\"\ncommand = \"{}\"\n",
        toml_escape(key),
        toml_escape(name),
        toml_escape(command),
    )
}

/// Escape a TOML basic-string value (quotes + backslashes) so the stanza is valid.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The pairing code the QR carries — the host's overlay dial target
/// (`mde-kdc-pair:<device-id>@<overlay-ip>:1716`). Honest when the overlay isn't
/// resolved yet (§7 — no fabricated address).
fn pairing_code(device_id: Option<&str>, overlay_ip: Option<&str>) -> String {
    match (
        device_id.filter(|s| !s.is_empty()),
        overlay_ip.filter(|s| !s.is_empty()),
    ) {
        (Some(id), Some(ip)) => format!("mde-kdc-pair:{id}@{ip}:{KDC_PORT}"),
        _ => "this node is not on the mesh yet — no pairing address".to_string(),
    }
}

#[derive(Debug, serde::Serialize)]
struct PairQrPayload<'a> {
    v: u8,
    enroll: Option<&'a str>,
    pair: &'a str,
}

/// KDC-MESH-4 QR payload. The URI prefix makes the scan target explicit; the
/// base64url body is compact JSON so future phone-side scanners can validate `v`
/// before consuming the enroll/pair fields.
fn pair_qr_payload(enroll_token: Option<&str>, pair_code: &str) -> String {
    let payload = PairQrPayload {
        v: 1,
        enroll: enroll_token.filter(|s| !s.trim().is_empty()),
        pair: pair_code,
    };
    let body = serde_json::to_vec(&payload).unwrap_or_default();
    format!("mde-kdc-mesh-pair:{}", URL_SAFE_NO_PAD.encode(body))
}

fn normalized_enroll_token(token: &str) -> Option<&str> {
    let trimmed = token.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn selected_pair_enroll_token<'a>(
    manual: &'a str,
    published: Option<&'a PairEnrollToken>,
    now_ms: i64,
) -> Option<&'a str> {
    normalized_enroll_token(manual).or_else(|| fresh_pair_enroll_token(published, now_ms))
}

fn fresh_pair_enroll_token(published: Option<&PairEnrollToken>, now_ms: i64) -> Option<&str> {
    let published = published?;
    let token = normalized_enroll_token(&published.token)?;
    if published
        .expires_at_ms
        .is_some_and(|expiry| expiry <= now_ms)
    {
        return None;
    }
    Some(token)
}

fn pair_enroll_token_needs_refresh(
    manual: &str,
    published: Option<&PairEnrollToken>,
    now_ms: i64,
) -> bool {
    if normalized_enroll_token(manual).is_some() {
        return false;
    }
    let Some(published) = published else {
        return true;
    };
    if normalized_enroll_token(&published.token).is_none() {
        return true;
    }
    published
        .expires_at_ms
        .is_some_and(|expiry| expiry <= now_ms.saturating_add(PAIR_ENROLL_REFRESH_LEAD_MS))
}

fn pair_qr_status_text(
    manual: Option<&str>,
    published: Option<&PairEnrollToken>,
    now_ms: i64,
) -> String {
    if manual.is_some() {
        return "The QR payload includes the KDC pairing target and the pasted mesh enroll token."
            .to_string();
    }
    match fresh_pair_enroll_token(published, now_ms) {
        Some(_) => "The QR payload includes the KDC pairing target and the latest daemon-published mesh enroll token.".to_string(),
        None => "The QR payload includes the KDC pairing target. No fresh mesh enroll token is published yet, so the enroll leg remains pending.".to_string(),
    }
}

fn latest_pair_enroll_token(persist: &Persist, now_ms: i64) -> Option<PairEnrollToken> {
    let msgs = persist.list_since(PAIR_ENROLL_TOKEN_TOPIC, None).ok()?;
    let body = msgs.last()?.body.as_deref()?;
    let token = serde_json::from_str::<PairEnrollToken>(body).ok()?;
    fresh_pair_enroll_token(Some(&token), now_ms)?;
    Some(token)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn pair_qr_matrix(payload: &str) -> Option<QrCode> {
    QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::M).ok()
}

fn render_pair_qr(ui: &mut egui::Ui, payload: &str) {
    let Some(code) = pair_qr_matrix(payload) else {
        ui.colored_label(
            Style::WARN,
            RichText::new("QR payload is too large to render").size(Style::SMALL),
        );
        return;
    };
    let modules = code.width();
    let edge = 176.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(edge, edge), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS, Style::TEXT_STRONG);
    let quiet = 4.0_f32;
    let module = edge / (modules as f32 + quiet * 2.0);
    let origin = rect.min + egui::vec2(module * quiet, module * quiet);
    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] == Color::Dark {
                let min = origin + egui::vec2(x as f32 * module, y as f32 * module);
                let max = min + egui::vec2(module.ceil(), module.ceil());
                painter.rect_filled(
                    egui::Rect::from_min_max(min, max),
                    egui::CornerRadius::ZERO,
                    Style::BG,
                );
            }
        }
    }
}

// ── small render helpers ─────────────────────────────────────────────────────

/// A labeled read-only value row.
fn labeled(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{label}:"))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.label(RichText::new(value).size(Style::SMALL).color(Style::TEXT));
    });
}

/// A centered honest empty state.
fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.add_space(Style::SP_L);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(title)
                .size(Style::TITLE)
                .color(Style::TEXT_DIM),
        );
        ui.colored_label(Style::TEXT_DIM, RichText::new(detail).size(Style::SMALL));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_devices_parses_the_roster_and_tolerates_junk() {
        let body = r#"[
            {"id":"aaaa1111bbbb","name":"Pixel","online":true,"battery":82},
            {"id":"c2","name":"Moto","online":false,"battery":null}
        ]"#;
        let got = fold_devices(body);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Pixel");
        assert_eq!(got[0].battery, Some(82));
        assert!(got[1].battery.is_none());
        // A malformed body is an honest empty roster, never a panic.
        assert!(fold_devices("not json").is_empty());
    }

    #[test]
    fn endpoint_election_mirrors_the_worker() {
        // Lowest hostname is the endpoint (the "Quasar Mesh" device).
        assert!(is_designated_endpoint(
            "eagle",
            &to_hosts(&["oak", "eagle", "pine"])
        ));
        assert!(!is_designated_endpoint(
            "oak",
            &to_hosts(&["oak", "eagle", "pine"])
        ));
        // A lone node (empty/self-only directory) is its own endpoint.
        assert!(is_designated_endpoint("solo", &[]));
    }

    #[test]
    fn endpoint_name_is_quasar_mesh_only_for_the_endpoint() {
        let mut s = state_for("eagle", &["oak", "eagle"]);
        assert_eq!(s.endpoint_name(), MESH_ENDPOINT_NAME);
        s.self_host = "oak".to_string();
        assert_eq!(s.endpoint_name(), format!("{MESH_NAME_PREFIX} oak"));
    }

    #[test]
    fn battery_and_signal_render_honestly() {
        assert_eq!(battery_label(Some(50)), "50%");
        assert_eq!(battery_label(None), "\u{2014}");
        assert_eq!(battery_color(Some(10)), Style::DANGER);
        assert_eq!(battery_color(Some(20)), Style::WARN);
        assert_eq!(battery_color(Some(90)), Style::OK);
        assert_eq!(signal_label(true), "signal: online");
        assert_eq!(signal_label(false), "signal: offline");
    }

    #[test]
    fn fold_browse_maps_ok_and_error_replies() {
        let ok = fold_browse(
            r#"{"ok":true,"path":"/pub","entries":[{"name":"a.txt","is_dir":false,"size":5}]}"#,
        );
        assert!(ok.error.is_none());
        assert_eq!(ok.entries.len(), 1);
        assert_eq!(ok.path, "/pub");
        let bad = fold_browse(r#"{"ok":false,"error":"OutsideSharedRoots"}"#);
        assert_eq!(bad.error.as_deref(), Some("OutsideSharedRoots"));
        assert!(bad.entries.is_empty());
    }

    #[test]
    fn catalog_carries_the_openstack_lifecycle_set_with_danger_flags() {
        let cat = runcommand_catalog();
        assert!(cat.iter().any(|c| c.key == "cloud-reboot-all" && c.danger));
        assert!(cat.iter().any(|c| c.key == "cloud-list" && !c.danger));
        assert!(cat.iter().any(|c| c.key == "mesh-health"));
        // Delete is deliberately NOT phone-exposed (safety, KDC-MESH-8).
        assert!(!cat.iter().any(|c| c.key.contains("delete")));
    }

    #[test]
    fn toml_stanza_is_valid_and_escapes_quotes() {
        let d = CmdDraft {
            key: "k".into(),
            name: "N".into(),
            command: "echo \"hi\"".into(),
        };
        let s = toml_stanza(&d);
        assert!(s.contains("[[command]]"));
        assert!(s.contains("key = \"k\""));
        assert!(s.contains("name = \"N\""));
        // The embedded quotes are TOML-escaped so the `[[command]]` stanza the host
        // reads (`load_runcommands`) stays valid.
        assert!(s.contains(r#"command = "echo \"hi\"""#));
    }

    #[test]
    fn pairing_code_is_the_overlay_target_or_honest_when_off_mesh() {
        assert_eq!(
            pairing_code(Some("dev1"), Some("10.42.0.9")),
            format!("mde-kdc-pair:dev1@10.42.0.9:{KDC_PORT}")
        );
        assert!(pairing_code(Some("dev1"), None).contains("not on the mesh"));
        assert!(pairing_code(None, Some("10.42.0.9")).contains("not on the mesh"));
    }

    #[test]
    fn pair_qr_payload_carries_enroll_and_pair_fields() {
        let pair = format!("mde-kdc-pair:dev1@10.42.0.9:{KDC_PORT}");
        let payload = pair_qr_payload(Some("mesh:magic@203.0.113.10:4243#bearer?fp=abc"), &pair);
        assert!(payload.starts_with("mde-kdc-mesh-pair:"));
        let encoded = payload
            .strip_prefix("mde-kdc-mesh-pair:")
            .expect("payload prefix");
        let body = URL_SAFE_NO_PAD.decode(encoded).expect("base64url payload");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json payload");
        assert_eq!(json["v"], 1);
        assert_eq!(json["enroll"], "mesh:magic@203.0.113.10:4243#bearer?fp=abc");
        assert_eq!(json["pair"], pair);
        assert!(
            pair_qr_matrix(&payload).is_some(),
            "combined enroll+pair payload should fit in a QR matrix"
        );
    }

    #[test]
    fn pair_qr_payload_honestly_allows_pending_enroll_token() {
        let pair = format!("mde-kdc-pair:dev1@10.42.0.9:{KDC_PORT}");
        let payload = pair_qr_payload(None, &pair);
        let encoded = payload
            .strip_prefix("mde-kdc-mesh-pair:")
            .expect("payload prefix");
        let body = URL_SAFE_NO_PAD.decode(encoded).expect("base64url payload");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json payload");
        assert!(json["enroll"].is_null());
        assert_eq!(json["pair"], pair);
    }

    #[test]
    fn normalized_enroll_token_trims_manual_token_for_the_qr() {
        assert_eq!(
            normalized_enroll_token("  mesh:m@1.2.3.4:4243#b  "),
            Some("mesh:m@1.2.3.4:4243#b")
        );
        assert_eq!(normalized_enroll_token(" \n\t "), None);
    }

    #[test]
    fn daemon_published_enroll_token_feeds_the_pair_qr_when_manual_is_empty() {
        let token = PairEnrollToken {
            token: "mesh:auto@203.0.113.10:4243#bearer?fp=abc".to_string(),
            expires_at_ms: Some(2_000),
            source: Some("onboard".to_string()),
        };
        assert_eq!(
            selected_pair_enroll_token("", Some(&token), 1_000),
            Some("mesh:auto@203.0.113.10:4243#bearer?fp=abc")
        );
        assert!(
            pair_qr_status_text(None, Some(&token), 1_000).contains("daemon-published"),
            "status text should name the automatic source"
        );
    }

    #[test]
    fn manual_enroll_token_overrides_the_daemon_published_token() {
        let token = PairEnrollToken {
            token: "mesh:auto@203.0.113.10:4243#bearer?fp=abc".to_string(),
            expires_at_ms: Some(2_000),
            source: None,
        };
        assert_eq!(
            selected_pair_enroll_token(" mesh:manual@203.0.113.20:4243#m ", Some(&token), 1_000),
            Some("mesh:manual@203.0.113.20:4243#m")
        );
    }

    #[test]
    fn expired_daemon_published_enroll_token_is_not_put_in_the_qr() {
        let token = PairEnrollToken {
            token: "mesh:expired@203.0.113.10:4243#bearer?fp=abc".to_string(),
            expires_at_ms: Some(999),
            source: None,
        };
        assert_eq!(selected_pair_enroll_token("", Some(&token), 1_000), None);
        assert!(
            pair_qr_status_text(None, Some(&token), 1_000).contains("pending"),
            "status text should stay honest when the published token is stale"
        );
    }

    #[test]
    fn pair_enroll_token_refreshes_only_when_automatic_token_is_missing_or_expiring() {
        let fresh = PairEnrollToken {
            token: "mde-invite:fresh".to_string(),
            expires_at_ms: Some(120_001),
            source: Some("kdc-host".to_string()),
        };
        let expiring = PairEnrollToken {
            token: "mde-invite:expiring".to_string(),
            expires_at_ms: Some(120_000),
            source: Some("kdc-host".to_string()),
        };
        assert!(!pair_enroll_token_needs_refresh(
            " mde-invite:manual ",
            None,
            60_000
        ));
        assert!(pair_enroll_token_needs_refresh("", None, 60_000));
        assert!(!pair_enroll_token_needs_refresh("", Some(&fresh), 60_000));
        assert!(pair_enroll_token_needs_refresh("", Some(&expiring), 60_000));
    }

    #[test]
    fn hub_requests_and_folds_daemon_enroll_token_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("bus");
        let mut state = PhonesHubState {
            bus_root: Some(tmp.path().to_path_buf()),
            workgroup_root: PathBuf::from("/nonexistent"),
            self_host: "eagle".to_string(),
            ..Default::default()
        };
        let ctx = egui::Context::default();

        state.poll(&ctx);
        let request = persist
            .list_since(PAIR_ENROLL_TOKEN_ACTION, None)
            .expect("mint request")
            .pop()
            .expect("one mint request");
        persist
            .write(
                &reply_topic(&request.ulid),
                Priority::Default,
                None,
                Some(
                    r#"{"ok":true,"token":"mde-invite:auto","expires_at_ms":123456,"source":"kdc-host"}"#,
                ),
            )
            .expect("reply");

        state.poll(&ctx);

        assert!(state.pair_enroll_pending.is_none());
        assert_eq!(
            selected_pair_enroll_token("", state.published_pair_enroll.as_ref(), 1_000),
            Some("mde-invite:auto")
        );
        assert_eq!(
            state
                .published_pair_enroll
                .as_ref()
                .and_then(|t| t.source.as_deref()),
            Some("kdc-host")
        );
    }

    #[test]
    fn latest_pair_enroll_token_reads_latest_wins_bus_state() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("bus");
        persist
            .write(
                PAIR_ENROLL_TOKEN_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"token":"mesh:old@203.0.113.1:4243#old","expires_at_ms":2000}"#),
            )
            .expect("write old");
        persist
            .write(
                PAIR_ENROLL_TOKEN_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"token":"mesh:new@203.0.113.2:4243#new","expires_at_ms":3000}"#),
            )
            .expect("write new");
        let got = latest_pair_enroll_token(&persist, 1_000).expect("fresh token");
        assert_eq!(got.token, "mesh:new@203.0.113.2:4243#new");
        assert!(latest_pair_enroll_token(&persist, 4_000).is_none());
    }

    #[test]
    fn collect_nodes_reads_the_service_directory_off_a_tempdir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = services_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("eagle.json"),
            r#"{"node_host":"eagle","node_device_id":"id-e","overlay_ip":"10.42.0.2","services":["files"],"shared_roots":[{"label":"Public","path":"/home/mm/Public","entries":[{"name":"a.txt","is_dir":false,"size":3}]}],"updated_ms":1}"#,
        )
        .unwrap();
        std::fs::write(dir.join("bad.json"), b"not json").unwrap();
        let nodes = collect_nodes(tmp.path());
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_host, "eagle");
        assert_eq!(nodes[0].shared_roots[0].entries[0].name, "a.txt");
    }

    #[test]
    fn human_size_is_compact() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }

    // ── test helpers ──────────────────────────────────────────────────────────

    fn to_hosts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    /// A hub state with a fixed self-host + a directory of the given hosts (no I/O).
    fn state_for(self_host: &str, hosts: &[&str]) -> PhonesHubState {
        let mut s = PhonesHubState {
            bus_root: None,
            workgroup_root: PathBuf::from("/nonexistent"),
            self_host: self_host.to_string(),
            ..Default::default()
        };
        s.nodes = hosts
            .iter()
            .map(|h| NodeMirror {
                node_host: (*h).to_string(),
                ..Default::default()
            })
            .collect();
        s
    }
}
