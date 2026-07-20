//! The **Infra as Code (`IaC`)** surface — the single workspace for ALL cloud
//! operations on the local-first **OpenTofu + Ansible** backend (WL-ARCH-001).
//!
//! The operator directive (2026-07-19) removed the old provider control plane and
//! rebuilt cloud operations on OpenTofu (provision) + Ansible (configure) over
//! local libvirt/KVM. This surface is the desktop face of that stack — a
//! six-mode workspace:
//!
//! 1. **Provision** — compose + run OpenTofu: the live resource roster (from the
//!    `state/cloud` mirror) plus the plan/apply gate (dry-run default; a
//!    typed-confirm to actually apply) and the per-instance lifecycle verbs.
//! 2. **Configure** — run Ansible: pick a playbook + target group and converge.
//! 3. **Images** — bootc / osbuild image builds (honest backend-pending until the
//!    verb lands, §7 — never faked).
//! 4. **Network** — libvirt networks (Nebula-adjacent): the libvirt backend
//!    health is real; list/compose is honestly backend-pending.
//! 5. **Containers** — Podman / Quadlet workloads (honest backend-pending).
//! 6. **Status** — day-2: per-tool backend health (OpenTofu / Ansible / libvirt),
//!    the resource roster, honest degraded states, and the session audit trail.
//!
//! ## How the cloud is consumed (§6)
//!
//! The shell never depends on `mackesd`. It **reads** the per-node status mirror
//! `state/cloud/<node>` ([`CloudState`], folded across every node) off the Bus,
//! and **emits** `action/cloud/*` verbs (provision / configure / destroy /
//! instance-{start,stop,reboot,delete}) as typed request/reply — the reply lands
//! on `reply/<request-ulid>`. Only the mesh-neutral
//! [`mackes_mesh_types::cloud`] shapes are shared; the worker owns the actual
//! `tofu` / `ansible-playbook` / `virsh` execution. The surface never shells a
//! tool itself.
//!
//! Every state is honest (§7): an off-mesh Bus is a silent degrade, an unprobed
//! backend reads its honest health (`Up` / `Down` / `Absent`, never a fabricated
//! up), an empty roster is a real "no instances", and a mode with no landed
//! backend verb renders an honest **backend pending** note rather than fake data.
//! Live mutation is operator-gated on the worker (`MDE_CLOUD_APPLY=1`); the mirror
//! carries each node's `apply_armed`, so the workspace shows plan-only vs. live
//! honestly, and every destructive intent passes a typed-confirm echo first.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;

use mde_egui::egui::{self, Color32, RichText, Sense};
use mde_egui::{carbon_icon, ChipTone, Style};

use mackes_mesh_types::cloud::{
    CloudProviderAdapter, CloudState, HealthState, ResourceTable, ServiceHealth, CLOUD_STATE_PREFIX,
};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

use crate::bus_reader::BusReader;

/// User-facing cloud product name. The backend toolchain (OpenTofu / Ansible /
/// libvirt) stays behind the Bus + shared payload contracts, so a later backend
/// can satisfy the same UI seam.
pub(super) const CLOUD_PRODUCT_LABEL: &str = "Construct Cloud";

/// The workspace title the MENUBAR-ALL bar wears.
pub(super) const WORKSPACE_TITLE: &str = "Infra as Code";

// ── the backend tools the mirror reports health for (the worker's `service_type`
//    on each health row) — provider-neutral local-first toolchain. ──
/// OpenTofu (the provision leg).
const TOOL_TOFU: &str = "opentofu";
/// Ansible (the configure leg).
const TOOL_ANSIBLE: &str = "ansible";
/// libvirt/KVM (the local VM + network backend).
const TOOL_LIBVIRT: &str = "libvirt";

/// Every backend tool the Status mode reads, in render order, with its display
/// label.
const BACKEND_TOOLS: [(&str, &str); 3] = [
    (TOOL_TOFU, "OpenTofu"),
    (TOOL_ANSIBLE, "Ansible"),
    (TOOL_LIBVIRT, "libvirt"),
];

/// The typed-confirm echo an apply intent must match before the verb publishes
/// (RUN-006's typed-arming idiom — the destructive-op hard wall).
const APPLY_ECHO: &str = "apply";
/// The typed-confirm echo an infrastructure destroy must match.
const DESTROY_ECHO: &str = "destroy";

/// How often the folded `state/cloud` mirror is re-read while the surface is in
/// view (a cheap bounded per-topic index probe).
const REFRESH: Duration = Duration::from_secs(15);

/// How long an emitted `action/cloud/*` request waits for its reply before it
/// reads as unanswered — an honest "the cloud backend didn't respond" (§7),
/// distinct from the worker's own gated/failed replies.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

/// The in-view repaint heartbeat that keeps the poll cadence alive.
const POLL_REPAINT: Duration = Duration::from_secs(1);

/// The most session-audit rows retained (the workspace's own record of the ops
/// it requested this session — the newest are kept).
const MAX_AUDIT: usize = 24;

// ─────────────────────────────── the six modes ──────────────────────────────

/// Which mode of the workspace is showing — the six cloud-ops surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Mode {
    /// Compose + run OpenTofu: the resource roster + the plan/apply gate.
    #[default]
    Provision,
    /// Run Ansible: pick a playbook + group and converge.
    Configure,
    /// bootc / osbuild image builds (honest backend-pending).
    Images,
    /// libvirt networks (real backend health; list/compose backend-pending).
    Network,
    /// Podman / Quadlet workloads (honest backend-pending).
    Containers,
    /// Day-2: per-tool backend health, roster, degraded states, audit trail.
    Status,
}

impl Mode {
    /// Every mode, in tab order.
    const ALL: [Self; 6] = [
        Self::Provision,
        Self::Configure,
        Self::Images,
        Self::Network,
        Self::Containers,
        Self::Status,
    ];

