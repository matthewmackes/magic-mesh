//! The **Workloads** cockpit (WL-ARCH-006) — the single workspace for every
//! delivery-type workload on the local-first **OpenTofu + Ansible + libvirt +
//! Podman** backend (WL-ARCH-001).
//!
//! The surface is organized around the plan's central metaphor: **five
//! delivery-type views**, each placeable on an explicit mesh node, over the
//! Tofu/Ansible/libvirt/Podman substrate. The primary axis is *what a workload
//! delivers* ([`DeliveryView`]); the secondary axis is *which lens* you view it
//! through ([`Panel`] — the roster, or one of the provision / configure / status
//! / images / containers panels).
//!
//! ## Layout (the U3 seam)
//!
//! This module owns the durable seam the six panel workers (U14–U19) plug into:
//! the nav, the folded `state/cloud` mirror, the typed-arming + audit backend
//! wiring, and the dispatch to each panel's own render fn. Each panel lives in
//! its own file and owns its own `State` sub-struct, so a downstream worker adds
//! panel-specific state + rendering in THEIR file and never edits this one.
//!
//! ## How the cloud is consumed (§6)
//!
//! The shell never depends on `mackesd`. It **reads** the per-node status mirror
//! `state/cloud/<node>` ([`CloudState`], folded across every node — now carrying
//! per-workload rows / drift / capacity) off the Bus, and **emits**
//! `action/cloud/*` verbs as typed request/reply (the reply lands on
//! `reply/<request-ulid>`). Only the mesh-neutral [`mackes_mesh_types::cloud`]
//! shapes are shared; the worker owns the actual `tofu` / `ansible-playbook` /
//! `virsh` / `podman` execution. The surface never shells a tool itself.
//!
//! Every state is honest (§7): an off-mesh Bus is a silent degrade, an empty
//! roster is a real "no workloads", and a panel with no landed backend render
//! draws an honest **not yet built** stub rather than fake data. Every
//! destructive intent passes a typed-confirm echo first (RUN-006), and every
//! performed op lands in the session audit trail.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;

use mde_egui::egui::{self, Color32, RichText, Sense};
use mde_egui::{carbon_icon, Style};

use mackes_mesh_types::cloud::{CloudState, DeliveryType, WorkloadRow, CLOUD_STATE_PREFIX};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

use crate::bus_reader::BusReader;

/// User-facing cloud product name. The backend toolchain (OpenTofu / Ansible /
/// libvirt / Podman) stays behind the Bus + shared payload contracts, so a later
/// backend can satisfy the same UI seam.
pub(super) const CLOUD_PRODUCT_LABEL: &str = "Construct Cloud";

/// The workspace title the MENUBAR-ALL bar wears.
pub(super) const WORKSPACE_TITLE: &str = "Workloads";

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

// ───────────────────────────── the delivery-type axis ───────────────────────

/// Which delivery-type view is showing — the cockpit's primary organizing axis
/// (delivery type × placement). Mirrors [`DeliveryType`] on the UI side, adding
/// the nav label + Mackes-Carbon glyph each view tab wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum DeliveryView {
    /// A full VM desktop delivered as a native VDI seat.
    #[default]
    DesktopVm,
    /// A headless VM running a service exposed on the mesh.
    ServiceVm,
    /// A VM whose individual apps are forwarded into the MDE desktop.
    AppVm,
    /// A VM providing Android via the two-layer Cuttlefish backend.
    AndroidVm,
    /// A Podman / Quadlet service container.
    ServiceContainer,
}

impl DeliveryView {
    /// Every delivery view, in tab order.
    pub(super) const ALL: [Self; 5] = [
        Self::DesktopVm,
        Self::ServiceVm,
        Self::AppVm,
        Self::AndroidVm,
        Self::ServiceContainer,
    ];

