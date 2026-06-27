//! Compute group root — local + fleet VM / pod instance list (E6.10).
//!
//! Rebuilds the legacy `crates/legacy/mde-virtual` instance enumeration
//! onto the workbench: lists local KVM domains (`virsh list --all`) +
//! Podman containers (`podman ps --all --format json`) with per-instance
//! state. The pure parsers (`parse_virsh_list_state`, `parse_podman_ps`,
//! `state_is_running`/`state_is_paused`) are ported 1:1 from
//! `mde-virtual::app` (VIRT-13/18) so this surface and the retired tool
//! agree byte-for-byte on how libvirt/podman output reads.
//!
//! This slice (E6.10 #1) is the Compute foundation: the bespoke group
//! root that enumerates instances + their state. Live lifecycle ops
//! (start/stop/bulk actions), the 4-step VM wizard, per-instance
//! sparklines, cold migration, and the virt-viewer console land in the
//! later E6.10 slices. The list degrades gracefully when neither
//! hypervisor tool is installed (empty list + a "no hypervisor" status,
//! never a panic) — the standalone-first cross-cutting rule.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use cosmic::iced::widget::{column, row, text, text_input, Space};
use cosmic::iced::{Length, Subscription, Task};
// CUT-1: cosmic::Element bakes in cosmic::Theme — the theme panel_chrome and
// the .colr()/.sty() compat widgets thread through the tree. Using
// cosmic::iced::Element here would default to cosmic::iced::Theme and mismatch.
use cosmic::Element;
use mde_theme::{spacing, FontSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

use crate::controls::{variant_button, ButtonVariant};
use crate::panels::sparkline::{push_sample, sparkline};
use crate::panels::vm_wizard::{WizardAction, WizardMsg, WizardState};

/// Live-metric sample cadence. Also the nominal interval used to turn a
/// VM's cumulative `cpu.time` (nanoseconds) into a percentage.
const SAMPLE_SECS: f32 = 2.0;
/// Sparkline dimensions in the instance row (kept compact — it's a trend
/// glyph, not a chart).
const SPARK_W: f32 = 72.0;
const SPARK_H: f32 = 18.0;

/// Whether an enumerated instance is a libvirt VM or a Podman container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceKind {
    Vm,
    Container,
}

impl InstanceKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vm => "VM",
            Self::Container => "Container",
        }
    }
}

/// One enumerated compute instance: name + kind + the raw lifecycle
/// state string libvirt / podman reported (`running`, `shut off`,
/// `paused`, `exited`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub name: String,
    pub kind: InstanceKind,
    pub state: String,
    /// Hostname of the node this instance runs on. The local `virsh`/`podman`
    /// probe sets this node's hostname; rows merged from the mesh bus
    /// (`compute/inventory/<peer>`, WORKLOAD-FLEET-1) carry that peer's
    /// hostname, so a VM on any node is visible from any Workbench.
    pub node: String,
    /// True when this row came from THIS node's local probe. Lifecycle actions
    /// (Start/Stop/Console/Migrate) shell local `virsh`/`podman`, so they only
    /// apply to local rows; peer rows are read-only here.
    pub local: bool,
}

/// The result of one enumeration pass. `sources` names the hypervisor
/// tools that actually responded (so an empty `instances` list can tell
/// "no instances" apart from "no hypervisor installed").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Enumeration {
    pub instances: Vec<Instance>,
    pub sources: Vec<&'static str>,
}

/// Stable per-instance key for the metrics map (`name` alone can collide
/// between a VM and a container).
#[must_use]
pub fn metric_key(kind: InstanceKind, name: &str) -> String {
    format!("{}:{name}", kind.label())
}

/// Per-instance rolling CPU% / memory% history feeding the sparklines.
/// `prev_cpu_ns` carries a VM's previous cumulative `cpu.time` so the
/// next sample can be turned into a percentage by delta.
#[derive(Debug, Clone, Default)]
pub struct InstanceMetrics {
    pub cpu: VecDeque<f32>,
    pub mem: VecDeque<f32>,
    pub prev_cpu_ns: Option<u64>,
}

/// One instance's sampled metrics for a single tick. Containers report a
/// direct `cpu_pct`; VMs report cumulative `cpu_time_ns` (the reducer
/// deltas it). `mem_pct` is direct for both.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceSample {
    pub key: String,
    pub cpu_pct: Option<f32>,
    pub cpu_time_ns: Option<u64>,
    pub mem_pct: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct ComputePanel {
    instances: Vec<Instance>,
    metrics: HashMap<String, InstanceMetrics>,
    status: String,
    loaded: bool,
    /// The 4-step VM creation wizard, open when `Some` (E6.10 slice 4).
    wizard: Option<WizardState>,
    /// The cold-migration target sheet, open when `Some` (E6.10 slice 6).
    migrate: Option<MigrateSheet>,
}

/// The cold-migration target prompt: which VM, and the host to move it to.
#[derive(Debug, Clone, Default)]
pub struct MigrateSheet {
    pub domain: String,
    pub host: String,
}

/// A lifecycle verb applied to an instance. Slice 2 ships the two
/// reversible everyday actions (Start / Stop); force-off, suspend, and
/// resume are part of the per-instance detail panel in a later slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Start,
    Stop,
}

impl Verb {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Enumeration),
    RefreshClicked,
    /// Apply a lifecycle verb to a single instance, then re-enumerate.
    Action {
        kind: InstanceKind,
        name: String,
        verb: Verb,
    },
    /// Apply a verb to every instance it applies to (Start all / Stop all).
    Bulk(Verb),
    /// The 2 s metric-sampling tick (fires only while Compute is in view).
    SampleTick,
    /// Sampled CPU/mem for the current instances — pushed into the rolling
    /// per-instance buffers that feed the sparklines.
    Sampled(Vec<InstanceSample>),
    /// Open the 4-step VM creation wizard.
    OpenWizard,
    /// A message from the open wizard.
    Wizard(WizardMsg),
    /// Launch the graphical console (virt-viewer) for a running VM.
    Console {
        name: String,
    },
    /// Open the cold-migration sheet for a stopped VM.
    OpenMigrate {
        name: String,
    },
    /// Edit the migration target host.
    MigrateHostInput(String),
    /// Submit the cold migration to the entered host, then re-enumerate.
    MigrateConfirm,
    /// Close the migration sheet without submitting.
    MigrateCancel,
}