    /// The tab label.
    const fn label(self) -> &'static str {
        match self {
            Self::Provision => "Provision",
            Self::Configure => "Configure",
            Self::Images => "Images",
            Self::Network => "Network",
            Self::Containers => "Containers",
            Self::Status => "Status",
        }
    }

    /// The Mackes-Carbon glyph name this mode's tab wears (§4 — a registered
    /// symbolic icon, never a text glyph).
    const fn icon(self) -> &'static str {
        match self {
            Self::Provision => "view-grid",
            Self::Configure => "document-edit",
            Self::Images => "camera-photo",
            Self::Network => "globe",
            Self::Containers => "overlay",
            Self::Status => "emblem-ok",
        }
    }
}

// ─────────────────────────────── the Bus reply ──────────────────────────────

/// The shell-side mirror of the worker's `CloudReply` for an `action/cloud/*`
/// mutation (§6 — the shell reads the JSON boundary without depending on the
/// daemon crate). Only the fields this workspace folds are named; the honest
/// tri-state is `ok` (applied) / `gated` (staged, nothing applied) / `error`
/// (failed).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CloudReply {
    /// `true` when a live mutation was performed; `false` on stage/failure.
    ok: bool,
    /// The verb this reply answers (echoed for the client's dispatch).
    verb: String,
    /// An honest gate reason — for a mutation this carries the staged
    /// `tofu plan` / `--check` summary (nothing was applied).
    gated: Option<String>,
    /// A rejection or a backend seam failure.
    error: Option<String>,
    /// Whether a destructive op (destroy / delete / reboot) was performed +
    /// audited on the events plane.
    audited: bool,
}

/// One in-flight `action/cloud/*` request awaiting its `reply/<ulid>`.
#[derive(Debug, Clone)]
struct Pending {
    /// The request ULID — the correlation key its reply rides.
    ulid: String,
    /// When the request was published (drives [`REQUEST_TIMEOUT`]).
    sent: Instant,
}

// ─────────────────────────────── typed arming ───────────────────────────────

/// What a confirmed typed-arming echo releases onto the Bus (RUN-006 idiom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArmAction {
    /// A live `provision` (OpenTofu apply) — echo [`APPLY_ECHO`].
    Provision,
    /// A live `configure` (Ansible apply) — echo [`APPLY_ECHO`].
    Configure,
    /// An infrastructure `destroy` — echo [`DESTROY_ECHO`].
    Destroy,
    /// A destructive per-instance lifecycle op (`instance-reboot` /
    /// `instance-delete`) — echo the instance name.
    Lifecycle {
        /// The lifecycle verb.
        verb: &'static str,
        /// The target instance id.
        instance_id: String,
        /// The instance's display name — the required echo.
        name: String,
    },
}

impl ArmAction {
    /// The `action/cloud/*` verb this action publishes.
    const fn verb(&self) -> &'static str {
        match self {
            Self::Provision => "provision",
            Self::Configure => "configure",
            Self::Destroy => "destroy",
            Self::Lifecycle { verb, .. } => verb,
        }
    }

    /// The exact echo the operator must type before this action publishes.
    fn echo(&self) -> String {
        match self {
            Self::Provision | Self::Configure => APPLY_ECHO.to_string(),
            Self::Destroy => DESTROY_ECHO.to_string(),
            Self::Lifecycle { name, .. } => name.clone(),
        }
    }

    /// The confirm button's verb word.
    fn confirm_word(&self) -> &'static str {
        match self {
            Self::Provision | Self::Configure => "Apply",
            Self::Destroy => "Destroy",
            Self::Lifecycle { verb, .. } => verb_label(verb),
        }
    }

    /// What the confirm acts on — the arming copy's subject.
    fn subject(&self) -> String {
        match self {
            Self::Provision => "the OpenTofu-managed infrastructure (live apply)".to_string(),
            Self::Configure => "the Ansible convergence (live apply)".to_string(),
            Self::Destroy => "ALL OpenTofu-managed infrastructure".to_string(),
            Self::Lifecycle { name, .. } => format!("instance {name}"),
        }
    }
}

/// A pending typed-arming confirm — the action it releases + the operator's echo
/// so far. Nothing reaches the Bus until [`armed`] returns true.
#[derive(Debug, Clone)]
pub(super) struct Arming {
    /// What confirming publishes.
    pub(super) action: ArmAction,
    /// The operator's typed echo.
    pub(super) typed: String,
}

/// The typed-arming gate (RUN-006): the operator's echo, trimmed, must equal the
/// required echo exactly before the mutation may publish. The one decision the
/// confirm button + the tests share, so "unconfirmed ⇒ blocked" is proven
/// without a render.
fn armed(typed: &str, echo: &str) -> bool {
    typed.trim() == echo
}

// ─────────────────────────────── the audit trail ────────────────────────────

/// The honest outcome class of a performed op — the session audit row's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditOutcome {
    /// The op was applied live (and, if destructive, audited to the events plane).
    Applied,
    /// The op was staged (a `tofu plan` / `--check` dry-run — nothing applied).
    Staged,
    /// The op failed.
    Failed,
}

impl AuditOutcome {
    /// The Style token this verdict paints in (§4).
    const fn color(self) -> Color32 {
        match self {
            Self::Applied => Style::OK,
            Self::Staged => Style::WARN,
            Self::Failed => Style::DANGER,
        }
    }

    /// The verdict word.
    const fn word(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Staged => "staged",
            Self::Failed => "failed",
        }
    }
}

/// One row of the session audit trail — the workspace's own honest record of an
/// op it requested (verb · verdict · detail). Distinct from the daemon's durable
/// hash-chained events log; this is the local "what did I do here" list.
#[derive(Debug, Clone)]
struct AuditEntry {
    /// The verb performed (`provision` / `instance-delete` / …).
    verb: String,
    /// The honest verdict.
    outcome: AuditOutcome,
    /// A short detail (the staged plan summary / the failure / "audited").
    detail: String,
}

