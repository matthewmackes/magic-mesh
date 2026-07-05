//! The **Instances** surface — this workstation's local cloud-hypervisor VMs.
//!
//! E12 "Quasar" runs local VM desktops on **cloud-hypervisor** (lock 11); the
//! `mde-kvm` crate is the broker between the shell and a `cloud-hypervisor`
//! process, driving the `create`/`boot`/`shutdown` lifecycle over its
//! HTTP-on-a-unix-socket API. This panel is that broker's **first shell caller** —
//! it gives `mde-kvm`'s surface a reachable home, the same reachability fix E12-5a
//! did for the two VDI decoder crates.
//!
//! The roster is the operator's local VM definitions; each row shows the VM's
//! resources and its **dual-homed** NICs (lock 19: every guest is its own Nebula
//! mesh peer *and* carries a LAN-bridged NIC). Create / Boot / Shutdown drive
//! `mde-kvm`'s real lifecycle verbs.
//!
//! ## Gated: the live VM
//!
//! `mde-kvm` only *talks to* an already-running `cloud-hypervisor --api-socket …`;
//! launching that process (with KVM + a golden image) is the integration-gated
//! layer. So on any box without a live VMM the lifecycle verbs fail at the
//! transport with a typed [`KvmError::Connect`] — which this panel surfaces as an
//! honest inline "cloud-hypervisor isn't running … gated" message and a `gated`
//! state pip. Never a crash, never a fake "running" (§7).

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{Motion, Style};

use mde_kvm::{api_socket_path, KvmError, Nic, NicRole, Vm, VmSpec};

/// A filled-circle status dot — the shared glyph the datacenter rows / chrome pip
/// / This Node / Network use, so a VM state pip reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

/// One lifecycle verb the panel drives against a local VM.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    /// Define the guest in cloud-hypervisor (`PUT /vm.create`).
    Create,
    /// Start the defined guest (`PUT /vm.boot`).
    Boot,
    /// Stop the running guest (`PUT /vm.shutdown`).
    Shutdown,
}

impl Op {
    /// The verb tag, for operator-facing error copy.
    const fn verb(self) -> &'static str {
        match self {
            Op::Create => "create",
            Op::Boot => "boot",
            Op::Shutdown => "shutdown",
        }
    }

    /// The runtime state a *successful* op leaves the VM in.
    const fn ok_state(self) -> VmRunState {
        match self {
            Op::Create => VmRunState::Defined,
            Op::Boot => VmRunState::Running,
            Op::Shutdown => VmRunState::Stopped,
        }
    }
}

/// The broker seam between the panel and `mde-kvm`'s per-VM cloud-hypervisor
/// lifecycle. The production impl ([`ChBroker`]) dials each VM's api-socket through
/// `mde-kvm`'s `UnixSocketTransport` (its injectable `ChTransport` seam); tests
/// inject a recording fake so the drive path is exercised without a live VMM.
trait VmBroker {
    /// Run one lifecycle verb against the VM described by `spec`.
    fn run(&self, spec: &VmSpec, op: Op) -> Result<(), KvmError>;
}

/// The production broker: for each op it binds a fresh [`Vm`] to the VM's
/// cloud-hypervisor api-socket ([`api_socket_path`]) and drives the matching verb.
/// One `cloud-hypervisor` process (hence one api-socket) hosts one guest, so the
/// handle is derived per-op from the VM name rather than held long-lived.
struct ChBroker;

impl VmBroker for ChBroker {
    fn run(&self, spec: &VmSpec, op: Op) -> Result<(), KvmError> {
        // `Vm::connect` binds the real `UnixSocketTransport` (mde-kvm's ChTransport
        // seam). With no `cloud-hypervisor` listening on the socket the very first
        // request fails with `KvmError::Connect` — the honest gated path.
        let vm = Vm::connect(api_socket_path(&spec.name));
        match op {
            Op::Create => vm.create(spec),
            Op::Boot => vm.boot(),
            Op::Shutdown => vm.shutdown(),
        }
    }
}

/// A local VM's last-known runtime state, folded from the last lifecycle op the
/// operator drove — never a fabricated metric (§7).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
enum VmRunState {
    /// Defined locally; not yet reconciled against a live VMM.
    #[default]
    Defined,
    /// cloud-hypervisor reported (or the last op left) the guest running.
    Running,
    /// The guest is stopped (shut down after a boot).
    Stopped,
    /// The last op couldn't reach cloud-hypervisor — the live VMM is the gated
    /// layer. Honest: not a fake "running".
    Gated,
}