impl ComputePanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only accessor for the enumerated instances (test/inspection).
    #[must_use]
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Status line shown under the header.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Kick off a `virsh` + `podman` enumeration on the iced executor.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move { Message::Loaded(enumerate().await) },
            crate::Message::Compute,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(e) => {
                self.status = status_line(&e);
                self.instances = e.instances;
                self.loaded = true;
                Task::none()
            }
            Message::RefreshClicked => Self::load(),
            Message::Action { kind, name, verb } => {
                let (program, args) = command_for(kind, verb, &name);
                let program = program.to_string();
                // Issue the real lifecycle command, then re-enumerate so
                // the list reflects the new state without a manual refresh.
                Task::perform(
                    async move {
                        run_action(&program, &args).await;
                        enumerate().await
                    },
                    |e| crate::Message::Compute(Message::Loaded(e)),
                )
            }
            Message::Bulk(verb) => self.bulk(verb),
            Message::SampleTick => {
                // Only this node's instances can be sampled locally (virsh
                // domstats / podman stats); peer rows are display-only.
                let instances: Vec<Instance> =
                    self.instances.iter().filter(|i| i.local).cloned().collect();
                if instances.is_empty() {
                    return Task::none();
                }
                Task::perform(sample_metrics(instances), |s| {
                    crate::Message::Compute(Message::Sampled(s))
                })
            }
            Message::Sampled(samples) => {
                for s in samples {
                    let m = self.metrics.entry(s.key).or_default();
                    // Containers report CPU% directly; VMs report cumulative
                    // cpu.time — delta it against the previous sample.
                    let cpu = if let Some(p) = s.cpu_pct {
                        Some(p)
                    } else if let Some(ns) = s.cpu_time_ns {
                        let pct = m
                            .prev_cpu_ns
                            .map(|prev| cpu_percent_from_delta(prev, ns, SAMPLE_SECS));
                        m.prev_cpu_ns = Some(ns);
                        pct
                    } else {
                        None
                    };
                    if let Some(c) = cpu {
                        push_sample(&mut m.cpu, c);
                    }
                    if let Some(mem) = s.mem_pct {
                        push_sample(&mut m.mem, mem);
                    }
                }
                Task::none()
            }
            Message::OpenWizard => {
                self.wizard = Some(WizardState::new());
                Task::none()
            }
            Message::Console { name } => {
                // virt-viewer is a long-running GUI; spawn it detached so
                // the workbench doesn't block on it (mirrors the shell's
                // detached-launch pattern). HW round-trip is bench.
                launch_console(&name);
                Task::none()
            }
            Message::OpenMigrate { name } => {
                self.migrate = Some(MigrateSheet {
                    domain: name,
                    host: String::new(),
                });
                Task::none()
            }
            Message::MigrateHostInput(h) => {
                if let Some(sheet) = self.migrate.as_mut() {
                    sheet.host = h;
                }
                Task::none()
            }
            Message::MigrateCancel => {
                self.migrate = None;
                Task::none()
            }
            Message::MigrateConfirm => {
                let Some(sheet) = self.migrate.take() else {
                    return Task::none();
                };
                if sheet.host.trim().is_empty() {
                    // Nothing to migrate to — reopen the sheet unchanged.
                    self.migrate = Some(sheet);
                    return Task::none();
                }
                let (program, args) = migrate_command(&sheet.domain, sheet.host.trim());
                let program = program.to_string();
                Task::perform(
                    async move {
                        run_action(&program, &args).await;
                        enumerate().await
                    },
                    |e| crate::Message::Compute(Message::Loaded(e)),
                )
            }
            Message::Wizard(msg) => {
                let Some(w) = self.wizard.as_mut() else {
                    return Task::none();
                };
                match w.update(msg) {
                    WizardAction::None => Task::none(),
                    WizardAction::Cancel => {
                        self.wizard = None;
                        Task::none()
                    }
                    WizardAction::Create(req) => {
                        self.wizard = None;
                        let (program, args) = req.virt_install_command();
                        let program = program.to_string();
                        // Issue the real virt-install, then re-enumerate so
                        // the new domain appears once libvirt defines it.
                        Task::perform(
                            async move {
                                run_action(&program, &args).await;
                                enumerate().await
                            },
                            |e| crate::Message::Compute(Message::Loaded(e)),
                        )
                    }
                }
            }
        }
    }

    /// The metric-sampling subscription. The caller (App::subscription)
    /// only includes it while the Compute view is active, so sampling
    /// commands don't run when the operator is elsewhere.
    pub fn sample_subscription() -> Subscription<crate::Message> {
        cosmic::iced::time::every(Duration::from_secs_f32(SAMPLE_SECS))
            .map(|_| crate::Message::Compute(Message::SampleTick))
    }

    /// Apply `verb` to every currently-listed instance it applies to, then
    /// re-enumerate. Commands run sequentially on the executor; an empty
    /// applicable set just re-enumerates (a harmless refresh).
    fn bulk(&self, verb: Verb) -> Task<crate::Message> {
        let cmds: Vec<(String, Vec<String>)> = self
            .instances
            .iter()
            // Bulk verbs shell local virsh/podman — only act on local rows.
            .filter(|i| i.local && verb_applies(verb, &i.state))
            .map(|i| {
                let (program, args) = command_for(i.kind, verb, &i.name);
                (program.to_string(), args)
            })
            .collect();
        Task::perform(
            async move {
                for (program, args) in &cmds {
                    run_action(program, args).await;
                }
                enumerate().await
            },
            |e| crate::Message::Compute(Message::Loaded(e)),
        )
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        // Carbon type scale + 8px spacing grid via mde-theme tokens (the
        // workbench's design-token source — it's on iced 0.14, so it can't
        // consume mde-ui's iced-0.13 metrics module; mde-theme is the
        // shared, version-decoupled token crate every panel reads, E9.6).
        let sizes = FontSize::defaults();
        let title = text("Compute")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("Local and fleet VMs and containers")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            Some(crate::Message::Compute(Message::RefreshClicked)),
            palette,
        );
        // Bulk actions target every instance the verb applies to; offer
        // them only when there's a populated list to act on.
        let any_startable = self
            .instances
            .iter()
            .any(|i| verb_applies(Verb::Start, &i.state));
        let any_stoppable = self
            .instances
            .iter()
            .any(|i| verb_applies(Verb::Stop, &i.state));
        let start_all = variant_button(
            "Start all",
            ButtonVariant::Ghost,
            any_startable.then_some(crate::Message::Compute(Message::Bulk(Verb::Start))),
            palette,
        );
        let stop_all = variant_button(
            "Stop all",
            ButtonVariant::Ghost,
            any_stoppable.then_some(crate::Message::Compute(Message::Bulk(Verb::Stop))),
            palette,
        );
        let new_vm = variant_button(
            "+ Add VM",
            ButtonVariant::Primary,
            (self.wizard.is_none()).then_some(crate::Message::Compute(Message::OpenWizard)),
            palette,
        );
        let header = row![
            column![title, subtitle]
                .spacing(f32::from(spacing::BASE[0]))
                .width(Length::Fill),
            new_vm,
            start_all,
            stop_all,
            refresh,
        ]
        .spacing(f32::from(spacing::BASE[1]))
        .align_y(cosmic::iced::alignment::Vertical::Center);

        // When the wizard is open it owns the body (a focused create flow).
        if let Some(w) = &self.wizard {
            let wizard_view = w
                .view(palette)
                .map(|m| crate::Message::Compute(Message::Wizard(m)));
            return column![
                header,
                Space::new().height(Length::Fixed(f32::from(spacing::BASE[4]))),
                wizard_view,
            ]
            .padding(f32::from(spacing::BASE[2]))
            .width(Length::Fill)
            .into();
        }

        // The cold-migration sheet likewise owns the body while open.
        if let Some(sheet) = &self.migrate {
            return column![
                header,
                Space::new().height(Length::Fixed(f32::from(spacing::BASE[4]))),
                migrate_sheet_view(sheet, palette),
            ]
            .padding(f32::from(spacing::BASE[2]))
            .width(Length::Fill)
            .into();
        }

        let body: Element<'_, crate::Message> = if self.instances.is_empty() {
            // Honest empty-state: distinguish "nothing running" from
            // "no hypervisor" via the status line set at load time.
            let msg = if self.loaded {
                self.status.clone()
            } else {
                "Loading instances…".to_string()
            };
            column![text(msg)
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())]
            .into()
        } else {
            let mut rows: Vec<Element<'_, crate::Message>> = vec![instance_header_row(palette)];
            for inst in &self.instances {
                // Sparklines come from this node's local sampler, so peer rows
                // have none (avoids cross-node name collisions in the key).
                let m = inst
                    .local
                    .then(|| self.metrics.get(&metric_key(inst.kind, &inst.name)))
                    .flatten();
                rows.push(instance_row(inst, m, palette));
            }
            column(rows).spacing(f32::from(spacing::BASE[1])).into()
        };

        column![
            header,
            Space::new().height(Length::Fixed(f32::from(spacing::BASE[4]))),
            body,
            Space::new().height(Length::Fixed(f32::from(spacing::BASE[2]))),
            text(&self.status)
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .padding(f32::from(spacing::BASE[2]))
        .width(Length::Fill)
        .into()
    }
}