// ─────────────────────────────── the surface state ──────────────────────────

/// The **Infra as Code** workspace state — the folded `state/cloud` mirror, the
/// active mode, the Configure inputs, the typed-arming confirm, the one in-flight
/// mutation, and the session audit trail. A plain field on the shell's struct,
/// borrowed `&mut` while the surface is in view.
pub struct InfraCodeState {
    /// The Bus persist root (the client data dir). `None` when the Bus is
    /// unavailable — an honest off-mesh degrade (§7), never a crash.
    bus_root: Option<PathBuf>,
    /// Which mode is showing.
    mode: Mode,
    /// The per-node status mirrors, folded across every `state/cloud/<node>`
    /// topic (host-sorted). Empty when nothing has published yet — an honest
    /// pre-mirror state, never fabricated.
    states: Vec<CloudState>,
    /// When the mirror was last folded (the refresh cadence anchor).
    loaded_at: Option<Instant>,
    /// A manual refresh is queued — re-reads the mirror on the next poll.
    forced: bool,
    /// The Configure mode's playbook selection (the Ansible entrypoint).
    configure_playbook: String,
    /// The Configure mode's target group (the mesh inventory group to converge).
    configure_group: String,
    /// A pending typed-arming confirm for a destructive intent, if any.
    arming: Option<Arming>,
    /// The one in-flight mutation — its reply resolves into the note + audit.
    mutation_pending: Option<Pending>,
    /// A transient one-line action note — honest feedback, never a silent op.
    note: Option<String>,
    /// The session audit trail (newest last), capped at [`MAX_AUDIT`].
    audit: Vec<AuditEntry>,
}

impl Default for InfraCodeState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            mode: Mode::default(),
            states: Vec::new(),
            loaded_at: None,
            forced: false,
            configure_playbook: "site.yml".to_string(),
            configure_group: "cloud_vm".to_string(),
            arming: None,
            mutation_pending: None,
            note: None,
            audit: Vec::new(),
        }
    }
}

impl InfraCodeState {
    /// Poll the Bus on the shared cadence + keep the repaint heartbeat alive —
    /// the shell calls this each frame while the surface is in view. Resolves any
    /// in-flight mutation reply, then re-folds the `state/cloud` mirror when due
    /// (the refresh cadence or a queued refresh). No blocking await — the mirror
    /// is a cheap local read and the reply is read off the Bus on a later tick.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.resolve_mutation();

        let due = self
            .loaded_at
            .is_none_or(|t| now.duration_since(t) >= REFRESH);
        if self.forced || due {
            self.states = self.read_states();
            self.loaded_at = Some(now);
            self.forced = false;
        }