    /// The wire delivery type this view renders — the key the roster filters the
    /// mirror's `workloads` on.
    pub(super) const fn delivery_type(self) -> DeliveryType {
        match self {
            Self::DesktopVm => DeliveryType::DesktopVm,
            Self::ServiceVm => DeliveryType::ServiceVm,
            Self::AppVm => DeliveryType::AppVm,
            Self::AndroidVm => DeliveryType::AndroidVm,
            Self::ServiceContainer => DeliveryType::ServiceContainer,
        }
    }

    /// The delivery-view tab label.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::DesktopVm => "Desktop VM",
            Self::ServiceVm => "Service VM",
            Self::AppVm => "App VM",
            Self::AndroidVm => "Android VM",
            Self::ServiceContainer => "Container",
        }
    }

    /// The Mackes-Carbon glyph this view's tab wears (§4 — a registered symbolic
    /// icon, never a text glyph).
    pub(super) const fn icon(self) -> &'static str {
        match self {
            Self::DesktopVm => "view-grid",
            Self::ServiceVm => "globe",
            Self::AppVm => "overlay",
            Self::AndroidVm => "system-lock-screen",
            Self::ServiceContainer => "text-x-generic",
        }
    }
}

// ─────────────────────────────── the lens axis ──────────────────────────────

/// Which lens (panel) the main area shows for the selected [`DeliveryView`] —
/// the secondary nav axis. `Roster` is the selected view's own workload roster;
/// the rest are the cross-cutting panels the U14–U19 workers own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Panel {
    /// The selected delivery view's live workload roster (the default lens).
    #[default]
    Roster,
    /// Author + place a new workload (U14 placement · U15 form).
    Provision,
    /// Run Ansible + the live inventory (U17).
    Configure,
    /// Day-2 status, metrics + drift (U18).
    Status,
    /// Golden per-type image roster (U19).
    Images,
    /// Podman / Quadlet containers (U19).
    Containers,
}

impl Panel {
    /// Every panel lens, in sub-nav order.
    pub(super) const ALL: [Self; 6] = [
        Self::Roster,
        Self::Provision,
        Self::Configure,
        Self::Status,
        Self::Images,
        Self::Containers,
    ];

    /// The panel sub-nav label.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Roster => "Roster",
            Self::Provision => "Provision",
            Self::Configure => "Configure",
            Self::Status => "Status",
            Self::Images => "Images",
            Self::Containers => "Containers",
        }
    }

    /// The Mackes-Carbon glyph this panel's tab wears.
    pub(super) const fn icon(self) -> &'static str {
        match self {
            Self::Roster => "view",
            Self::Provision => "list-add",
            Self::Configure => "document-edit",
            Self::Status => "emblem-ok",
            Self::Images => "camera-photo",
            Self::Containers => "overlay",
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
    /// A destructive per-workload lifecycle op (`instance-reboot` /
    /// `instance-delete`) — echo the workload name.
    Lifecycle {
        /// The lifecycle verb.
        verb: &'static str,
        /// The target workload/instance id.
        instance_id: String,
        /// The workload's display name — the required echo.
        name: String,
    },
}

impl ArmAction {
    /// The `action/cloud/*` verb this action publishes (test seam — the perform
    /// path matches the variant directly).
    #[cfg(test)]
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
            Self::Lifecycle { name, .. } => format!("workload {name}"),
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
pub(super) struct AuditEntry {
    /// The verb performed (`provision` / `instance-delete` / …).
    verb: String,
    /// The honest verdict.
    outcome: AuditOutcome,
    /// A short detail (the staged plan summary / the failure / "audited").
    detail: String,
}

// ─────────────────────────────── the surface state ──────────────────────────

/// The **Workloads** cockpit state — the folded `state/cloud` mirror, the active
/// delivery view + lens, the selected placement node, each panel's own sub-state,
/// the typed-arming confirm, the one in-flight mutation, and the session audit
/// trail. A plain field on the shell's struct, borrowed `&mut` while the surface
/// is in view. Every panel owns its `State` in its own file, so a downstream
/// worker adds panel-specific state without touching this struct.
///
/// `#[derive(Debug)]` deliberately: it keeps every sub-panel `State` field a live
/// read (the panel workers fill them incrementally), so the seam compiles clean
/// as the panels land one by one.
#[derive(Debug)]
pub struct WorkloadsState {
    // ── preserved backend wiring (do not repurpose) ──
    /// The Bus persist root (the client data dir). `None` when the Bus is
    /// unavailable — an honest off-mesh degrade (§7), never a crash.
    bus_root: Option<PathBuf>,
    /// The per-node status mirrors, folded across every `state/cloud/<node>`
    /// topic (host-sorted). Empty when nothing has published yet — an honest
    /// pre-mirror state, never fabricated.
    states: Vec<CloudState>,
    /// When the mirror was last folded (the refresh cadence anchor).
    loaded_at: Option<Instant>,
    /// A manual refresh is queued — re-reads the mirror on the next poll.
    forced: bool,
    /// A pending typed-arming confirm for a destructive intent, if any.
    arming: Option<Arming>,
    /// The one in-flight mutation — its reply resolves into the note + audit.
    mutation_pending: Option<Pending>,
    /// A transient one-line action note — honest feedback, never a silent op.
    note: Option<String>,
    /// The session audit trail (newest last), capped at [`MAX_AUDIT`].
    audit: Vec<AuditEntry>,