impl VmRunState {
    /// The state pip colour + label for the roster row.
    const fn pip(&self) -> (Color32, &'static str) {
        match self {
            VmRunState::Running => (Style::OK, "running"),
            VmRunState::Stopped => (Style::TEXT_DIM, "stopped"),
            VmRunState::Defined => (Style::TEXT_DIM, "defined"),
            VmRunState::Gated => (Style::WARN, "gated — no VMM"),
        }
    }
}

/// One local VM: its `mde-kvm` [`VmSpec`] (the dual-homed NICs live here) plus the
/// last-known runtime state.
struct LocalVm {
    /// The broker spec — the single source of the VM's name, resources, and NICs.
    spec: VmSpec,
    /// Last-known runtime state (folded from the last driven op).
    state: VmRunState,
}

/// The "New VM" form's raw text fields (parsed + validated on Create).
#[derive(Default)]
struct CreateForm {
    name: String,
    vcpus: String,
    mem_mib: String,
    disk: String,
    /// Inline validation error for the open form (honest; never a panic).
    error: Option<String>,
}

impl CreateForm {
    /// Parse + validate into a dual-homed [`VmSpec`], or a human-readable message.
    /// Both NICs (lock 19: a Nebula mesh peer + a LAN bridge) are derived from the
    /// name, matching mde-kvm's `mvm-<name>-<role>` tap convention.
    fn to_spec(&self) -> Result<VmSpec, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("VM name is required.".to_string());
        }
        let vcpus: u8 = self
            .vcpus
            .trim()
            .parse()
            .map_err(|_| "vCPUs must be a whole number (1–255).".to_string())?;
        if vcpus == 0 {
            return Err("vCPUs must be at least 1.".to_string());
        }
        let mem_mib: u64 = self
            .mem_mib
            .trim()
            .parse()
            .map_err(|_| "RAM (MiB) must be a whole number.".to_string())?;
        if mem_mib == 0 {
            return Err("RAM (MiB) must be greater than 0.".to_string());
        }
        let disk = self.disk.trim();
        if disk.is_empty() {
            return Err("A running-disk path is required.".to_string());
        }
        Ok(VmSpec::new(name, vcpus, mem_mib, disk)
            .with_nic(Nic::mesh(format!("mvm-{name}-mesh")))
            .with_nic(Nic::lan(format!("mvm-{name}-lan"))))
    }
}

/// One collected UI action, applied after the render borrow ends (the egui idiom
/// datacenter uses) so the broker drive can take `&mut` state freely.
enum Action {
    /// Define a new VM from the validated form spec.
    Create(VmSpec),
    /// Drive a lifecycle verb against the VM at this roster index.
    Lifecycle { idx: usize, op: Op },
}

/// The Instances surface state: the local VM roster, the selected row, the broker
/// handle, and the create-form / inline-error context.
pub(crate) struct InstancesState {
    /// The operator's local VM definitions (empty until the first Create — the
    /// honest `EmptyState`, never demo data).
    vms: Vec<LocalVm>,
    /// The selected roster row, if any.
    selected: Option<usize>,
    /// The lifecycle broker (the production `ChBroker`; a fake in tests).
    broker: Box<dyn VmBroker>,
    /// Whether the inline "New VM" form is open.
    show_create: bool,
    /// The (single, one-open-at-a-time) create form's fields.
    form: CreateForm,
    /// The last lifecycle error, surfaced inline (honest — never a panic).
    last_error: Option<String>,
}

/// A read view of one VM's power state — what the System panel's Power section
/// renders for its per-VM power row (§6: the Instances roster, a second view).
pub(crate) struct VmPowerRow {
    /// The VM name.
    pub name: String,
    /// The operator-facing runtime state label.
    pub state: &'static str,
    /// Whether the VM is (last-known) running — picks Boot vs Shutdown affordance.
    pub running: bool,
    /// Whether the last op was gated (no live VMM) — toned honestly in the row.
    pub gated: bool,
}

impl InstancesState {
    /// The per-VM power rows for the System panel's Power section — the SAME roster
    /// the Instances surface drives, surfaced read-only for a second view (§6 /
    /// lock 12: per-VM power rows reuse the Instances broker verbs).
    pub(crate) fn power_rows(&self) -> Vec<VmPowerRow> {
        self.vms
            .iter()
            .map(|vm| VmPowerRow {
                name: vm.spec.name.clone(),
                state: vm.state.pip().1,
                running: vm.state == VmRunState::Running,
                gated: vm.state == VmRunState::Gated,
            })
            .collect()
    }