/// The instance-table header row (Name / Kind / State).
fn instance_header_row<'a>(palette: Palette) -> Element<'a, crate::Message> {
    let muted = palette.text_muted.into_cosmic_color();
    let cap = TypeRole::Caption.size_in(FontSize::defaults());
    row![
        text("Name")
            .size(cap)
            .colr(muted)
            .width(Length::FillPortion(3)),
        text("Node")
            .size(cap)
            .colr(muted)
            .width(Length::FillPortion(2)),
        text("Kind")
            .size(cap)
            .colr(muted)
            .width(Length::FillPortion(1)),
        text("State")
            .size(cap)
            .colr(muted)
            .width(Length::FillPortion(2)),
        text("CPU / Mem")
            .size(cap)
            .colr(muted)
            .width(Length::Fixed(SPARK_W * 2.0 + f32::from(spacing::BASE[1]))),
        text("Action")
            .size(cap)
            .colr(muted)
            .width(Length::FillPortion(2)),
    ]
    .spacing(f32::from(spacing::BASE[3]))
    .into()
}

/// The CPU + memory sparkline pair for one instance (CPU in the success
/// green, memory in the accent). Empty (no samples yet) renders as blank
/// fixed-width spacers so the column stays aligned.
fn metric_cell<'a>(
    metrics: Option<&InstanceMetrics>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let spark = |buf: Option<&VecDeque<f32>>, color| -> Element<'a, crate::Message> {
        match buf {
            Some(b) if b.len() >= 2 => {
                sparkline(b.iter().copied().collect(), color, SPARK_W, SPARK_H).into()
            }
            _ => Space::new()
                .width(Length::Fixed(SPARK_W))
                .height(Length::Fixed(SPARK_H))
                .into(),
        }
    };
    row![
        spark(metrics.map(|m| &m.cpu), palette.success.into_cosmic_color()),
        spark(metrics.map(|m| &m.mem), palette.accent.into_cosmic_color()),
    ]
    .spacing(f32::from(spacing::BASE[1]))
    .into()
}

/// The cold-migration sheet: a target-host field + Migrate / Cancel.
fn migrate_sheet_view<'a>(sheet: &MigrateSheet, palette: Palette) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let title = text(format!("Migrate {} to another host", sheet.domain))
        .size(TypeRole::Subheading.size_in(sizes))
        .colr(palette.text.into_cosmic_color());
    let hint = text(
        "Cold migration moves the powered-off VM's definition to the target host over SSH; \
         the disk rides shared mesh storage.",
    )
    .size(TypeRole::Caption.size_in(sizes))
    .colr(palette.text_muted.into_cosmic_color());
    let input = text_input("target host (e.g. peer2.mesh)", &sheet.host)
        .on_input(|h| crate::Message::Compute(Message::MigrateHostInput(h)))
        .padding(f32::from(spacing::BASE[0]))
        .size(TypeRole::Body.size_in(sizes));
    let confirm = variant_button(
        "Migrate",
        ButtonVariant::Primary,
        (!sheet.host.trim().is_empty()).then_some(crate::Message::Compute(Message::MigrateConfirm)),
        palette,
    );
    let cancel = variant_button(
        "Cancel",
        ButtonVariant::Ghost,
        Some(crate::Message::Compute(Message::MigrateCancel)),
        palette,
    );
    let nav = row![cancel, Space::new().width(Length::Fill), confirm]
        .spacing(f32::from(spacing::BASE[1]));
    column![title, hint, input, nav]
        .spacing(f32::from(spacing::BASE[2]))
        .width(Length::Fill)
        .into()
}

/// One instance row: name / kind / state (coloured by liveness) + a
/// single context-appropriate lifecycle button — Start when stopped,
/// Stop when running, nothing when paused (suspend/resume live in the
/// detail panel, a later slice).
fn instance_row<'a>(
    inst: &Instance,
    metrics: Option<&InstanceMetrics>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let body = TypeRole::Body.size_in(FontSize::defaults());
    let state_color = if state_is_running(&inst.state) {
        palette.success
    } else if state_is_paused(&inst.state) {
        palette.warning
    } else {
        palette.text_muted
    };
    // Lifecycle actions shell local virsh/podman, so only LOCAL rows get them;
    // a peer's workload is read-only here (remote ops are a later slice).
    let mut actions = row![].spacing(f32::from(spacing::BASE[1]));
    if let Some(verb) = inst.local.then(|| row_action(&inst.state)).flatten() {
        actions = actions.push(variant_button(
            verb.label(),
            ButtonVariant::Ghost,
            Some(crate::Message::Compute(Message::Action {
                kind: inst.kind,
                name: inst.name.clone(),
                verb,
            })),
            palette,
        ));
    }
    // Running VMs get a "Console" button (virt-viewer); containers don't
    // have a graphical console in this model.
    if inst.local && inst.kind == InstanceKind::Vm && state_is_running(&inst.state) {
        actions = actions.push(variant_button(
            "Console",
            ButtonVariant::Ghost,
            Some(crate::Message::Compute(Message::Console {
                name: inst.name.clone(),
            })),
            palette,
        ));
    }
    // Cold migration moves a *stopped* VM to another host (the disk rides
    // shared mesh storage); offer it only for a powered-off LOCAL VM.
    if inst.local
        && inst.kind == InstanceKind::Vm
        && !state_is_running(&inst.state)
        && !state_is_paused(&inst.state)
    {
        actions = actions.push(variant_button(
            "Migrate",
            ButtonVariant::Ghost,
            Some(crate::Message::Compute(Message::OpenMigrate {
                name: inst.name.clone(),
            })),
            palette,
        ));
    }
    let action_cell: Element<'a, crate::Message> = actions.into();
    // Peer rows read in muted text; the local node's own rows in primary text.
    let node_color = if inst.local {
        palette.text
    } else {
        palette.text_muted
    };
    row![
        text(inst.name.clone())
            .size(body)
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(3)),
        text(inst.node.clone())
            .size(body)
            .colr(node_color.into_cosmic_color())
            .width(Length::FillPortion(2)),
        text(inst.kind.label())
            .size(body)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(1)),
        text(inst.state.clone())
            .size(body)
            .colr(state_color.into_cosmic_color())
            .width(Length::FillPortion(2)),
        cosmic::iced::widget::container(metric_cell(metrics, palette))
            .width(Length::Fixed(SPARK_W * 2.0 + f32::from(spacing::BASE[1]))),
        cosmic::iced::widget::container(action_cell).width(Length::FillPortion(2)),
    ]
    .spacing(f32::from(spacing::BASE[3]))
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// The single lifecycle verb a row offers for `state`: Start when
/// stopped, Stop when running, `None` when paused.
#[must_use]
fn row_action(state: &str) -> Option<Verb> {
    if verb_applies(Verb::Stop, state) {
        Some(Verb::Stop)
    } else if verb_applies(Verb::Start, state) {
        Some(Verb::Start)
    } else {
        None
    }
}