    // ── the cockpit nav ──
    /// The active delivery-type view (the primary axis).
    view: DeliveryView,
    /// The active lens (the secondary axis).
    panel: Panel,
    /// The placement node the provision panel targets (from the placement
    /// picker); `None` until one is chosen.
    selected_node: Option<String>,

    // ── the per-panel sub-state (each panel worker owns its own file) ──
    //
    // `allow(dead_code)`: these are the fan-out seam — each panel worker (U14–U19)
    // reads + fills its own `State` in its own file. They are honestly unread
    // until then; the allow drops off the moment a worker consumes the field.
    // (`configure` is already consumed by `configure_body` + the Configure lens.)
    /// U14 — placement picker state.
    #[allow(dead_code)]
    placement: placement::State,
    /// U15 — provision form state.
    #[allow(dead_code)]
    form: provision_form::State,
    /// U17 — configure + inventory state (holds the Ansible playbook/group).
    configure: configure::State,
    /// U18 — status + metrics state.
    #[allow(dead_code)]
    status: status::State,
    /// U19 — images panel state.
    #[allow(dead_code)]
    images: images::State,
    /// U19 — containers panel state.
    #[allow(dead_code)]
    containers: containers::State,

    // ── the per-delivery-view sub-state (each U16 view worker owns its file) ──
    /// Desktop VM view state.
    #[allow(dead_code)]
    desktop_vm: views::desktop_vm::State,
    /// Service VM view state.
    #[allow(dead_code)]
    service_vm: views::service_vm::State,
    /// App VM view state.
    #[allow(dead_code)]
    app_vm: views::app_vm::State,
    /// Android VM view state.
    #[allow(dead_code)]
    android_vm: views::android_vm::State,
    /// Service Container view state.
    #[allow(dead_code)]
    service_container: views::container::State,
}

impl Default for WorkloadsState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            states: Vec::new(),
            loaded_at: None,
            forced: false,
            arming: None,
            mutation_pending: None,
            note: None,
            audit: Vec::new(),
            view: DeliveryView::default(),
            panel: Panel::default(),
            selected_node: None,
            placement: placement::State::default(),
            form: provision_form::State::default(),
            configure: configure::State::default(),
            status: status::State::default(),
            images: images::State::default(),
            containers: containers::State::default(),
            desktop_vm: views::desktop_vm::State::default(),
            service_vm: views::service_vm::State::default(),
            app_vm: views::app_vm::State::default(),
            android_vm: views::android_vm::State::default(),
            service_container: views::container::State::default(),
        }
    }
}

/// The external seam name main.rs (and any other caller) binds to — an alias so
/// the rename to [`WorkloadsState`] stays source-compatible.
pub(super) type InfraCodeState = WorkloadsState;

impl WorkloadsState {
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