    /// Drive a VM power verb from the Power section, reusing the ONE Instances
    /// broker (`Op::Boot`/`Op::Shutdown`) — no reimplementation of VM power (§6).
    /// `boot` selects Boot; otherwise Shutdown. Folds the result into the roster +
    /// the shared inline error exactly like the Instances surface's own buttons.
    pub(crate) fn drive_power(&mut self, idx: usize, boot: bool) {
        let op = if boot { Op::Boot } else { Op::Shutdown };
        apply_action(
            &mut self.vms,
            &mut self.selected,
            &mut self.last_error,
            &*self.broker,
            Action::Lifecycle { idx, op },
        );
    }
}

impl Default for InstancesState {
    fn default() -> Self {
        Self {
            vms: Vec::new(),
            selected: None,
            broker: Box::new(ChBroker),
            show_create: false,
            form: CreateForm::default(),
            last_error: None,
        }
    }
}

/// Render the Instances surface into `ui`: a header + New-VM toggle, the local VM
/// roster (name · state · resources · dual-homed NICs) with Create / Boot /
/// Shutdown wired to `mde-kvm`, and an honest "No local VMs" `EmptyState` when empty.
pub(crate) fn instances_panel(ui: &mut egui::Ui, state: &mut InstancesState) {
    // MENUBAR-ALL — the shared top bar (INSTANCES). Its menus are the mouse twins of
    // the panel's own affordances (§6, one dispatch path): **Instance** toggles the
    // New-VM form, **Power** boots / shuts the selected VM through the SAME broker
    // verbs the roster rows drive. Handled before the field destructure so the apply
    // path can take `&mut state` freely; the separator sits it above the body.
    if let Some(action) = menubar::show(ui, state) {
        menubar::apply(state, action);
    }
    ui.separator();
    ui.add_space(Style::SP_S);

    // Disjoint field borrows so the render closures can hold several at once (the
    // egui idiom datacenter::show uses).
    let InstancesState {
        vms,
        selected,
        broker,
        show_create,
        form,
        last_error,
    } = state;

    // Header — title + the always-available New VM toggle.
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Local VMs")
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        let toggle = if *show_create {
            "Close"
        } else {
            "\u{FF0B} New VM"
        };
        if ui
            .button(RichText::new(toggle).size(Style::SMALL))
            .clicked()
        {
            *show_create = !*show_create;
            if *show_create {
                *form = CreateForm::default();
            }
        }
    });
    mde_egui::muted_note(
        ui,
        "cloud-hypervisor VMs on this workstation — each a dual-homed mesh peer.",
    );
    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_S);

    // Inline error banner (the gated / failed-op honest message).
    if let Some(err) = last_error.as_deref() {
        ui.colored_label(Style::DANGER, err);
        ui.add_space(Style::SP_S);
    }

    // Empty + not creating: the honest EmptyState fills the body (no pending here).
    if vms.is_empty() && !*show_create {
        crate::session::empty_state(
            ui,
            "No local VMs",
            "Define a cloud-hypervisor VM with New VM — it runs here as a dual-homed mesh peer.",
        );
        return;
    }

    // Collect at most one action this frame, applied after the render borrow ends.
    let mut pending: Option<Action> = None;

    // The New-VM form reveal eases in on the shared BASE curve (§4 — motion via
    // the shared table only; the form still mounts/unmounts on the toggle).
    let reveal = Motion::animate(
        ui.ctx(),
        "instances-create-form",
        *show_create,
        Motion::BASE,
    );

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if *show_create {
                ui.scope(|ui| {
                    ui.set_opacity(reveal);
                    // A small rise as the form settles in (the shell-expand idiom).
                    ui.add_space((1.0 - reveal) * Style::SP_S);
                    ui.group(|ui| show_create_form(ui, form, show_create, &mut pending));
                });
                ui.add_space(Style::SP_S);
            }
            for (idx, vm) in vms.iter().enumerate() {
                ui.group(|ui| show_vm(ui, idx, vm, selected, &mut pending));
                ui.add_space(Style::SP_S);
            }
        });

    if let Some(action) = pending {
        apply_action(vms, selected, last_error, &**broker, action);
    }
}