        ctx.request_repaint_after(POLL_REPAINT);
    }

    /// Resolve the one in-flight mutation's reply into the note + an audit row
    /// (never a silent op, §7); on a live apply, re-fold the mirror so the change
    /// reflects. A no-responder is an honest timeout.
    fn resolve_mutation(&mut self) {
        let Some((ulid, sent)) = self
            .mutation_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent))
        else {
            return;
        };
        if let Some(reply) = self.read_reply(&ulid) {
            let (note, entry) = fold_mutation(&reply);
            self.record_audit(entry);
            if reply.ok {
                self.forced = true;
            }
            self.note = Some(note);
            self.mutation_pending = None;
        } else if sent.elapsed() >= REQUEST_TIMEOUT {
            self.note = Some(
                "The cloud backend did not answer the request — it may not be running on any \
                 reachable node."
                    .to_string(),
            );
            self.mutation_pending = None;
        }
    }

    /// Fold every `state/cloud/<node>` mirror off the Bus into the host-sorted
    /// roster (all nodes). A missing/unopenable Bus is an honest empty fold
    /// (off-mesh, §7); an undecodable body is skipped, never fabricated.
    fn read_states(&self) -> Vec<CloudState> {
        let Some(persist) = self.persist() else {
            return Vec::new();
        };
        let Ok(topics) = persist.list_topics() else {
            return Vec::new();
        };
        let mut states: Vec<CloudState> = topics
            .into_iter()
            .filter(|t| t.starts_with(CLOUD_STATE_PREFIX))
            .filter_map(|topic| {
                let msg = persist.read_latest(&topic).ok().flatten()?;
                let body = msg.body.as_deref()?;
                serde_json::from_str::<CloudState>(body).ok()
            })
            .collect();
        states.sort_by(|a, b| a.host.cmp(&b.host));
        states
    }

    /// Read the reply on `reply/<ulid>`, if one has landed (oldest wins — the RPC
    /// convention).
    fn read_reply(&self, ulid: &str) -> Option<CloudReply> {
        let persist = self.persist()?;
        let msgs = persist.list_since(&reply_topic(ulid), None).ok()?;
        let body = msgs.first()?.body.as_deref()?;
        serde_json::from_str::<CloudReply>(body).ok()
    }

    /// Open the Bus persist mirror at the client data dir, if reachable
    /// (fail-soft, through the shared [`BusReader`] seam).
    fn persist(&self) -> Option<Persist> {
        BusReader::new(self.bus_root.clone()).open()
    }

    /// Publish an `action/cloud/<verb>` request, answering a pending handle or an
    /// honest error string (a missing Bus degrades, never panics — §7).
    fn publish(&self, verb: &str, body: Option<&str>) -> Result<Pending, String> {
        let persist = self
            .persist()
            .ok_or_else(|| "the local mesh Bus is unavailable".to_string())?;
        let topic = format!("{}{verb}", mackes_mesh_types::cloud::CLOUD_ACTION_PREFIX);
        publish_request(&persist, &topic, Priority::Default, None, body)
            .map(|ulid| Pending {
                ulid,
                sent: Instant::now(),
            })
            .map_err(|e| e.to_string())
    }

    /// Emit a mutation verb and track its reply — the honest outcome lands in the
    /// note. A newly-issued mutation replaces an unresolved one (its reply is
    /// simply never read).
    fn issue(&mut self, verb: &str, body: Option<&str>, label: &str) {
        match self.publish(verb, body) {
            Ok(pending) => {
                self.mutation_pending = Some(pending);
                self.note = Some(format!("Requested {label}\u{2026}"));
            }
            Err(e) => self.note = Some(format!("Could not request {label}: {e}")),
        }
    }

    /// Record one session-audit row, trimming to [`MAX_AUDIT`] newest.
    fn record_audit(&mut self, entry: AuditEntry) {
        self.audit.push(entry);
        let overflow = self.audit.len().saturating_sub(MAX_AUDIT);
        if overflow > 0 {
            self.audit.drain(0..overflow);
        }
    }

    /// The Configure mode's request body — the picked playbook + target group.
    /// (The worker converges `cloud_vm` on `site.yml`; the selection is honest
    /// operator context the reply echoes.)
    fn configure_body(&self) -> String {
        serde_json::json!({
            "playbook": self.configure_playbook.trim(),
            "group": self.configure_group.trim(),
        })
        .to_string()
    }

    /// Perform a confirmed armed action — called only past the typed-arming gate
    /// ([`armed`]).
    fn perform(&mut self, action: ArmAction) {
        match action {
            ArmAction::Provision => self.issue("provision", None, "live provision (apply)"),
            ArmAction::Configure => {
                let body = self.configure_body();
                self.issue("configure", Some(&body), "live configuration (apply)");
            }
            ArmAction::Destroy => self.issue("destroy", None, "infrastructure destroy"),
            ArmAction::Lifecycle {
                verb,
                instance_id,
                name,
            } => {
                let body = serde_json::json!({ "instance": instance_id }).to_string();
                self.issue(
                    verb,
                    Some(&body),
                    &format!("{} on {name}", verb_label(verb)),
                );
            }
        }
    }

    // ── the plan/apply gate seams (§6, shared by the body + the menubar) ──

    /// Emit a provision **plan** (dry-run) — direct, no confirm. On a plan-only
    /// node (the default, live apply operator-gated) the worker stages a
    /// `tofu plan` and returns it in the reply.
    pub(super) fn plan_provision(&mut self) {
        self.issue("provision", None, "provision plan (dry-run)");
    }

    /// Open the typed-arming confirm for a live provision **apply** (#RUN-006 —
    /// nothing publishes until the echo matches).
    pub(super) fn arm_provision(&mut self) {
        self.arming = Some(Arming {
            action: ArmAction::Provision,
            typed: String::new(),
        });
    }

    /// Emit a configuration **check** (dry-run `--check`) — direct.
    pub(super) fn check_configure(&mut self) {
        let body = self.configure_body();
        self.issue("configure", Some(&body), "configuration check (dry-run)");
    }

    /// Open the typed-arming confirm for a live configuration **apply**.
    pub(super) fn arm_configure(&mut self) {
        self.arming = Some(Arming {
            action: ArmAction::Configure,
            typed: String::new(),
        });
    }

    /// Open the typed-arming confirm for an infrastructure **destroy**.
    pub(super) fn arm_destroy(&mut self) {
        self.arming = Some(Arming {
            action: ArmAction::Destroy,
            typed: String::new(),
        });
    }

    /// Emit a non-destructive lifecycle op (`instance-start` / `instance-stop`)
    /// directly — armed to the backend, no typed confirm (never destructive).
    fn issue_lifecycle_direct(&mut self, verb: &'static str, instance_id: &str, name: &str) {
        let body = serde_json::json!({ "instance": instance_id }).to_string();
        self.issue(
            verb,
            Some(&body),
            &format!("{} on {name}", verb_label(verb)),
        );
    }

    /// Open the typed-arming confirm for a destructive lifecycle op
    /// (`instance-reboot` / `instance-delete`) — nothing publishes until the
    /// instance name is typed (RUN-006).
    fn arm_lifecycle(&mut self, verb: &'static str, instance_id: &str, name: &str) {
        self.arming = Some(Arming {
            action: ArmAction::Lifecycle {
                verb,
                instance_id: instance_id.to_string(),
                name: name.to_string(),
            },
            typed: String::new(),
        });
    }

    // ── the menubar seam (§6, one dispatch path shared with the body) ──

    /// The folded per-node `state/cloud` mirror (the menubar status cluster reads
    /// the same fold the body renders — no second read, §7).
    pub(super) fn states(&self) -> &[CloudState] {
        &self.states
    }

    /// Which mode is showing.
    pub(super) fn mode(&self) -> Mode {
        self.mode
    }

    /// Switch the active mode.
    pub(super) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Queue an immediate re-fold of the `state/cloud` mirror.
    pub(super) fn request_refresh(&mut self) {
        self.forced = true;
    }

    /// Surface the honest apply-gate + audit posture in the action note (Help).
    pub(super) fn set_help_note(&mut self) {
        self.note = Some(
            "Live apply is operator-gated per node (MDE_CLOUD_APPLY=1); provision, configure, and \
             destroy stage as dry-runs otherwise. Every destructive op passes a typed-confirm; \
             performed ops land in the Status audit trail."
                .to_string(),
        );
    }

    /// Whether a typed-arming confirm is open (test seam).
    pub(super) fn has_arming(&self) -> bool {
        self.arming.is_some()
    }

    /// The current action note text, if any (test seam).
    pub(super) fn note_text(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// Fold a settled mutation reply into `(honest note, audit row)` (§7 — the pure
/// seam shared by the poll path and the tests). `ok` reads applied; a `gated`
/// reply reads staged (a dry-run — nothing applied) carrying the plan summary;
/// an error reads failed.
fn fold_mutation(reply: &CloudReply) -> (String, AuditEntry) {
    let verb = if reply.verb.is_empty() {
        "cloud op".to_string()
    } else {
        reply.verb.clone()
    };
    if reply.ok {
        let audited = if reply.audited { " (audited)" } else { "" };
        (
            format!("{verb} applied{audited}."),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Applied,
                detail: if reply.audited {
                    "audited to the events plane".to_string()
                } else {
                    "applied".to_string()
                },
            },
        )
    } else if let Some(gated) = &reply.gated {
        (
            format!("{verb} staged (dry-run): {gated}"),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Staged,
                detail: gated.clone(),
            },
        )
    } else {
        let error = reply
            .error
            .clone()
            .unwrap_or_else(|| "unknown error".to_string());
        (
            format!("{verb} failed: {error}"),
            AuditEntry {
                verb,
                outcome: AuditOutcome::Failed,
                detail: error,
            },
        )
    }
}