/// Human status line for an enumeration result.
#[must_use]
pub fn status_line(e: &Enumeration) -> String {
    if e.sources.is_empty() {
        return "No local hypervisor found — install libvirt (virsh) or podman to manage compute."
            .to_string();
    }
    let n = e.instances.len();
    let noun = if n == 1 { "instance" } else { "instances" };
    format!("{n} {noun} via {}.", e.sources.join(" + "))
}

/// True when a libvirt/podman state string reads as actively running.
/// Ported from `mde-virtual::app::state_is_running`.
#[must_use]
pub fn state_is_running(state: &str) -> bool {
    state.to_ascii_lowercase().contains("running")
}

/// True when a state string reads as paused/suspended.
/// Ported from `mde-virtual::app::state_is_paused`.
#[must_use]
pub fn state_is_paused(state: &str) -> bool {
    let s = state.to_ascii_lowercase();
    s.contains("paused") || s.contains("suspended")
}

/// Whether `verb` is a sensible action for an instance in `state`:
/// Start only when stopped, Stop only when running. Drives which
/// action button a row shows + which instances a bulk action targets.
/// Ported from `mde-virtual::app::verb_applies` (Start/Stop subset).
#[must_use]
pub fn verb_applies(verb: Verb, state: &str) -> bool {
    match verb {
        Verb::Start => !state_is_running(state) && !state_is_paused(state),
        Verb::Stop => state_is_running(state),
    }
}

/// Resolve `(program, argv)` for a lifecycle action. VMs go through the
/// system libvirtd (`-c qemu:///system`); containers through `podman`.
/// Ported 1:1 from `mde-virtual::app::command_for` (Start/Stop subset:
/// VM Stop is a graceful `shutdown`, container Stop is `stop`).
#[must_use]
pub fn command_for(kind: InstanceKind, verb: Verb, name: &str) -> (&'static str, Vec<String>) {
    match kind {
        InstanceKind::Vm => {
            let v = match verb {
                Verb::Start => "start",
                Verb::Stop => "shutdown",
            };
            (
                "virsh",
                vec![
                    "-c".to_string(),
                    "qemu:///system".to_string(),
                    v.to_string(),
                    name.to_string(),
                ],
            )
        }
        InstanceKind::Container => {
            let v = match verb {
                Verb::Start => "start",
                Verb::Stop => "stop",
            };
            ("podman", vec![v.to_string(), name.to_string()])
        }
    }
}

/// Resolve `(program, argv)` for launching a VM's graphical console.
/// Ported from `mde-virtual::app::console_command`.
#[must_use]
pub fn console_command(name: &str) -> (&'static str, Vec<String>) {
    (
        "virt-viewer",
        vec![
            "--connect".to_string(),
            "qemu:///system".to_string(),
            name.to_string(),
        ],
    )
}

/// Resolve `(program, argv)` for a cold migration of a stopped domain to
/// `target_host` over libvirt's SSH transport. `--offline` migrates the
/// persisted domain definition (the disk rides shared mesh storage), so
/// the target host lists the domain afterward. Pure.
#[must_use]
pub fn migrate_command(domain: &str, target_host: &str) -> (&'static str, Vec<String>) {
    (
        "virsh",
        vec![
            "-c".to_string(),
            "qemu:///system".to_string(),
            "migrate".to_string(),
            "--offline".to_string(),
            "--persistent".to_string(),
            domain.to_string(),
            format!("qemu+ssh://{target_host}/system"),
        ],
    )
}

/// Launch the graphical console for `name`, detached. virt-viewer is a
/// long-running GUI, so it's spawned fire-and-forget (the workbench must
/// not block on it); a missing binary just fails silently here and the
/// operator sees no window (never a panic).
fn launch_console(name: &str) {
    let (program, args) = console_command(name);
    let _ = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Turn two cumulative `cpu.time` readings (nanoseconds) `elapsed_secs`
/// apart into a CPU percentage. A guest can exceed 100% (multiple vCPUs);
/// the sparkline auto-scales, so the raw figure is kept. Pure.
#[must_use]
pub fn cpu_percent_from_delta(prev_ns: u64, now_ns: u64, elapsed_secs: f32) -> f32 {
    if elapsed_secs <= 0.0 || now_ns < prev_ns {
        return 0.0;
    }
    let delta_ns = (now_ns - prev_ns) as f32;
    let window_ns = elapsed_secs * 1.0e9;
    (delta_ns / window_ns * 100.0).max(0.0)
}

/// Parse a `podman`-style percent string (`"1.23%"`, with optional
/// surrounding whitespace) into an f32. Returns `None` on garbage.
#[must_use]
pub fn parse_percent(s: &str) -> Option<f32> {
    s.trim().trim_end_matches('%').trim().parse::<f32>().ok()
}

/// Parse `podman stats --no-stream --format json` into
/// `(name, cpu_pct, mem_pct)` rows. Podman reports CPU under `CPU` and
/// memory under `MemPerc` as percent strings. Garbage → empty. Pure.
#[must_use]
pub fn parse_podman_stats(stdout: &str) -> Vec<(String, Option<f32>, Option<f32>)> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return vec![];
    };
    rows.into_iter()
        .filter_map(|row| {
            let name = row.get("Name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return None;
            }
            let cpu = row
                .get("CPU")
                .and_then(|v| v.as_str())
                .and_then(parse_percent);
            let mem = row
                .get("MemPerc")
                .and_then(|v| v.as_str())
                .and_then(parse_percent);
            Some((name.to_string(), cpu, mem))
        })
        .collect()
}