/// One roster row: a state pip, the selectable name, the resource line, the
/// dual-homed NICs, and the Boot / Shutdown affordances (targeted at this VM).
fn show_vm(
    ui: &mut egui::Ui,
    idx: usize,
    vm: &LocalVm,
    selected: &mut Option<usize>,
    pending: &mut Option<Action>,
) {
    let (color, label) = vm.state.pip();

    ui.horizontal(|ui| {
        ui.label(RichText::new(DOT).color(color).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        if ui
            .selectable_label(
                *selected == Some(idx),
                RichText::new(&vm.spec.name)
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            )
            .clicked()
        {
            *selected = Some(idx);
        }
        ui.add_space(Style::SP_S);
        ui.colored_label(color, RichText::new(label).size(Style::SMALL));
    });

    ui.indent((vm.spec.name.as_str(), "body"), |ui| {
        // Resources.
        mde_egui::muted_note(
            ui,
            format!(
                "{} vCPU · {} MiB · {}",
                vm.spec.vcpus,
                vm.spec.mem_mib,
                vm.spec.disk.display()
            ),
        );

        // Dual-homed NICs (lock 19).
        if vm.spec.nics.is_empty() {
            ui.colored_label(
                Style::WARN,
                RichText::new("no NICs — not dual-homed").size(Style::SMALL),
            );
        } else {
            for nic in &vm.spec.nics {
                show_nic(ui, nic);
            }
        }

        // Lifecycle affordances.
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("Boot").size(Style::SMALL))
                .clicked()
            {
                *pending = Some(Action::Lifecycle { idx, op: Op::Boot });
            }
            if ui
                .button(RichText::new("Shutdown").size(Style::SMALL))
                .clicked()
            {
                *pending = Some(Action::Lifecycle {
                    idx,
                    op: Op::Shutdown,
                });
            }
        });
    });
}

/// One dual-homed NIC line: the role (Nebula mesh peer / LAN bridge), the host tap
/// device, and the pinned MAC when set. The guest's live LAN address is DHCP-
/// assigned at boot (the gated live layer), so it is not fabricated here (§7).
fn show_nic(ui: &mut egui::Ui, nic: &Nic) {
    let (color, role) = match nic.role {
        NicRole::Mesh => (Style::ACCENT, "Nebula mesh peer"),
        NicRole::Lan => (Style::TEXT_DIM, "LAN bridge"),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("\u{2022}").color(color).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        ui.colored_label(color, RichText::new(role).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(ui, format!("tap {}", nic.tap));
        if let Some(mac) = &nic.mac {
            ui.add_space(Style::SP_XS);
            mde_egui::muted_note(ui, format!("· {mac}"));
        }
    });
}

/// The inline "New VM" form: name / vCPUs / RAM / disk, then Create (validates +
/// queues a spec) or Cancel. Both dual-homed NICs are derived from the name.
fn show_create_form(
    ui: &mut egui::Ui,
    form: &mut CreateForm,
    show_create: &mut bool,
    pending: &mut Option<Action>,
) {
    ui.label(
        RichText::new("New local VM")
            .color(Style::TEXT)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    form_field(ui, "Name", &mut form.name);
    form_field(ui, "vCPUs", &mut form.vcpus);
    form_field(ui, "RAM (MiB)", &mut form.mem_mib);
    form_field(ui, "Running disk", &mut form.disk);
    mde_egui::muted_note(
        ui,
        "Both NICs (Nebula mesh peer + LAN bridge) are derived from the name.",
    );

    if let Some(err) = form.error.as_deref() {
        ui.colored_label(Style::DANGER, RichText::new(err).size(Style::SMALL));
    }

    ui.horizontal(|ui| {
        if ui
            .button(RichText::new("Create").size(Style::SMALL))
            .clicked()
        {
            match form.to_spec() {
                Ok(spec) => {
                    *pending = Some(Action::Create(spec));
                    *show_create = false;
                }
                Err(e) => form.error = Some(e),
            }
        }
        if ui
            .button(RichText::new("Cancel").size(Style::SMALL))
            .clicked()
        {
            *show_create = false;
        }
    });
}

/// A labelled single-line text field, on the spacing grid (mirrors datacenter).
fn form_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(egui::TextEdit::singleline(value).desired_width(Style::SP_XL * 4.0));
    });
}