/// The button/label word for a lifecycle (or mutation) verb.
fn verb_label(verb: &str) -> &'static str {
    match verb {
        "instance-start" => "Start",
        "instance-stop" => "Stop",
        "instance-reboot" => "Reboot",
        "instance-delete" => "Delete",
        "provision" => "Provision",
        "configure" => "Configure",
        "destroy" => "Destroy",
        _ => "Run",
    }
}

// ─────────────────────────── honest read helpers (pure) ──────────────────────

/// The Mackes-Carbon glyph a health state paints (§4 — a registered symbolic
/// icon, never a text dot).
const fn health_icon(state: HealthState) -> &'static str {
    match state {
        HealthState::Up => "emblem-ok",
        HealthState::Down => "dialog-warning",
        HealthState::Absent => "changes-prevent",
    }
}

/// The semantic tone a health state reads in.
const fn health_tone(state: HealthState) -> ChipTone {
    match state {
        HealthState::Up => ChipTone::Ok,
        HealthState::Down => ChipTone::Danger,
        HealthState::Absent => ChipTone::Warn,
    }
}

/// The hosts where live apply is armed (`apply_armed`) — the operator's honest
/// "these nodes execute for real" set.
fn armed_hosts(states: &[CloudState]) -> Vec<String> {
    states
        .iter()
        .filter(|s| s.apply_armed)
        .map(|s| s.host.clone())
        .collect()
}

/// Whether a resource table is the instance roster (the lifecycle verbs act on
/// its rows). The worker publishes the roster as `compute`/`instances`.
fn is_instance_roster(table: &ResourceTable) -> bool {
    table.collection == "instances" || table.service_type == "compute"
}

// ───────────────────────────────── the render ───────────────────────────────

/// Render the Infra-as-Code workspace into `ui`: the shared MENUBAR-ALL bar, the
/// six-mode tab strip, the typed-arming confirm + action note, then the active
/// mode's body.
pub fn infra_code_panel(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    if let Some(action) = menubar::show(ui, state) {
        menubar::apply(state, action);
    }
    ui.separator();
    ui.add_space(Style::SP_XS);

    mode_bar(ui, state);
    ui.add_space(Style::SP_S);

    render_arming(ui, state);
    render_note(ui, state);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match state.mode {
            Mode::Provision => render_provision(ui, state),
            Mode::Configure => render_configure(ui, state),
            Mode::Images => render_images(ui),
            Mode::Network => render_network(ui, state),
            Mode::Containers => render_containers(ui),
            Mode::Status => render_status(ui, state),
        });
}

/// The six-mode tab strip — each a Carbon icon + label, tinted the Workloads
/// accent when active (§4).
fn mode_bar(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_M;
        for mode in Mode::ALL {
            if mode_tab(ui, state.mode == mode, mode.icon(), mode.label()).clicked() {
                state.mode = mode;
            }
        }
    });
}

/// One mode tab — a clickable icon + label row. The icon is drawn through
/// [`carbon_icon`] with the tab's accent as the current text color, so the glyph
/// glows Workloads-purple when active.
fn mode_tab(ui: &mut egui::Ui, selected: bool, icon: &str, label: &str) -> egui::Response {
    let color = if selected {
        Style::ACCENT_WORKLOADS
    } else {
        Style::TEXT_DIM
    };
    let resp = ui
        .horizontal(|ui| {
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(color);
                carbon_icon(ui, icon, Style::BODY + 2.0);
            });
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new(label).size(Style::BODY).color(color).strong());
        })
        .response
        .interact(Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A framed icon + label button (Carbon icon tinted `accent`) — the workspace's
/// primary action affordance.
fn icon_button(ui: &mut egui::Ui, icon: &str, label: &str, accent: Color32) -> egui::Response {
    let resp = egui::Frame::NONE
        .fill(Style::SURFACE_HI)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS_S)
        .inner_margin(egui::Margin::symmetric(
            Style::SP_S as i8,
            Style::SP_XS as i8,
        ))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    ui.visuals_mut().override_text_color = Some(accent);
                    carbon_icon(ui, icon, Style::SMALL + 2.0);
                });
                ui.add_space(Style::SP_XS);
                ui.label(RichText::new(label).size(Style::SMALL).color(Style::TEXT));
            });
        })
        .response
        .interact(Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A small text button sized for a table row's inline lifecycle verb.
fn row_button(ui: &mut egui::Ui, label: &str, danger: bool) -> egui::Response {
    let color = if danger { Style::DANGER } else { Style::TEXT };
    ui.add(egui::Button::new(
        RichText::new(label).size(Style::SMALL).color(color),
    ))
}

/// The transient one-line action note (last issued op / its outcome) with a
/// dismiss affordance — honest feedback, never a silent op.
fn render_note(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    let Some(note) = state.note.clone() else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(Style::ACCENT, RichText::new(note).size(Style::SMALL));
        if ui.small_button("dismiss").clicked() {
            state.note = None;
        }
    });
    ui.add_space(Style::SP_XS);
}