    /// The Configure lens's request body — the picked playbook + target group.
    /// (The worker converges `cloud_vm` on `site.yml`; the selection is honest
    /// operator context the reply echoes.) The inputs live in [`configure::State`]
    /// so the U17 worker owns them without touching this struct.
    fn configure_body(&self) -> String {
        serde_json::json!({
            "playbook": self.configure.playbook.trim(),
            "group": self.configure.group.trim(),
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
    /// node the worker stages a `tofu plan` and returns it in the reply.
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
    /// directly — armed to the backend, no typed confirm (never destructive). The
    /// roster rows drive this seam.
    pub(super) fn issue_lifecycle_direct(
        &mut self,
        verb: &'static str,
        instance_id: &str,
        name: &str,
    ) {
        let body = serde_json::json!({ "instance": instance_id }).to_string();
        self.issue(
            verb,
            Some(&body),
            &format!("{} on {name}", verb_label(verb)),
        );
    }

    /// Open the typed-arming confirm for a destructive lifecycle op
    /// (`instance-reboot` / `instance-delete`) — nothing publishes until the
    /// workload name is typed (RUN-006). The roster rows drive this seam.
    pub(super) fn arm_lifecycle(&mut self, verb: &'static str, instance_id: &str, name: &str) {
        self.arming = Some(Arming {
            action: ArmAction::Lifecycle {
                verb,
                instance_id: instance_id.to_string(),
                name: name.to_string(),
            },
            typed: String::new(),
        });
    }

    // ── the nav + menubar seam (§6, one dispatch path shared with the body) ──

    /// The folded per-node `state/cloud` mirror (the menubar status cluster reads
    /// the same fold the body renders — no second read, §7).
    pub(super) fn states(&self) -> &[CloudState] {
        &self.states
    }

    /// Every workload of a given delivery type, across every node — the idiom a
    /// delivery view uses to read its own rows from the mirror.
    pub(super) fn workloads_of(
        &self,
        view: DeliveryView,
    ) -> impl Iterator<Item = &WorkloadRow> + '_ {
        let dt = view.delivery_type();
        self.states
            .iter()
            .flat_map(|s| s.workloads.iter())
            .filter(move |w| w.delivery_type == dt)
    }

    /// Which delivery view is showing (test seam; production reads the field).
    #[cfg(test)]
    pub(super) fn view(&self) -> DeliveryView {
        self.view
    }

    /// Switch the active delivery view.
    pub(super) fn set_view(&mut self, view: DeliveryView) {
        self.view = view;
    }

    /// Which lens is showing (test seam; production reads the field).
    #[cfg(test)]
    pub(super) fn panel(&self) -> Panel {
        self.panel
    }

    /// Switch the active lens.
    pub(super) fn set_panel(&mut self, panel: Panel) {
        self.panel = panel;
    }

    /// The placement node the provision panel targets, if one is chosen.
    pub(super) fn selected_node(&self) -> Option<&str> {
        self.selected_node.as_deref()
    }

    /// Queue an immediate re-fold of the `state/cloud` mirror.
    pub(super) fn request_refresh(&mut self) {
        self.forced = true;
    }

    /// Surface the honest apply-gate + audit posture in the action note (Help).
    pub(super) fn set_help_note(&mut self) {
        self.note = Some(
            "Live apply is capability-gated per node (armed token); provision, configure, and \
             destroy stage as dry-runs otherwise. Every destructive op passes a typed-confirm; \
             performed ops land in the Status audit trail."
                .to_string(),
        );
    }

    /// Whether a typed-arming confirm is open (test seam).
    #[cfg(test)]
    pub(super) fn has_arming(&self) -> bool {
        self.arming.is_some()
    }

    /// The current action note text, if any (test seam).
    #[cfg(test)]
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

// ───────────────────────────────── the render ───────────────────────────────

/// Render the Workloads cockpit into `ui`: the shared MENUBAR-ALL bar, the
/// delivery-view selector, the lens sub-nav, the typed-arming confirm + action
/// note, then the active lens's body.
///
/// The name is the stable external entry seam (main.rs binds it); the state type
/// is [`WorkloadsState`] (aliased as `InfraCodeState`).
pub fn infra_code_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    if let Some(action) = menubar::show(ui, state) {
        menubar::apply(state, action);
    }
    ui.separator();
    ui.add_space(Style::SP_XS);

    delivery_view_bar(ui, state);
    ui.add_space(Style::SP_XS);
    panel_bar(ui, state);
    ui.add_space(Style::SP_S);

    render_arming(ui, state);
    render_note(ui, state);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match state.panel {
            Panel::Roster => {
                let view = state.view;
                views::dispatch(ui, state, view);
            }
            Panel::Provision => {
                if let Some(node) = placement::placement_picker(ui, state) {
                    state.selected_node = Some(node);
                }
                provision_form::provision_form(ui, state);
            }
            Panel::Configure => configure::configure_panel(ui, state),
            Panel::Status => status::status_panel(ui, state),
            Panel::Images => images::images_panel(ui, state),
            Panel::Containers => containers::containers_panel(ui, state),
        });
}

/// The delivery-view selector — the primary axis. Each tab is a Carbon icon +
/// label, tinted the Workloads accent when active (§4). Selecting a view snaps
/// the lens back to its roster.
fn delivery_view_bar(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_M;
        for view in DeliveryView::ALL {
            let selected = state.view == view;
            if nav_tab(
                ui,
                selected,
                view.icon(),
                view.label(),
                Style::ACCENT_WORKLOADS,
            )
            .clicked()
            {
                state.set_view(view);
                state.set_panel(Panel::Roster);
            }
        }
    });
}