/// Apply one collected action: run the broker verb and fold its result into the
/// roster (a Create also appends the VM). Runs after the render borrow ends.
fn apply_action(
    vms: &mut Vec<LocalVm>,
    selected: &mut Option<usize>,
    last_error: &mut Option<String>,
    broker: &dyn VmBroker,
    action: Action,
) {
    match action {
        Action::Create(spec) => {
            // The definition is a real local fact regardless of the VMM's presence;
            // the folded state records whether it's live or gated.
            let state = fold(broker, last_error, &spec, Op::Create).unwrap_or_default();
            vms.push(LocalVm { spec, state });
            *selected = Some(vms.len() - 1);
        }
        Action::Lifecycle { idx, op } => {
            let Some(spec) = vms.get(idx).map(|v| v.spec.clone()) else {
                return;
            };
            if let Some(new_state) = fold(broker, last_error, &spec, op) {
                if let Some(vm) = vms.get_mut(idx) {
                    vm.state = new_state;
                }
            }
        }
    }
}

/// Run one broker op and map its result to a new [`VmRunState`], recording any
/// error inline. `None` ⇒ the state is unchanged (a non-gated error the op didn't
/// take). A gated ([`KvmError::Connect`]) failure is the honest "no VMM" path.
fn fold(
    broker: &dyn VmBroker,
    last_error: &mut Option<String>,
    spec: &VmSpec,
    op: Op,
) -> Option<VmRunState> {
    match broker.run(spec, op) {
        Ok(()) => {
            *last_error = None;
            Some(op.ok_state())
        }
        Err(KvmError::Connect(path, _)) => {
            *last_error = Some(format!(
                "cloud-hypervisor isn't running for '{}' ({}) — the live VM boot is gated.",
                spec.name,
                path.display()
            ));
            Some(VmRunState::Gated)
        }
        Err(e) => {
            *last_error = Some(format!("VM {} failed for '{}': {e}", op.verb(), spec.name));
            None
        }
    }
}