/// The pending typed-arming confirm (RUN-006) — the operator types the required
/// echo; the confirm button is disabled (never omitted) until it matches, then
/// releases the action. Cancel clears it. Nothing reaches the Bus until armed.
fn render_arming(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    let Some(arming) = state.arming.as_mut() else {
        return;
    };
    let echo = arming.action.echo();
    let word = arming.action.confirm_word();
    let subject = arming.action.subject();
    let mut confirm = false;
    let mut cancel = false;

    egui::Frame::group(ui.style())
        .fill(Style::SURFACE_HI)
        .stroke(egui::Stroke::new(1.0, Style::DANGER))
        .corner_radius(Style::RADIUS_S)
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "Arming — type \u{201C}{echo}\u{201D} exactly to {} {subject}. Nothing is \
                     sent until it matches.",
                    word.to_lowercase()
                ))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut arming.typed)
                        .hint_text(echo.as_str())
                        .desired_width(Style::SP_XL * 5.0),
                );
                ui.add_space(Style::SP_S);
                let is_armed = armed(&arming.typed, &echo);
                if ui
                    .add_enabled(
                        is_armed,
                        egui::Button::new(
                            RichText::new(word).size(Style::SMALL).color(Style::DANGER),
                        ),
                    )
                    .clicked()
                {
                    confirm = true;
                }
                if ui
                    .add(egui::Button::new(
                        RichText::new("Cancel").size(Style::SMALL),
                    ))
                    .clicked()
                {
                    cancel = true;
                }
            });
        });
    ui.add_space(Style::SP_S);

    if confirm {
        if let Some(arming) = state.arming.take() {
            state.perform(arming.action);
        }
    } else if cancel {
        state.arming = None;
    }
}

/// The apply-posture banner — plan-only vs. live, folded straight from each
/// node's `apply_armed` (the mirror's honest plan/apply signal).
fn render_apply_posture(ui: &mut egui::Ui, states: &[CloudState]) {
    let armed = armed_hosts(states);
    if armed.is_empty() {
        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(Style::OK);
                carbon_icon(ui, "changes-prevent", Style::SMALL + 2.0);
            });
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::OK,
                RichText::new(
                    "Plan-only — live apply is operator-gated (MDE_CLOUD_APPLY=1) on every node; \
                     provision, configure, and destroy stage as dry-runs.",
                )
                .size(Style::SMALL),
            );
        });
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(Style::DANGER);
                carbon_icon(ui, "dialog-warning", Style::SMALL + 2.0);
            });
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::DANGER,
                RichText::new(format!(
                    "LIVE — apply is armed on {}. Provision, configure, and destroy execute for \
                     real there.",
                    armed.join(", ")
                ))
                .size(Style::SMALL),
            );
        });
    }
    ui.add_space(Style::SP_S);
}

/// A mode heading — the icon + title + one-line blurb.
fn mode_heading(ui: &mut egui::Ui, icon: &str, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, icon, Style::BODY + 2.0);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(title)
                .size(Style::BODY)
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
    });
    mde_egui::muted_note(ui, blurb);
    ui.add_space(Style::SP_S);
}

/// The **Provision** mode — the OpenTofu resource roster (from the mirror) + the
/// plan/apply gate + the per-instance lifecycle verbs.
fn render_provision(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    let states = state.states.clone();
    mode_heading(
        ui,
        Mode::Provision.icon(),
        "Provision \u{2014} OpenTofu",
        "Compose + run OpenTofu against local libvirt. The roster below is the live \
         state/cloud mirror; the worker owns tofu plan/apply.",
    );
    render_apply_posture(ui, &states);

    // The plan/apply gate: Plan is the dry-run default (direct); Apply + Destroy
    // pass a typed-confirm echo first (RUN-006).
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_S;
        if icon_button(ui, "view-refresh", "Plan (dry-run)", Style::ACCENT).clicked() {
            state.plan_provision();
        }
        if icon_button(ui, "list-add", "Apply infrastructure\u{2026}", Style::OK).clicked() {
            state.arm_provision();
        }
        if icon_button(
            ui,
            "process-stop",
            "Destroy infrastructure\u{2026}",
            Style::DANGER,
        )
        .clicked()
        {
            state.arm_destroy();
        }
    });
    ui.add_space(Style::SP_M);

    if states.is_empty() {
        crate::session::empty_state(
            ui,
            "No cloud mirror yet",
            "The OpenTofu + Ansible worker publishes state/cloud/<node> from each node. This roster \
             fills once a node answers.",
        );
        return;
    }

    for st in &states {
        render_node_roster(ui, st, state);
    }
}

/// One node's resource roster — its header + each published resource table, with
/// the instance roster carrying the inline lifecycle verbs.
fn render_node_roster(ui: &mut egui::Ui, st: &CloudState, state: &mut InfraCodeState) {
    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
            node_header(ui, st);
            if st.resources.is_empty() {
                mde_egui::muted_note(ui, "No resource roster reported by this node yet.");
                return;
            }
            for table in &st.resources {
                render_table(ui, st, table, state);
            }
        });
    ui.add_space(Style::SP_S);
}

/// A node card's header — host, backend adapter, and apply posture chip.
fn node_header(ui: &mut egui::Ui, st: &CloudState) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&st.host)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(adapter_label(st.adapter)).size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let (word, tone) = if st.apply_armed {
            ("live-armed", ChipTone::Danger)
        } else {
            ("plan-only", ChipTone::Ok)
        };
        ui.colored_label(tone.color(), RichText::new(word).size(Style::SMALL));
    });
    ui.add_space(Style::SP_XS);
}