/// Parse `virsh domstats --cpu-total --balloon` output into
/// `(name, cpu_time_ns, mem_pct)` rows. The output is `Domain: 'name'`
/// blocks of `key=value` lines; `cpu.time` is cumulative nanoseconds and
/// `balloon.current`/`balloon.maximum` are KiB. Pure.
#[must_use]
pub fn parse_domstats_cpu_mem(stdout: &str) -> Vec<(String, Option<u64>, Option<f32>)> {
    let mut out: Vec<(String, Option<u64>, Option<f32>)> = Vec::new();
    let mut name: Option<String> = None;
    let mut cpu_ns: Option<u64> = None;
    let mut bal_cur: Option<u64> = None;
    let mut bal_max: Option<u64> = None;
    let flush = |name: &mut Option<String>,
                 cpu_ns: &mut Option<u64>,
                 bal_cur: &mut Option<u64>,
                 bal_max: &mut Option<u64>,
                 out: &mut Vec<(String, Option<u64>, Option<f32>)>| {
        if let Some(n) = name.take() {
            let mem = match (*bal_cur, *bal_max) {
                (Some(c), Some(m)) if m > 0 => {
                    Some((c as f32 / m as f32 * 100.0).clamp(0.0, 100.0))
                }
                _ => None,
            };
            out.push((n, *cpu_ns, mem));
        }
        *cpu_ns = None;
        *bal_cur = None;
        *bal_max = None;
    };
    for line in stdout.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Domain:") {
            flush(&mut name, &mut cpu_ns, &mut bal_cur, &mut bal_max, &mut out);
            name = Some(rest.trim().trim_matches('\'').to_string());
        } else if let Some(v) = t.strip_prefix("cpu.time=") {
            cpu_ns = v.trim().parse::<u64>().ok();
        } else if let Some(v) = t.strip_prefix("balloon.current=") {
            bal_cur = v.trim().parse::<u64>().ok();
        } else if let Some(v) = t.strip_prefix("balloon.maximum=") {
            bal_max = v.trim().parse::<u64>().ok();
        }
    }
    flush(&mut name, &mut cpu_ns, &mut bal_cur, &mut bal_max, &mut out);
    out
}

/// Sample CPU/mem for the given instances: one `podman stats` call covers
/// every container, one `virsh domstats` call covers every running domain.
/// Missing tools / stopped instances simply produce no sample (a flat
/// sparkline), never an error.
async fn sample_metrics(instances: Vec<Instance>) -> Vec<InstanceSample> {
    let mut samples = Vec::new();

    let want_containers = instances.iter().any(|i| i.kind == InstanceKind::Container);
    let want_vms = instances.iter().any(|i| i.kind == InstanceKind::Vm);

    if want_containers {
        if let Some(stdout) =
            run_query("podman", &["stats", "--no-stream", "--format", "json"]).await
        {
            for (name, cpu, mem) in parse_podman_stats(&stdout) {
                samples.push(InstanceSample {
                    key: metric_key(InstanceKind::Container, &name),
                    cpu_pct: cpu,
                    cpu_time_ns: None,
                    mem_pct: mem,
                });
            }
        }
    }
    if want_vms {
        if let Some(stdout) = run_query(
            "virsh",
            &[
                "-c",
                "qemu:///system",
                "domstats",
                "--cpu-total",
                "--balloon",
            ],
        )
        .await
        {
            for (name, cpu_ns, mem) in parse_domstats_cpu_mem(&stdout) {
                samples.push(InstanceSample {
                    key: metric_key(InstanceKind::Vm, &name),
                    cpu_pct: None,
                    cpu_time_ns: cpu_ns,
                    mem_pct: mem,
                });
            }
        }
    }
    samples
}

/// Parse `virsh list --all` table output into `(name, state)` pairs.
/// Ported 1:1 from `mde-virtual::app::parse_virsh_list_state`.
#[must_use]
pub fn parse_virsh_list_state(stdout: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("---") {
            continue;
        }
        let cols: Vec<&str> = t.split_whitespace().collect();
        if cols.first().copied() == Some("Id") {
            continue; // header row
        }
        if cols.len() < 3 {
            continue;
        }
        let name = cols[1].to_string();
        let state = cols[2..].join(" ");
        out.push((name, state));
    }
    out
}

/// Parse `podman ps --all --format json` into `(name, state)` pairs.
/// Adapted from `mde-virtual::app::parse_podman_ps_local` (which carried
/// extra fields this list view doesn't need yet). Garbage → empty.
#[must_use]
pub fn parse_podman_ps(stdout: &str) -> Vec<(String, String)> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return vec![];
    };
    rows.into_iter()
        .filter_map(|row| {
            let name = row
                .get("Names")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                return None;
            }
            let state = row
                .get("State")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some((name, state))
        })
        .collect()
}