/// The lens sub-nav — the secondary axis (roster + the cross-cutting panels).
/// Tinted the blue action accent when active, so it reads as subordinate to the
/// delivery-view selector above it.
fn panel_bar(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_S;
        for panel in Panel::ALL {
            let selected = state.panel == panel;
            if nav_tab(ui, selected, panel.icon(), panel.label(), Style::ACCENT).clicked() {
                state.set_panel(panel);
            }
        }
    });
}

/// One nav tab — a clickable icon + label row. The icon is drawn through
/// [`carbon_icon`] with the tab's accent as the current text color, so the glyph
/// glows the axis accent when active.
fn nav_tab(
    ui: &mut egui::Ui,
    selected: bool,
    icon: &str,
    label: &str,
    accent: Color32,
) -> egui::Response {
    let color = if selected { accent } else { Style::TEXT_DIM };
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

/// A small text button sized for a roster row's inline lifecycle verb.
fn row_button(ui: &mut egui::Ui, label: &str, danger: bool) -> egui::Response {
    let color = if danger { Style::DANGER } else { Style::TEXT };
    ui.add(egui::Button::new(
        RichText::new(label).size(Style::SMALL).color(color),
    ))
}

/// The transient one-line action note (last issued op / its outcome) with a
/// dismiss affordance — honest feedback, never a silent op.
fn render_note(ui: &mut egui::Ui, state: &mut WorkloadsState) {
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
fn render_arming(ui: &mut egui::Ui, state: &mut WorkloadsState) {
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

// ─────────────────────────── shared panel-body helpers ──────────────────────

/// The honest **not yet built** stub every seam panel renders for its
/// unimplemented body (§7) — a Workloads-accent glyph + `<unit> · <what>` +
/// the honest reason. Downstream (U14–U19) replaces the panel's body with the
/// real render; the backend (WL-ARCH-001) is already live behind the Bus.
pub(super) fn workloads_pending(ui: &mut egui::Ui, unit: &str, what: &str) {
    egui::Frame::group(ui.style())
        .fill(Style::SURFACE_HI)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS_S)
        .shadow(card_shadow())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
                    carbon_icon(ui, "view-grid", Style::BODY + 2.0);
                });
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(format!("{unit} \u{00B7} {what}"))
                        .size(Style::BODY)
                        .strong()
                        .color(Style::TEXT),
                );
            });
            mde_egui::muted_note(
                ui,
                "This cockpit panel is a seam stub \u{2014} not yet built. The Workloads backend \
                 (OpenTofu + Ansible + libvirt + Podman) is live behind the Bus; this surface \
                 panel lands with its build unit.",
            );
        });
    ui.add_space(Style::SP_S);
}