/// Render one resource table as a grid; when it is the instance roster, each row
/// carries inline Start / Stop / Reboot… / Delete… verbs (destructive ones armed).
fn render_table(
    ui: &mut egui::Ui,
    st: &CloudState,
    table: &ResourceTable,
    state: &mut InfraCodeState,
) {
    let roster = is_instance_roster(table);
    ui.label(
        RichText::new(format!(
            "{} \u{00B7} {} ({} row{})",
            table.service_type,
            table.collection,
            table.rows.len(),
            if table.rows.len() == 1 { "" } else { "s" }
        ))
        .size(Style::SMALL)
        .color(Style::TEXT_DIM),
    );
    if table.rows.is_empty() {
        mde_egui::muted_note(ui, "No resources of this kind (an honest empty roster).");
        return;
    }

    egui::Grid::new((
        "iac-table",
        &st.host,
        &table.service_type,
        &table.collection,
    ))
    .striped(true)
    .spacing(egui::vec2(Style::SP_M, Style::SP_XS))
    .show(ui, |ui| {
        for col in &table.columns {
            ui.label(
                RichText::new(col)
                    .size(Style::SMALL)
                    .strong()
                    .color(Style::TEXT_DIM),
            );
        }
        if roster {
            ui.label(
                RichText::new("actions")
                    .size(Style::SMALL)
                    .strong()
                    .color(Style::TEXT_DIM),
            );
        }
        ui.end_row();

        for row in &table.rows {
            for cell in &row.cells {
                ui.label(RichText::new(cell).size(Style::SMALL).color(Style::TEXT));
            }
            if roster {
                let id = row.id.clone();
                let name = table.row_label(row).to_string();
                ui.horizontal(|ui| {
                    if row_button(ui, "Start", false).clicked() {
                        state.issue_lifecycle_direct("instance-start", &id, &name);
                    }
                    if row_button(ui, "Stop", false).clicked() {
                        state.issue_lifecycle_direct("instance-stop", &id, &name);
                    }
                    if row_button(ui, "Reboot\u{2026}", true).clicked() {
                        state.arm_lifecycle("instance-reboot", &id, &name);
                    }
                    if row_button(ui, "Delete\u{2026}", true).clicked() {
                        state.arm_lifecycle("instance-delete", &id, &name);
                    }
                });
            }
            ui.end_row();
        }
    });
    ui.add_space(Style::SP_XS);
}

/// The **Configure** mode — pick a playbook + target group and run Ansible; the
/// check is the dry-run default, the apply is typed-confirm gated.
fn render_configure(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    mode_heading(
        ui,
        Mode::Configure.icon(),
        "Configure \u{2014} Ansible",
        "Converge the mesh inventory with Ansible. The backend converges cloud_vm on site.yml; \
         pick the playbook + group below.",
    );

    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Playbook")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut state.configure_playbook)
                        .hint_text("site.yml")
                        .desired_width(Style::SP_XL * 5.0),
                );
                ui.add_space(Style::SP_M);
                ui.label(
                    RichText::new("Group")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                egui::ComboBox::from_id_salt("iac-configure-group")
                    .selected_text(state.configure_group.clone())
                    .show_ui(ui, |ui| {
                        for group in ["cloud_vm", "all", "compute", "network"] {
                            ui.selectable_value(
                                &mut state.configure_group,
                                group.to_string(),
                                group,
                            );
                        }
                    });
            });
        });
    ui.add_space(Style::SP_M);

    render_apply_posture(ui, &state.states.clone());

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_S;
        if icon_button(ui, "view-refresh", "Check (dry-run)", Style::ACCENT).clicked() {
            state.check_configure();
        }
        if icon_button(
            ui,
            "document-edit",
            "Apply configuration\u{2026}",
            Style::OK,
        )
        .clicked()
        {
            state.arm_configure();
        }
    });
    ui.add_space(Style::SP_M);
    mde_egui::muted_note(
        ui,
        "The run status appears in the action note above and in the Status mode's audit trail.",
    );
}

/// The **Images** mode — bootc / osbuild builds. No backend verb is landed yet,
/// so this renders an honest backend-pending note rather than faking a build
/// list (§7).
fn render_images(ui: &mut egui::Ui) {
    mode_heading(
        ui,
        Mode::Images.icon(),
        "Images \u{2014} bootc / osbuild",
        "Build + register base images with bootc / osbuild.",
    );
    render_backend_pending(
        ui,
        "Image builds",
        "The cloud backend does not yet expose an image build/list verb. Nothing is shown here \
         rather than fabricated data; this mode goes live when the worker lands the verb.",
    );
}

/// The **Network** mode — libvirt networks. The libvirt backend health is real
/// (from the mirror); the network list/compose verb is honestly backend-pending.
fn render_network(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    let states = state.states.clone();
    mode_heading(
        ui,
        Mode::Network.icon(),
        "Network \u{2014} libvirt",
        "libvirt (Nebula-adjacent) networks. The libvirt backend health below is live from the \
         mirror.",
    );

    if states.is_empty() {
        crate::session::empty_state(
            ui,
            "No cloud mirror yet",
            "libvirt health appears here once a node publishes its state/cloud mirror.",
        );
    } else {
        for st in &states {
            egui::Frame::group(ui.style())
                .shadow(card_shadow())
                .show(ui, |ui| {
                    node_header(ui, st);
                    render_tool_health(ui, st, TOOL_LIBVIRT, "libvirt");
                });
            ui.add_space(Style::SP_S);
        }
    }

    render_backend_pending(
        ui,
        "Network list + compose",
        "The cloud backend does not yet expose a libvirt-network list/compose verb. The network \
         roster and the compose form go live when the worker lands them; nothing is faked (§7).",
    );
}