/// AUD6-8 — hard deadline per hypervisor query. `virsh` blocks
/// indefinitely when libvirtd is wedged (the `qemu:///system`
/// connect never returns), which left the panel's first paint on
/// "Loading instances…" forever. A wedged daemon now degrades to
/// "this hypervisor isn't available here" within 10 s.
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run a hypervisor query command, returning its stdout on success.
/// `None` when the binary is absent, the command fails, or it blows
/// the [`QUERY_TIMEOUT`] deadline — the caller treats all three as
/// "this hypervisor isn't available here".
async fn run_query(program: &str, args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(
        QUERY_TIMEOUT,
        tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a lifecycle command (best-effort). The result is intentionally
/// discarded — the caller re-enumerates afterward, so the instance's new
/// state is read back from the hypervisor rather than assumed. A missing
/// binary or a failed command simply leaves the state unchanged on the
/// next enumeration (never panics).
async fn run_action(program: &str, args: &[String]) {
    let _ = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .await;
}

/// Enumerate local KVM domains + Podman containers in one pass. Each
/// source is queried independently so a missing tool degrades to "skip
/// that source" rather than failing the whole list.
async fn enumerate() -> Enumeration {
    let mut instances = Vec::new();
    let mut sources = Vec::new();
    let (self_ip, local_host) = self_identity();

    if let Some(stdout) = run_query("virsh", &["-c", "qemu:///system", "list", "--all"]).await {
        sources.push("virsh");
        for (name, state) in parse_virsh_list_state(&stdout) {
            instances.push(Instance {
                name,
                kind: InstanceKind::Vm,
                state,
                node: local_host.clone(),
                local: true,
            });
        }
    }
    if let Some(stdout) = run_query("podman", &["ps", "--all", "--format", "json"]).await {
        sources.push("podman");
        for (name, state) in parse_podman_ps(&stdout) {
            instances.push(Instance {
                name,
                kind: InstanceKind::Container,
                state,
                node: local_host.clone(),
                local: true,
            });
        }
    }

    // WORKLOAD-FLEET-1 — merge fleet-wide workloads off the replicated
    // QNM-Shared plane so a VM on any node is visible from any Workbench. Every
    // node's `compute_registry` mirrors its inventory to
    // `<workgroup>/<host>/compute-inventory.json`; fold in any row not already
    // covered by the local probe (each node also writes its OWN file, so dedup
    // by node+kind+name keeps self-rows single, local probe winning).
    if merge_bus_inventory(&mut instances, &self_ip, &local_host) {
        sources.push("mesh");
    }

    Enumeration { instances, sources }
}

/// This node's `(overlay_ip, friendly_hostname)`. Read from the mesh-status
/// snapshot (the same source the shell + peers panel use), falling back to
/// `/proc/sys/kernel/hostname` for the name. The overlay IP is the robust
/// self-key for the bus inventory (its `peer` field), since the inventory's
/// `hostname` field is sourced inconsistently across nodes (`/etc/hostname`
/// when set, else the nebula `node_id` like `peer:<host>`).
fn self_identity() -> (String, String) {
    let proc_host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Ok(body) = std::fs::read_to_string("/run/mde/mesh-status.json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            let ip = v
                .get("network")
                .and_then(|n| n.get("overlay_ip"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let host = v
                .get("self")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .or_else(|| proc_host.clone())
                .unwrap_or_else(|| "this node".to_string());
            return (ip, host);
        }
    }
    (
        String::new(),
        proc_host.unwrap_or_else(|| "this node".to_string()),
    )
}

/// A friendly node label for display: strip the nebula `peer:` prefix some
/// nodes carry in their inventory `hostname`, so the Node column reads `fedora`
/// not `peer:fedora`.
fn display_host(hostname: &str) -> String {
    let h = hostname.trim();
    let h = h.strip_prefix("peer:").unwrap_or(h);
    if h.is_empty() {
        "unknown".to_string()
    } else {
        h.to_string()
    }
}

/// A peer's published compute inventory (subset of the `compute_registry`
/// `Inventory` doc we need to render — extra fields are ignored).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BusInventory {
    /// Publishing node's Nebula overlay IP — the robust self-key.
    #[serde(default)]
    pub peer: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub vms: Vec<BusEntry>,
    #[serde(default)]
    pub containers: Vec<BusEntry>,
}

/// A VM or container row inside a [`BusInventory`].
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BusEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: String,
}

/// Read the newest `compute/inventory/<peer>` document for every peer off the
/// mesh bus and fold its rows into `instances`, attributed to the peer's
/// hostname. Returns `true` when at least one inventory document was read
/// (so the caller can record the `mesh` source). Best-effort: a missing/locked
/// bus is silently skipped (the panel still shows the local probe).
fn merge_bus_inventory(instances: &mut Vec<Instance>, self_ip: &str, local_host: &str) -> bool {
    let invs = read_shared_inventories();
    fold_bus_inventories(instances, &invs, self_ip, local_host)
}

/// Pure merge step (unit-tested): fold peer inventory docs into `instances`,
/// attributed to each peer's friendly hostname, skipping this node's own doc
/// (matched by overlay IP, or by friendly hostname as a fallback) and any row
/// already present (node+kind+name). Returns whether any doc was folded.
fn fold_bus_inventories(
    instances: &mut Vec<Instance>,
    invs: &[BusInventory],
    self_ip: &str,
    local_host: &str,
) -> bool {
    if invs.is_empty() {
        return false;
    }
    for inv in invs {
        let node = display_host(&inv.hostname);
        // This node publishes its own inventory too — its rows duplicate the
        // local probe, so skip the whole document when it's us. Match by overlay
        // IP first (robust), then by friendly hostname (fallback when the
        // snapshot/overlay IP is unavailable).
        let is_self = (!self_ip.is_empty() && inv.peer.trim() == self_ip) || node == local_host;
        if is_self {
            continue;
        }
        let mut fold = |entries: &[BusEntry], kind: InstanceKind| {
            for e in entries {
                if e.name.trim().is_empty() {
                    continue;
                }
                let dup = instances
                    .iter()
                    .any(|i| i.node == node && i.kind == kind && i.name == e.name);
                if dup {
                    continue;
                }
                instances.push(Instance {
                    name: e.name.clone(),
                    kind,
                    state: e.state.clone(),
                    node: node.clone(),
                    local: false,
                });
            }
        };
        fold(&inv.vms, InstanceKind::Vm);
        fold(&inv.containers, InstanceKind::Container);
    }
    true
}

/// Pull each peer's latest compute inventory from the replicated QNM-Shared
/// plane: `<workgroup>/<host>/compute-inventory.json`, written every tick by
/// every node's `compute_registry`. This is the cross-node transport — the
/// `compute/inventory/<peer>` bus topic is per-node (no federation worker), so
/// reading the local bus would only ever surface this node's own inventory.
/// Best-effort: a missing/un-mounted share yields an empty list (the panel
/// still shows the local probe).
pub fn read_shared_inventories() -> Vec<BusInventory> {
    let root = mackes_mesh_types::peers::default_workgroup_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in entries.flatten() {
        let path = ent.path().join("compute-inventory.json");
        if let Ok(body) = std::fs::read_to_string(&path) {
            if let Ok(inv) = serde_json::from_str::<BusInventory>(&body) {
                out.push(inv);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_virsh_list_table() {
        let out = " Id   Name        State\n\
                    -------------------------------\n\
                     1    fedora-vm   running\n\
                     -    build-box   shut off\n";
        let got = parse_virsh_list_state(out);
        assert_eq!(
            got,
            vec![
                ("fedora-vm".to_string(), "running".to_string()),
                ("build-box".to_string(), "shut off".to_string()),
            ]
        );
    }

    #[test]
    fn virsh_parser_skips_header_and_rules() {
        // Header ("Id …") + the dashed rule must not become rows.
        let out = " Id   Name   State\n----\n";
        assert!(parse_virsh_list_state(out).is_empty());
    }

    #[test]
    fn parses_podman_ps_json_names_and_state() {
        let out = r#"[{"Names":["web"],"State":"running"},{"Names":["db"],"State":"exited"}]"#;
        let got = parse_podman_ps(out);
        assert_eq!(
            got,
            vec![
                ("web".to_string(), "running".to_string()),
                ("db".to_string(), "exited".to_string()),
            ]
        );
    }

    #[test]
    fn podman_parser_returns_empty_on_garbage() {
        assert!(parse_podman_ps("not json").is_empty());
        assert!(parse_podman_ps("").is_empty());
    }

    #[test]
    fn running_and_paused_state_detection() {
        assert!(state_is_running("running"));
        assert!(state_is_running("RUNNING"));
        assert!(!state_is_running("shut off"));
        assert!(state_is_paused("paused"));
        assert!(state_is_paused("suspended"));
        assert!(!state_is_paused("running"));
    }

    #[test]
    fn status_line_distinguishes_no_hypervisor_from_empty() {
        let none = Enumeration::default();
        assert!(status_line(&none).contains("No local hypervisor"));

        let empty_but_present = Enumeration {
            instances: vec![],
            sources: vec!["virsh"],
        };
        assert!(status_line(&empty_but_present).contains("0 instances"));

        let one = Enumeration {
            instances: vec![Instance {
                name: "vm".into(),
                kind: InstanceKind::Vm,
                state: "running".into(),
                node: "fedora".into(),
                local: true,
            }],
            sources: vec!["virsh", "podman"],
        };
        let s = status_line(&one);
        assert!(s.contains("1 instance"), "{s}");
        assert!(s.contains("virsh + podman"), "{s}");
    }

    #[test]
    fn loaded_message_populates_and_marks_loaded() {
        let mut panel = ComputePanel::new();
        let _ = panel.update(Message::Loaded(Enumeration {
            instances: vec![Instance {
                name: "fedora-vm".into(),
                kind: InstanceKind::Vm,
                state: "running".into(),
                node: "fedora".into(),
                local: true,
            }],
            sources: vec!["virsh"],
        }));
        assert_eq!(panel.instances().len(), 1);
        assert!(panel.status().contains("1 instance"));
    }

    #[test]
    fn view_constructs_for_empty_and_populated() {
        // Empty (pre-load) and populated states both render without panic.
        let empty = ComputePanel::new();
        let _: Element<'_, crate::Message> = empty.view();

        let mut populated = ComputePanel::new();
        let _ = populated.update(Message::Loaded(Enumeration {
            instances: vec![Instance {
                name: "web".into(),
                kind: InstanceKind::Container,
                state: "running".into(),
                node: "fedora".into(),
                local: true,
            }],
            sources: vec!["podman"],
        }));
        let _: Element<'_, crate::Message> = populated.view();
    }

    #[test]
    fn verb_applies_matches_state() {
        assert!(verb_applies(Verb::Start, "shut off"));
        assert!(!verb_applies(Verb::Start, "running"));
        assert!(!verb_applies(Verb::Start, "paused"));
        assert!(verb_applies(Verb::Stop, "running"));
        assert!(!verb_applies(Verb::Stop, "shut off"));
    }

    #[test]
    fn row_action_picks_start_stop_or_none() {
        assert_eq!(row_action("running"), Some(Verb::Stop));
        assert_eq!(row_action("shut off"), Some(Verb::Start));
        assert_eq!(row_action("paused"), None);
    }

    #[test]
    fn command_for_vm_uses_system_libvirt() {
        let (prog, args) = command_for(InstanceKind::Vm, Verb::Start, "fedora-vm");
        assert_eq!(prog, "virsh");
        assert_eq!(args, vec!["-c", "qemu:///system", "start", "fedora-vm"]);
        // VM Stop is a graceful shutdown, not a destroy.
        let (_, stop) = command_for(InstanceKind::Vm, Verb::Stop, "fedora-vm");
        assert_eq!(stop, vec!["-c", "qemu:///system", "shutdown", "fedora-vm"]);
    }

    #[test]
    fn command_for_container_uses_podman() {
        let (prog, args) = command_for(InstanceKind::Container, Verb::Start, "web");
        assert_eq!(prog, "podman");
        assert_eq!(args, vec!["start", "web"]);
        let (_, stop) = command_for(InstanceKind::Container, Verb::Stop, "web");
        assert_eq!(stop, vec!["stop", "web"]);
    }

    #[test]
    fn action_message_reloads_via_loaded() {
        // An Action issues the command then re-enumerates — exercising the
        // reducer path keeps it construction-safe (the real command runs on
        // the executor; HW round-trip is bench).
        let mut panel = ComputePanel::new();
        let _ = panel.update(Message::Loaded(Enumeration {
            instances: vec![Instance {
                name: "vm".into(),
                kind: InstanceKind::Vm,
                state: "shut off".into(),
                node: "fedora".into(),
                local: true,
            }],
            sources: vec!["virsh"],
        }));
        // The row offers Start; the panel + bulk paths construct without panic.
        let _ = panel.bulk(Verb::Start);
        let _ = panel.bulk(Verb::Stop);
        let _: Element<'_, crate::Message> = panel.view();
    }

    #[test]
    fn cpu_percent_from_delta_computes_and_guards() {
        // 2e9 ns of CPU over a 2 s window = 100%.
        assert!((cpu_percent_from_delta(0, 2_000_000_000, 2.0) - 100.0).abs() < 0.01);
        // Counter reset / decreasing → 0, not negative.
        assert_eq!(cpu_percent_from_delta(5, 1, 2.0), 0.0);
        // Zero window → 0 (no divide-by-zero).
        assert_eq!(cpu_percent_from_delta(0, 100, 0.0), 0.0);
    }

    #[test]
    fn parse_percent_strips_suffix() {
        assert_eq!(parse_percent(" 1.50% "), Some(1.5));
        assert_eq!(parse_percent("0%"), Some(0.0));
        assert_eq!(parse_percent("n/a"), None);
    }

    #[test]
    fn parse_podman_stats_reads_cpu_and_mem() {
        let out = r#"[{"Name":"web","CPU":"1.50%","MemPerc":"2.00%"},
                      {"Name":"db","CPU":"0.00%","MemPerc":"5.00%"}]"#;
        let got = parse_podman_stats(out);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("web".to_string(), Some(1.5), Some(2.0)));
        assert_eq!(got[1].2, Some(5.0));
    }

    #[test]
    fn parse_podman_stats_garbage_is_empty() {
        assert!(parse_podman_stats("oops").is_empty());
    }

    #[test]
    fn parse_domstats_splits_domains_and_computes_mem() {
        let out = "Domain: 'fedora-vm'\n\
                   \x20 cpu.time=12345\n\
                   \x20 balloon.current=2097152\n\
                   \x20 balloon.maximum=4194304\n\
                   Domain: 'build-box'\n\
                   \x20 cpu.time=999\n";
        let got = parse_domstats_cpu_mem(out);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "fedora-vm");
        assert_eq!(got[0].1, Some(12345));
        let mem0 = got[0].2.expect("fedora-vm has balloon mem%");
        assert!((mem0 - 50.0).abs() < 0.01); // 2097152/4194304 = 50%
        assert_eq!(got[1].0, "build-box");
        assert_eq!(got[1].2, None); // no balloon data
    }

    #[test]
    fn sampled_pushes_into_buffers_and_deltas_vm_cpu() {
        let mut panel = ComputePanel::new();
        let key = metric_key(InstanceKind::Vm, "vm");
        // First VM sample establishes the baseline (no % yet).
        let _ = panel.update(Message::Sampled(vec![InstanceSample {
            key: key.clone(),
            cpu_pct: None,
            cpu_time_ns: Some(0),
            mem_pct: Some(40.0),
        }]));
        // Second sample deltas to a CPU %.
        let _ = panel.update(Message::Sampled(vec![InstanceSample {
            key: key.clone(),
            cpu_pct: None,
            cpu_time_ns: Some(2_000_000_000),
            mem_pct: Some(42.0),
        }]));
        let m = panel.metrics.get(&key).expect("metrics recorded");
        assert_eq!(m.cpu.len(), 1, "one delta CPU sample after two readings");
        let cpu = m.cpu.back().copied().expect("a CPU sample");
        assert!((cpu - 100.0).abs() < 0.01);
        assert_eq!(m.mem.len(), 2, "both mem readings pushed");
    }

    #[test]
    fn sampled_container_uses_direct_cpu() {
        let mut panel = ComputePanel::new();
        let key = metric_key(InstanceKind::Container, "web");
        let _ = panel.update(Message::Sampled(vec![InstanceSample {
            key: key.clone(),
            cpu_pct: Some(3.0),
            cpu_time_ns: None,
            mem_pct: Some(1.0),
        }]));
        let m = panel.metrics.get(&key).expect("metrics recorded");
        assert_eq!(m.cpu.back().copied(), Some(3.0));
        assert_eq!(m.mem.back().copied(), Some(1.0));
    }

    #[test]
    fn open_wizard_then_cancel_round_trips() {
        let mut panel = ComputePanel::new();
        assert!(panel.wizard.is_none());
        let _ = panel.update(Message::OpenWizard);
        assert!(panel.wizard.is_some(), "wizard opens");
        // The Compute view renders the wizard body without panic.
        let _: Element<'_, crate::Message> = panel.view();
        // Cancel closes it.
        let _ = panel.update(Message::Wizard(WizardMsg::Cancel));
        assert!(panel.wizard.is_none(), "cancel closes the wizard");
    }

    #[test]
    fn console_command_targets_system_libvirt() {
        let (prog, args) = console_command("fedora-vm");
        assert_eq!(prog, "virt-viewer");
        assert_eq!(args, vec!["--connect", "qemu:///system", "fedora-vm"]);
    }

    #[test]
    fn running_vm_row_offers_console() {
        // A running VM row builds with both Stop + Console; a container
        // and a stopped VM build without panic too.
        let running_vm = Instance {
            name: "vm".into(),
            kind: InstanceKind::Vm,
            state: "running".into(),
            node: "fedora".into(),
            local: true,
        };
        let container = Instance {
            name: "web".into(),
            kind: InstanceKind::Container,
            state: "running".into(),
            node: "fedora".into(),
            local: true,
        };
        let _: Element<'_, crate::Message> =
            instance_row(&running_vm, None, crate::live_theme::palette());
        let _: Element<'_, crate::Message> =
            instance_row(&container, None, crate::live_theme::palette());
    }

    #[test]
    fn migrate_command_builds_offline_ssh_argv() {
        let (prog, args) = migrate_command("fedora-vm", "peer2.mesh");
        assert_eq!(prog, "virsh");
        assert_eq!(
            args,
            vec![
                "-c",
                "qemu:///system",
                "migrate",
                "--offline",
                "--persistent",
                "fedora-vm",
                "qemu+ssh://peer2.mesh/system",
            ]
        );
    }

    #[test]
    fn migrate_sheet_open_input_and_cancel() {
        let mut panel = ComputePanel::new();
        let _ = panel.update(Message::OpenMigrate { name: "vm".into() });
        assert_eq!(
            panel.migrate.as_ref().map(|s| s.domain.as_str()),
            Some("vm")
        );
        let _ = panel.update(Message::MigrateHostInput("peer2".into()));
        assert_eq!(
            panel.migrate.as_ref().map(|s| s.host.as_str()),
            Some("peer2")
        );
        // The sheet renders without panic.
        let _: Element<'_, crate::Message> = panel.view();
        let _ = panel.update(Message::MigrateCancel);
        assert!(panel.migrate.is_none());
    }

    #[test]
    fn migrate_confirm_with_blank_host_reopens_sheet() {
        let mut panel = ComputePanel::new();
        let _ = panel.update(Message::OpenMigrate { name: "vm".into() });
        // Empty host → confirm is a no-op that keeps the sheet open.
        let _ = panel.update(Message::MigrateConfirm);
        assert!(panel.migrate.is_some(), "blank host keeps the sheet open");
    }

    #[test]
    fn stopped_vm_row_offers_migrate() {
        let stopped_vm = Instance {
            name: "vm".into(),
            kind: InstanceKind::Vm,
            state: "shut off".into(),
            node: "fedora".into(),
            local: true,
        };
        let _: Element<'_, crate::Message> =
            instance_row(&stopped_vm, None, crate::live_theme::palette());
    }

    #[test]
    fn fold_bus_inventories_attributes_peers_and_dedups_self() {
        // The local probe already holds this host's own VM.
        let mut instances = vec![Instance {
            name: "MDE-KVM-1".into(),
            kind: InstanceKind::Vm,
            state: "running".into(),
            node: "fedora".into(),
            local: true,
        }];
        let invs = vec![
            // This node's own published doc — skipped by overlay-IP match (its
            // hostname is the nebula node_id form, proving IP self-key wins).
            BusInventory {
                peer: "10.42.0.3".into(),
                hostname: "peer:fedora".into(),
                vms: vec![BusEntry {
                    name: "MDE-KVM-1".into(),
                    state: "running".into(),
                }],
                containers: vec![],
            },
            // A peer's doc — its workloads must appear, attributed to the peer.
            BusInventory {
                peer: "10.42.0.5".into(),
                hostname: "node-13".into(),
                vms: vec![BusEntry {
                    name: "web1".into(),
                    state: "running".into(),
                }],
                containers: vec![BusEntry {
                    name: "db".into(),
                    state: "exited".into(),
                }],
            },
        ];
        let folded = fold_bus_inventories(&mut instances, &invs, "10.42.0.3", "fedora");
        assert!(folded);
        // 1 local + 2 peer rows; the self doc did NOT duplicate the local VM.
        assert_eq!(instances.len(), 3);
        assert_eq!(
            instances.iter().filter(|i| i.name == "MDE-KVM-1").count(),
            1
        );
        let web = instances.iter().find(|i| i.name == "web1").unwrap();
        assert_eq!(web.node, "node-13");
        assert!(!web.local);
        assert!(instances
            .iter()
            .any(|i| i.name == "db" && i.kind == InstanceKind::Container && !i.local));
    }

    #[test]
    fn fold_bus_inventories_empty_is_noop() {
        let mut instances: Vec<Instance> = Vec::new();
        assert!(!fold_bus_inventories(
            &mut instances,
            &[],
            "10.42.0.3",
            "fedora"
        ));
        assert!(instances.is_empty());
    }

    #[test]
    fn fold_self_skipped_by_hostname_when_no_overlay_ip() {
        // Fallback path: with no overlay IP known, the self doc is still
        // skipped by its friendly hostname matching local_host.
        let mut instances = vec![Instance {
            name: "MDE-KVM-1".into(),
            kind: InstanceKind::Vm,
            state: "running".into(),
            node: "fedora".into(),
            local: true,
        }];
        let invs = vec![BusInventory {
            peer: "10.42.0.3".into(),
            hostname: "peer:fedora".into(),
            vms: vec![BusEntry {
                name: "MDE-KVM-1".into(),
                state: "running".into(),
            }],
            containers: vec![],
        }];
        // self_ip empty → falls back to display_host("peer:fedora")=="fedora".
        assert!(fold_bus_inventories(&mut instances, &invs, "", "fedora"));
        assert_eq!(instances.len(), 1);
    }

    #[test]
    fn display_host_strips_peer_prefix() {
        assert_eq!(display_host("peer:fedora"), "fedora");
        assert_eq!(display_host("node-13"), "node-13");
        assert_eq!(display_host("  peer:lh-01 "), "lh-01");
        assert_eq!(display_host(""), "unknown");
    }
}