/// MENUBAR-ALL (Instances) — the shared top bar over the local VM broker.
///
/// Every item is the mouse twin of an affordance the panel already renders (§6, one
/// dispatch path through [`super::apply_action`] / the `show_create` toggle), never
/// a new behaviour and never a stub. **Instance → New VM…** flips the same inline
/// create form the header button toggles; **Power → Boot / Shutdown** drives the
/// selected roster VM through the SAME `mde-kvm` lifecycle verbs its own Boot /
/// Shutdown buttons use, honestly greyed when no row is selected (§7 — a
/// context-gated item disables, never a silent no-op). The File/Edit/Help spine is
/// omitted (the surface has no file / clipboard / about seam). The status cluster
/// counts the roster (total · running · a gated-VMM warning) from live state.
mod menubar {
    use super::{apply_action, Action, CreateForm, InstancesState, Op, VmRunState, DOT};
    use mde_egui::egui::Ui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};

    /// One menu action — each routes to a real Instances seam in [`apply`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// Toggle the inline "New VM" create form (the `show_create` seam).
        ToggleCreate,
        /// Boot the selected VM (`Op::Boot` via the broker).
        Boot,
        /// Shut down the selected VM (`Op::Shutdown` via the broker).
        Shutdown,
    }

    /// Render the INSTANCES bar and return the action picked this frame, if any.
    pub(super) fn show(ui: &mut Ui, state: &InstancesState) -> Option<MenuAction> {
        let has_selection = state.selected.is_some();
        let menus = build_menus(state.show_create, has_selection);
        let status = build_status(state);
        let model = MenuBarModel {
            // The dock groups Instances under **Workloads** (purple), so the title
            // wears that categorical accent (lock 2).
            title: "Instances",
            accent: Style::ACCENT_WORKLOADS,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// Build the two menus from live gating: the New-VM toggle's check state, and
    /// whether a roster row is selected (Power's gate).
    fn build_menus(show_create: bool, has_selection: bool) -> Vec<Menu<MenuAction>> {
        vec![
            Menu::new(
                "Instance",
                vec![Entry::Item(
                    Item::new(MenuAction::ToggleCreate, "New VM\u{2026}").checked(show_create),
                )],
            ),
            Menu::new(
                "Power",
                vec![
                    Entry::Item(
                        Item::new(MenuAction::Boot, "Boot selected").enabled(has_selection),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::Shutdown, "Shut down selected")
                            .enabled(has_selection),
                    ),
                ],
            ),
        ]
    }

    /// The live status cluster: the roster size, the running count, and an honest
    /// gated-VMM warning when a last op couldn't reach cloud-hypervisor.
    fn build_status(state: &InstancesState) -> Vec<StatusChip> {
        let total = state.vms.len();
        let running = state
            .vms
            .iter()
            .filter(|v| v.state == VmRunState::Running)
            .count();
        let gated = state.vms.iter().any(|v| v.state == VmRunState::Gated);
        let mut chips = vec![StatusChip::new(
            format!("{total} VM{}", if total == 1 { "" } else { "s" }),
            ChipTone::Neutral,
        )];
        if running > 0 {
            chips.push(StatusChip::with_icon(
                DOT,
                format!("{running} running"),
                ChipTone::Ok,
            ));
        }
        if gated {
            chips.push(StatusChip::with_icon(DOT, "no VMM", ChipTone::Warn));
        }
        chips
    }

    /// Apply a picked action to its real seam (§6). Boot / Shutdown route through the
    /// SAME [`apply_action`] the roster buttons use, so gating + the honest inline
    /// error are preserved; the toggle mirrors the header's New-VM button.
    pub(super) fn apply(state: &mut InstancesState, action: MenuAction) {
        match action {
            MenuAction::ToggleCreate => {
                state.show_create = !state.show_create;
                if state.show_create {
                    state.form = CreateForm::default();
                }
            }
            MenuAction::Boot | MenuAction::Shutdown => {
                let Some(idx) = state.selected else {
                    return;
                };
                let op = if matches!(action, MenuAction::Boot) {
                    Op::Boot
                } else {
                    Op::Shutdown
                };
                apply_action(
                    &mut state.vms,
                    &mut state.selected,
                    &mut state.last_error,
                    &*state.broker,
                    Action::Lifecycle { idx, op },
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{InstancesState, LocalVm, VmRunState};
        use super::{apply, build_menus, build_status, MenuAction};
        use mde_egui::menubar::Entry;
        use mde_egui::ChipTone;
        use mde_kvm::VmSpec;

        fn vm(name: &str, state: VmRunState) -> LocalVm {
            LocalVm {
                spec: VmSpec::new(name, 2, 2048, format!("/x/{name}.img")),
                state,
            }
        }

        #[test]
        fn power_items_disable_without_a_selection() {
            // No row selected → Boot/Shutdown grey (a disabled item, never omitted).
            let menus = build_menus(false, false);
            let power = menus
                .iter()
                .find(|m| m.title == "Power")
                .expect("Power menu");
            for entry in &power.entries {
                if let Entry::Item(item) = entry {
                    assert!(!item.enabled, "{} greys with no selection", item.label);
                }
            }
            // With a selection they enable.
            let menus = build_menus(false, true);
            let power = menus
                .iter()
                .find(|m| m.title == "Power")
                .expect("Power menu");
            for entry in &power.entries {
                if let Entry::Item(item) = entry {
                    assert!(item.enabled, "{} enables with a selection", item.label);
                }
            }
        }

        #[test]
        fn new_vm_item_tracks_the_form_toggle() {
            // The Instance menu's New-VM item is a checkable toggle whose mark tracks
            // the `show_create` state.
            let new_vm_checked = |show_create: bool| -> Option<bool> {
                let menus = build_menus(show_create, false);
                assert_eq!(menus[0].title, "Instance");
                menus[0]
                    .entries
                    .iter()
                    .find_map(|e| match e {
                        Entry::Item(i) => Some(i.checked),
                        _ => None,
                    })
                    .flatten()
            };
            assert_eq!(
                new_vm_checked(false),
                Some(false),
                "form closed ⇒ unchecked"
            );
            assert_eq!(new_vm_checked(true), Some(true), "form open ⇒ checked");
        }

        #[test]
        fn toggle_create_flips_the_form() {
            let mut state = InstancesState::default();
            assert!(!state.show_create);
            apply(&mut state, MenuAction::ToggleCreate);
            assert!(state.show_create, "the toggle opens the form");
            apply(&mut state, MenuAction::ToggleCreate);
            assert!(!state.show_create, "the toggle closes it again");
        }

        #[test]
        fn boot_drives_the_selected_vm_through_the_broker() {
            // The REAL ChBroker against a VM with no live VMM: the menu Boot folds the
            // honest gated state exactly like the row button (§7 — no fake success).
            let mut state = InstancesState {
                vms: vec![vm("ghost", VmRunState::Defined)],
                selected: Some(0),
                ..InstancesState::default()
            };
            apply(&mut state, MenuAction::Boot);
            assert_eq!(state.vms[0].state, VmRunState::Gated);
            assert!(
                state.last_error.is_some(),
                "a gated boot records the honest error"
            );
        }

        #[test]
        fn boot_without_a_selection_is_an_honest_no_op() {
            let mut state = InstancesState {
                vms: vec![vm("web1", VmRunState::Defined)],
                selected: None,
                ..InstancesState::default()
            };
            apply(&mut state, MenuAction::Boot);
            // Nothing was driven — the VM keeps its state, no error minted.
            assert_eq!(state.vms[0].state, VmRunState::Defined);
            assert!(state.last_error.is_none());
        }

        #[test]
        fn status_counts_the_roster_and_flags_a_gated_vmm() {
            let state = InstancesState {
                vms: vec![
                    vm("a", VmRunState::Running),
                    vm("b", VmRunState::Gated),
                    vm("c", VmRunState::Stopped),
                ],
                ..InstancesState::default()
            };
            let chips = build_status(&state);
            assert!(chips.iter().any(|c| c.text == "3 VMs"));
            assert!(chips
                .iter()
                .any(|c| c.text == "1 running" && c.tone == ChipTone::Ok));
            assert!(chips
                .iter()
                .any(|c| c.text == "no VMM" && c.tone == ChipTone::Warn));
        }
    }
}

#[cfg(test)]
impl InstancesState {
    /// Build state over an injected broker (the test seam).
    fn with_broker(broker: Box<dyn VmBroker>) -> Self {
        Self {
            broker,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A shared log of the (VM name, op) pairs the panel drove into the broker.
    type Calls = Rc<RefCell<Vec<(String, Op)>>>;

    /// A recording fake broker: captures every (name, op) and replays a canned
    /// outcome — `Ok`, or the exact `KvmError::Connect` the real transport raises
    /// when no cloud-hypervisor is listening (the gated path).
    struct FakeBroker {
        calls: Calls,
        gated: bool,
    }

    impl FakeBroker {
        fn ok() -> (Self, Calls) {
            let calls = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    calls: calls.clone(),
                    gated: false,
                },
                calls,
            )
        }
    }

    impl VmBroker for FakeBroker {
        fn run(&self, spec: &VmSpec, op: Op) -> Result<(), KvmError> {
            self.calls.borrow_mut().push((spec.name.clone(), op));
            if self.gated {
                Err(KvmError::Connect(
                    api_socket_path(&spec.name),
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no api-socket"),
                ))
            } else {
                Ok(())
            }
        }
    }

    /// A dual-homed VM built through mde-kvm's public constructors (lock 19).
    fn dual_homed(name: &str) -> VmSpec {
        VmSpec::new(name, 2, 2048, format!("/home/op/Local/{name}.img"))
            .with_virtio_gpu(true)
            .with_nic(Nic::mesh(format!("mvm-{name}-mesh")))
            .with_nic(Nic::lan(format!("mvm-{name}-lan")))
    }

    /// Drive one headless frame of `instances_panel` and tessellate it on the CPU —
    /// the same `Context::run` → `tessellate` path the DRM runner drives minus the
    /// GPU. Returns whether it produced draw primitives.
    fn run_panel(state: &mut InstancesState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| instances_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn empty_roster_paints_the_no_vms_empty_state() {
        let mut state = InstancesState::default();
        assert!(state.vms.is_empty());
        let drew = run_panel(&mut state);
        assert!(drew, "the no-VMs EmptyState produced no draw primitives");
    }

    #[test]
    fn roster_lists_local_vms_with_their_dual_homed_nics() {
        let (broker, _calls) = FakeBroker::ok();
        let mut state = InstancesState::with_broker(Box::new(broker));
        state.vms = vec![
            LocalVm {
                spec: dual_homed("web1"),
                state: VmRunState::Running,
            },
            LocalVm {
                spec: dual_homed("db1"),
                state: VmRunState::Stopped,
            },
        ];
        let drew = run_panel(&mut state);
        assert!(drew, "the VM roster produced no draw primitives");
        // Each VM carries a mesh peer + a LAN NIC (lock 19) — the panel's NIC lines.
        assert_eq!(state.vms[0].spec.nics.len(), 2);
        assert_eq!(state.vms[0].spec.nics[0].role, NicRole::Mesh);
        assert_eq!(state.vms[0].spec.nics[1].role, NicRole::Lan);
    }

    #[test]
    fn a_boot_op_drives_the_broker_with_the_right_verb_and_folds_running() {
        let (broker, calls) = FakeBroker::ok();
        let mut state = InstancesState::with_broker(Box::new(broker));
        state.vms = vec![LocalVm {
            spec: dual_homed("web1"),
            state: VmRunState::Defined,
        }];
        // The click path: apply a Boot action (headless — no real button press).
        apply_action(
            &mut state.vms,
            &mut state.selected,
            &mut state.last_error,
            &*state.broker,
            Action::Lifecycle {
                idx: 0,
                op: Op::Boot,
            },
        );
        assert_eq!(
            calls.borrow().as_slice(),
            &[("web1".to_string(), Op::Boot)],
            "Boot must reach the broker as exactly one boot verb"
        );
        assert_eq!(state.vms[0].state, VmRunState::Running);
        assert!(state.last_error.is_none());
    }

    #[test]
    fn create_appends_the_vm_and_selects_it() {
        let (broker, calls) = FakeBroker::ok();
        let mut state = InstancesState::with_broker(Box::new(broker));
        apply_action(
            &mut state.vms,
            &mut state.selected,
            &mut state.last_error,
            &*state.broker,
            Action::Create(dual_homed("web1")),
        );
        assert_eq!(state.vms.len(), 1, "Create appends the VM to the roster");
        assert_eq!(state.vms[0].spec.name, "web1");
        assert_eq!(state.selected, Some(0));
        assert_eq!(
            calls.borrow().as_slice(),
            &[("web1".to_string(), Op::Create)]
        );
    }

    #[test]
    fn an_absent_vmm_surfaces_the_typed_gated_error_not_a_crash() {
        // The REAL ChBroker against a VM whose api-socket has no listener (no
        // cloud-hypervisor on the build host) — the real UnixSocketTransport raises
        // KvmError::Connect, which must surface as an honest inline message + a
        // `gated` state, never a panic. This also proves mde-kvm is a live caller.
        let mut state = InstancesState::default();
        state.vms = vec![LocalVm {
            spec: dual_homed("ghost-vm-e12-7"),
            state: VmRunState::Defined,
        }];
        apply_action(
            &mut state.vms,
            &mut state.selected,
            &mut state.last_error,
            &*state.broker,
            Action::Lifecycle {
                idx: 0,
                op: Op::Boot,
            },
        );
        assert_eq!(state.vms[0].state, VmRunState::Gated);
        let err = state
            .last_error
            .clone()
            .expect("a gated op records an honest error, not a fake success");
        assert!(err.contains("cloud-hypervisor isn't running"), "{err}");
        assert!(err.contains("gated"), "{err}");
        // The error banner + the gated pip still tessellate cleanly.
        assert!(
            run_panel(&mut state),
            "the gated roster produced no draw primitives"
        );
    }

    #[test]
    fn create_form_validates_and_derives_dual_homed_nics() {
        let mut f = CreateForm {
            name: "web1".to_string(),
            vcpus: "2".to_string(),
            mem_mib: "2048".to_string(),
            disk: "/home/op/Local/web1.img".to_string(),
            error: None,
        };
        let spec = f.to_spec().expect("a fully-specified form parses");
        assert_eq!(spec.name, "web1");
        assert_eq!(spec.vcpus, 2);
        assert_eq!(spec.mem_mib, 2048);
        // Derived dual-homed NICs (lock 19): a mesh peer + a LAN bridge.
        assert_eq!(spec.nics.len(), 2);
        assert_eq!(spec.nics[0].role, NicRole::Mesh);
        assert_eq!(spec.nics[0].tap, "mvm-web1-mesh");
        assert_eq!(spec.nics[1].role, NicRole::Lan);
        assert_eq!(spec.nics[1].tap, "mvm-web1-lan");

        // Blank name / zero vCPUs / missing disk are rejected inline (no panic).
        f.name = "  ".to_string();
        assert!(f.to_spec().is_err());
        f.name = "web1".to_string();
        f.vcpus = "0".to_string();
        assert!(f.to_spec().is_err());
        f.vcpus = "2".to_string();
        f.disk = String::new();
        assert!(f.to_spec().is_err());
    }
}