/// The **Containers** mode — Podman / Quadlet workloads. A separate runtime plane
/// with no `action/cloud` verb yet, so this is honestly backend-pending.
fn render_containers(ui: &mut egui::Ui) {
    mode_heading(
        ui,
        Mode::Containers.icon(),
        "Containers \u{2014} Podman / Quadlet",
        "Podman / Quadlet service workloads.",
    );
    render_backend_pending(
        ui,
        "Container workloads",
        "Podman / Quadlet is a separate runtime plane; the cloud backend exposes no container \
         list/control verb here yet. This mode goes live when the worker lands it — no faked \
         workloads (§7).",
    );
}

/// The **Status** mode — day-2: per-tool backend health, the roster summary,
/// honest degraded states, and the session audit trail.
fn render_status(ui: &mut egui::Ui, state: &mut InfraCodeState) {
    let states = state.states.clone();
    mode_heading(
        ui,
        Mode::Status.icon(),
        "Status \u{2014} day-2",
        "Per-tool backend health, the live roster, honest degraded states, and the ops performed \
         from this workspace.",
    );

    if states.is_empty() {
        crate::session::empty_state(
            ui,
            "No cloud mirror yet",
            "Each node publishes state/cloud/<node> with per-tool health + its roster. This fills \
             once a node answers.",
        );
    } else {
        for st in &states {
            egui::Frame::group(ui.style())
                .shadow(card_shadow())
                .show(ui, |ui| {
                    node_header(ui, st);
                    let ready = st.backend_ready();
                    ui.colored_label(
                        if ready { Style::OK } else { Style::WARN },
                        RichText::new(if ready {
                            "backend ready"
                        } else {
                            "backend degraded"
                        })
                        .size(Style::SMALL),
                    );
                    ui.add_space(Style::SP_XS);
                    for (tool, label) in BACKEND_TOOLS {
                        render_tool_health(ui, st, tool, label);
                    }
                    let roster: usize = st.resources.iter().map(|t| t.rows.len()).sum();
                    ui.add_space(Style::SP_XS);
                    ui.colored_label(
                        Style::TEXT_DIM,
                        RichText::new(format!("{roster} resource row(s) in the roster"))
                            .size(Style::SMALL),
                    );
                });
            ui.add_space(Style::SP_S);
        }
    }

    render_audit(ui, &state.audit);
}

/// One tool's health row — a Carbon Up/Down/Absent glyph + label + the honest
/// detail (the "why" of a Down/Absent). An unprobed tool reads honestly.
fn render_tool_health(ui: &mut egui::Ui, st: &CloudState, tool: &str, label: &str) {
    ui.horizontal_wrapped(|ui| match st.tool_health(tool) {
        Some(h) => {
            let tone = health_tone(h.state);
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(tone.color());
                carbon_icon(ui, health_icon(h.state), Style::SMALL + 2.0);
            });
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                tone.color(),
                RichText::new(label).size(Style::SMALL).strong(),
            );
            let state_word = match h.state {
                HealthState::Up => "up",
                HealthState::Down => "down",
                HealthState::Absent => "absent",
            };
            ui.colored_label(tone.color(), RichText::new(state_word).size(Style::SMALL));
            render_health_detail(ui, h);
        }
        None => {
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(Style::TEXT_DIM);
                carbon_icon(ui, "changes-prevent", Style::SMALL + 2.0);
            });
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("{label} unprobed")).size(Style::SMALL),
            );
        }
    });
}

/// The honest "why" trailer for a health row — latency + the detail reason.
fn render_health_detail(ui: &mut egui::Ui, h: &ServiceHealth) {
    if let Some(ms) = h.latency_ms {
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(format!("{ms} ms")).size(Style::SMALL),
        );
    }
    if let Some(detail) = &h.detail {
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(format!("\u{2014} {detail}")).size(Style::SMALL),
        );
    }
}

/// The session audit trail — the workspace's honest record of the ops it
/// requested (verb · verdict · detail). An empty trail reads honestly.
fn render_audit(ui: &mut egui::Ui, audit: &[AuditEntry]) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Audit trail (this session)")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    if audit.is_empty() {
        mde_egui::muted_note(ui, "No ops performed from this workspace yet.");
        return;
    }
    for entry in audit.iter().rev() {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                entry.outcome.color(),
                RichText::new(format!("{} {}", entry.verb, entry.outcome.word()))
                    .size(Style::SMALL)
                    .strong(),
            );
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("\u{2014} {}", entry.detail)).size(Style::SMALL),
            );
        });
    }
}

/// An honest **backend pending** card (§7) — a warning glyph + the honest reason
/// a mode has no landed backend verb yet. Never a faked list, never a dead
/// button.
fn render_backend_pending(ui: &mut egui::Ui, what: &str, reason: &str) {
    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    ui.visuals_mut().override_text_color = Some(Style::WARN);
                    carbon_icon(ui, "dialog-warning", Style::BODY + 2.0);
                });
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::WARN,
                    RichText::new(format!("{what} \u{2014} backend pending"))
                        .size(Style::BODY)
                        .strong(),
                );
            });
            mde_egui::muted_note(ui, reason);
        });
    ui.add_space(Style::SP_S);
}

/// The display label for a backend adapter (provider-neutral; the compatibility
/// backend is honestly labeled, never product-default).
fn adapter_label(adapter: CloudProviderAdapter) -> &'static str {
    adapter.label()
}

/// The shared **Raised** depth token the workspace's cards cast (Phase-C depth
/// adoption): every field comes straight from the token (offset/blur/spread + the
/// translucent umbra colour, no minted `Color32`, §4), so the cards read as
/// genuinely lifted.
fn card_shadow() -> egui::Shadow {
    let token = mde_egui::style::Elevation::Raised.shadow();
    egui::Shadow {
        offset: [token.offset[0] as i8, token.offset[1] as i8],
        blur: token.blur as u8,
        spread: token.spread as u8,
        color: token.umbra,
    }
}

// ─────────────────────────── the MENUBAR-ALL bar ────────────────────────────

mod menubar;

#[cfg(test)]
#[allow(clippy::panic)]
mod tests;