/// An honest one-line summary of the folded `state/cloud` mirror — the shared
/// context line the stub panels show so the seam's live data flow is visible
/// even before a panel's body lands.
pub(super) fn mirror_summary(ui: &mut egui::Ui, state: &WorkloadsState) {
    let nodes = state.states.len();
    let workloads: usize = state.states.iter().map(|s| s.workloads.len()).sum();
    mde_egui::muted_note(
        ui,
        format!("state/cloud mirror: {nodes} node(s) \u{00B7} {workloads} workload(s) folded."),
    );
}

/// The session audit trail — the workspace's honest record of the ops it
/// requested (verb · verdict · detail), newest first. An empty trail reads
/// honestly. The preserved audit machinery renders here (the Status lens's home);
/// U18 enriches the rest of the day-2 view around it.
pub(super) fn render_audit(ui: &mut egui::Ui, audit: &[AuditEntry]) {
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

/// Render one delivery view's live roster — the U3 seam the five U16 view files
/// share: the view heading, each matching [`WorkloadRow`] from the mirror with
/// its inline lifecycle verbs (Start/Stop direct, Reboot…/Delete… armed), and an
/// honest note that the rich per-type body is U16. An empty roster reads
/// honestly (§7), never fabricated.
pub(super) fn roster(ui: &mut egui::Ui, state: &mut WorkloadsState, view: DeliveryView) {
    view_heading(ui, view);

    // Snapshot the matching rows so the immutable mirror borrow ends before the
    // lifecycle verbs take `&mut state`.
    let rows: Vec<WorkloadRow> = state.workloads_of(view).cloned().collect();
    if rows.is_empty() {
        crate::session::empty_state(
            ui,
            "No workloads of this type yet",
            "This delivery-type roster fills once a placement node reports a matching workload in \
             its state/cloud mirror.",
        );
    } else {
        for row in &rows {
            workload_row(ui, state, row);
        }
    }

    mde_egui::muted_note(
        ui,
        "U16 \u{2014} the rich per-type view (live metrics, drift, console attach) lands with its \
         build unit.",
    );
}

/// A delivery-view heading — the view's icon + label + the placement blurb.
fn view_heading(ui: &mut egui::Ui, view: DeliveryView) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, view.icon(), Style::BODY + 2.0);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(view.label())
                .size(Style::BODY)
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
    });
    mde_egui::muted_note(
        ui,
        "Workloads of this delivery type, placed on their mesh nodes.",
    );
    ui.add_space(Style::SP_S);
}

/// One workload roster row — its name · status · node, then the inline lifecycle
/// verbs wired to the preserved seams (destructive ones typed-armed).
fn workload_row(ui: &mut egui::Ui, state: &mut WorkloadsState, row: &WorkloadRow) {
    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&row.name)
                        .size(Style::BODY)
                        .strong()
                        .color(Style::TEXT),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(&row.status).size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(format!("on {}", row.node)).size(Style::SMALL),
                );
            });
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                if row_button(ui, "Start", false).clicked() {
                    state.issue_lifecycle_direct("instance-start", &row.name, &row.name);
                }
                if row_button(ui, "Stop", false).clicked() {
                    state.issue_lifecycle_direct("instance-stop", &row.name, &row.name);
                }
                if row_button(ui, "Reboot\u{2026}", true).clicked() {
                    state.arm_lifecycle("instance-reboot", &row.name, &row.name);
                }
                if row_button(ui, "Delete\u{2026}", true).clicked() {
                    state.arm_lifecycle("instance-delete", &row.name, &row.name);
                }
            });
        });
    ui.add_space(Style::SP_S);
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

// ─────────────────────────── the seam module layout ─────────────────────────

mod menubar;

mod placement;
mod provision_form;

mod configure;
mod containers;
mod images;
mod status;

mod views;

#[cfg(test)]
#[allow(clippy::panic)]
mod tests;
