//! U16 — the **App VM** delivery view: VMs whose individual apps are forwarded
//! into the MDE desktop (VDI **app-mode** via `session_broker`). Rather than a
//! full seat, the operator runs single apps from the VM windowed into their own
//! desktop. The roster carries the same live status · drift · metrics, a lead
//! `app-mode` tag, explicit per-app launch, whole-VM console + lifecycle verbs,
//! and honest readiness text.

use mackes_mesh_types::android_apps::pending_starter_entries;
use mackes_mesh_types::app_catalog::{is_valid_flatpak_app_id, FlatpakInstallState};
use mackes_mesh_types::cloud::{DeliveryType, DriftFlag, WorkloadRow, APP_VM_ALLOWED_CAPABILITIES};
use mackes_mesh_types::vdi_session::{
    AppVmLaunchRequest, AppVmLifecycleState, SessionRequest, BROWSER_VM_WORKLOAD_ID,
};
use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{carbon_icon, card, field, inset, muted_note, status_dot, Style};

use crate::bus_reader::BusReader;

use super::super::{row_button, DeliveryView, WorkloadsRoute, WorkloadsState};

const VDI_SESSION_ACTION_TOPIC: &str = "action/vdi/session";

/// The App VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the App VM view — the app-mode roster + per-VM console/lifecycle.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    heading(
        ui,
        "App VM",
        "VMs whose individual apps are forwarded into your desktop (VDI app-mode).",
    );
    application_family_index(ui, state);
    provision_cta(ui, state, "Provision an app VM");

    let rows: Vec<WorkloadRow> = state.workloads_of(DeliveryView::AppVm).cloned().collect();
    if rows.is_empty() {
        crate::empty_state::show(
            ui,
            "No app VMs yet",
            "An app VM appears here once a placement node reports an app_vm workload in its \
             state/cloud mirror.",
        );
    } else {
        for row in &rows {
            let model = render_model(row, read_session_projection(state, row));
            app_card(ui, state, row, &model);
        }
    }
    muted_note(
        ui,
        "Launch app opens the admitted catalog identity through session_broker (VDI app-mode). \
         Open uses the typed Workload attachment lane; guest readiness and reconnect state stay \
         separate from the transport link.",
    );
}

// ───────────────────────── application-family information architecture ─────

/// The three application-delivery families have different authorities and
/// readiness contracts. Keep their labels stable in the Workloads surface so a
/// Chromium guest, an Android package, and a Flatpak App VM cannot read as one
/// interchangeable launch target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplicationFamily {
    ChromiumVm,
    AndroidApplications,
    FlatpakAppVm,
}

impl ApplicationFamily {
    const fn label(self) -> &'static str {
        match self {
            Self::ChromiumVm => "Chromium VM",
            Self::AndroidApplications => "Android applications",
            Self::FlatpakAppVm => "Flatpak / App VM",
        }
    }

    /// Existing typed identities, not UI-created aliases.
    const fn stable_id(self) -> &'static str {
        match self {
            Self::ChromiumVm => BROWSER_VM_WORKLOAD_ID,
            Self::AndroidApplications => DeliveryType::AndroidVm.as_str(),
            Self::FlatpakAppVm => DeliveryType::AppVm.as_str(),
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::ChromiumVm => "Dedicated guest-owned Chromium browser workload.",
            Self::AndroidApplications => {
                "Governed AOSP starter applications inside a Cuttlefish Android VM."
            }
            Self::FlatpakAppVm => {
                "Guest-owned Flatpak applications forwarded through the App VM session broker."
            }
        }
    }
}

/// Count only the typed rows belonging to the three application families. A
/// normal Desktop VM, service, or container is deliberately not pulled into
/// this index; it remains in its existing delivery view.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ApplicationFamilyCounts {
    chromium_vms: usize,
    android_vms: usize,
    flatpak_app_vms: usize,
    admitted_flatpak_apps: usize,
}

impl ApplicationFamilyCounts {
    fn add(&mut self, family: ApplicationFamily, admitted_flatpak_app: bool) {
        match family {
            ApplicationFamily::ChromiumVm => self.chromium_vms += 1,
            ApplicationFamily::AndroidApplications => self.android_vms += 1,
            ApplicationFamily::FlatpakAppVm => {
                self.flatpak_app_vms += 1;
                if admitted_flatpak_app {
                    self.admitted_flatpak_apps += 1;
                }
            }
        }
    }

    const fn workload_count(self, family: ApplicationFamily) -> usize {
        match family {
            ApplicationFamily::ChromiumVm => self.chromium_vms,
            ApplicationFamily::AndroidApplications => self.android_vms,
            ApplicationFamily::FlatpakAppVm => self.flatpak_app_vms,
        }
    }
}

/// Classify only the stable identities already present in the cloud contract.
/// `None` is important: it lets callers preserve existing behavior for rows
/// this family index does not own.
fn application_family(row: &WorkloadRow) -> Option<ApplicationFamily> {
    match row.delivery_type {
        DeliveryType::DesktopVm if row.name == BROWSER_VM_WORKLOAD_ID => {
            Some(ApplicationFamily::ChromiumVm)
        }
        DeliveryType::AndroidVm => Some(ApplicationFamily::AndroidApplications),
        DeliveryType::AppVm => Some(ApplicationFamily::FlatpakAppVm),
        _ => None,
    }
}

fn application_family_counts(state: &WorkloadsState) -> ApplicationFamilyCounts {
    let mut counts = ApplicationFamilyCounts::default();
    for view in DeliveryView::ALL {
        for row in state.workloads_of(view) {
            let Some(family) = application_family(row) else {
                continue;
            };
            counts.add(
                family,
                family == ApplicationFamily::FlatpakAppVm
                    && row.app.as_ref().is_some_and(valid_app_vm_request),
            );
        }
    }
    counts
}

/// A display-only projection for the family index. It contains no action and
/// no synthetic readiness: every line is either an existing cloud/session gate
/// or an explicit pending state from the typed Android inventory contract.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ApplicationFamilyProjection {
    family: ApplicationFamily,
    workload_count: usize,
    detail: String,
    lifecycle: String,
    readiness: String,
}

fn application_family_projections(
    browser_target: Option<(&str, &str, &str, bool)>,
    counts: ApplicationFamilyCounts,
) -> [ApplicationFamilyProjection; 3] {
    let browser = browser_projection(
        browser_target,
        counts.workload_count(ApplicationFamily::ChromiumVm),
    );
    let android = android_projection(counts.android_vms);
    let flatpak = flatpak_projection(counts);
    [browser, android, flatpak]
}

fn browser_projection(
    target: Option<(&str, &str, &str, bool)>,
    workload_count: usize,
) -> ApplicationFamilyProjection {
    let (detail, lifecycle, readiness) = match target {
        Some((node, workload, status, broker_available)) => (
            format!("{workload} on {node}"),
            format!("guest lifecycle: {}", status.trim()),
            if broker_available {
                "session gate: available".to_owned()
            } else {
                "session gate: unavailable".to_owned()
            },
        ),
        None => (
            BROWSER_VM_WORKLOAD_ID.to_owned(),
            "not admitted: no browser-vm workload reported".to_owned(),
            "session gate: unavailable".to_owned(),
        ),
    };
    ApplicationFamilyProjection {
        family: ApplicationFamily::ChromiumVm,
        workload_count,
        detail,
        lifecycle,
        readiness,
    }
}

fn android_projection(vm_count: usize) -> ApplicationFamilyProjection {
    let starter_entries = pending_starter_entries();
    let pending = starter_entries
        .first()
        .copied()
        .map(|entry| {
            (
                entry.availability.label(),
                entry.readiness.label(),
                entry.launch_readiness.label(),
            )
        })
        .unwrap_or((
            "inventory pending",
            "guest pending",
            "launch integration pending",
        ));
    let lifecycle = if vm_count == 0 {
        "not admitted: no Android VM workload reported".to_owned()
    } else {
        format!("{vm_count} Android VM workload(s) reported")
    };
    ApplicationFamilyProjection {
        family: ApplicationFamily::AndroidApplications,
        workload_count: vm_count,
        detail: format!(
            "{} governed AOSP starter applications per Android VM",
            starter_entries.len()
        ),
        lifecycle,
        readiness: format!("{} · {} · {}", pending.0, pending.1, pending.2),
    }
}

fn flatpak_projection(counts: ApplicationFamilyCounts) -> ApplicationFamilyProjection {
    let lifecycle = if counts.flatpak_app_vms == 0 {
        "not admitted: no App VM workload reported".to_owned()
    } else {
        format!(
            "{} App VM workload(s); per-app broker lifecycle shown below",
            counts.flatpak_app_vms
        )
    };
    let readiness = if counts.flatpak_app_vms == 0 {
        "launch gate: unavailable".to_owned()
    } else if counts.admitted_flatpak_apps == 0 {
        "launch gate: unavailable (no admitted catalog identity)".to_owned()
    } else {
        "per-app launch gate: installed + connected or paused".to_owned()
    };
    ApplicationFamilyProjection {
        family: ApplicationFamily::FlatpakAppVm,
        workload_count: counts.flatpak_app_vms,
        detail: format!(
            "{} of {} rows carry an admitted Flatpak identity",
            counts.admitted_flatpak_apps, counts.flatpak_app_vms
        ),
        lifecycle,
        readiness,
    }
}

fn application_family_index(ui: &mut egui::Ui, state: &WorkloadsState) {
    let projections =
        application_family_projections(state.browser_vm_target(), application_family_counts(state));
    ui.label(
        RichText::new("Application families")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    muted_note(
        ui,
        "Chromium, Android, and Flatpak applications keep separate guest, session, and launch authorities.",
    );
    ui.add_space(Style::SP_XS);
    for projection in &projections {
        application_family_card(ui, projection);
    }
    ui.add_space(Style::SP_S);
}

fn application_family_card(ui: &mut egui::Ui, projection: &ApplicationFamilyProjection) {
    card().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            tag(ui, projection.family.label());
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("id {}", projection.family.stable_id())).size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT,
                RichText::new(projection.family.description()).size(Style::SMALL),
            );
        });
        ui.add_space(Style::SP_XS);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(&projection.detail).size(Style::SMALL),
        );
        ui.horizontal_wrapped(|ui| {
            let workload_count = projection.workload_count.to_string();
            field(ui, "workloads", &workload_count, Style::TEXT_DIM);
            ui.add_space(Style::SP_M);
            field(ui, "lifecycle", &projection.lifecycle, Style::TEXT);
            ui.add_space(Style::SP_M);
            field(ui, "readiness", &projection.readiness, Style::TEXT);
        });
    });
    ui.add_space(Style::SP_XS);
}

/// One app-VM card — name · `app-mode` tag · status · drift, the metrics, then the
/// whole-VM typed attachment and lifecycle verbs.
fn app_card(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    row: &WorkloadRow,
    model: &AppVmRenderModel,
) {
    card().show(ui, |ui| {
        header_row(ui, row);
        catalog_line(ui, model);
        lifecycle_line(ui, model);
        metrics_line(ui, row);
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            if launch_button(ui, model.launch.is_ready()).clicked() {
                state.issue_app_launch(row);
            }
            if row_button(ui, "Console", false).clicked() {
                state.issue_console_attach(&row.node, &row.name, &row.name);
            }
            if row_button(ui, "Start", false).clicked() {
                state.issue_workload_operation(
                    WorkloadOperationAction::StartAndAttach,
                    Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Stop", false).clicked() {
                state.issue_workload_operation(
                    WorkloadOperationAction::Stop,
                    None,
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Reboot\u{2026}", true).clicked() {
                state.issue_workload_operation(
                    WorkloadOperationAction::Restart,
                    Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Destroy\u{2026}", true).clicked() {
                state.issue_workload_operation(
                    WorkloadOperationAction::Destroy,
                    None,
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
        });
        if let Some(reason) = model.launch.block_reason() {
            muted_note(ui, reason);
        }
    });
    ui.add_space(Style::SP_S);
}

/// A typed catalog identity row. The Workloads mirror intentionally carries the
/// already-admitted App VM request rather than raw Flatpak command data; its
/// install state is derived only from the matching broker lifecycle evidence.
fn catalog_line(ui: &mut egui::Ui, model: &AppVmRenderModel) {
    ui.horizontal(|ui| {
        ui.colored_label(Style::TEXT_DIM, RichText::new("catalog").size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        let state = catalog_state_label(model.catalog.state);
        ui.colored_label(
            catalog_tone(model.catalog.state),
            RichText::new(state).size(Style::SMALL).strong(),
        );
        if let Some(request) = model.catalog.request.as_ref() {
            ui.add_space(Style::SP_M);
            ui.colored_label(
                Style::TEXT,
                RichText::new(&request.app_id).size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("revision {}", request.catalog_revision)).size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!(
                    "profile {} · capabilities {}",
                    request.guest_profile,
                    capability_label(&request.requested_capabilities)
                ))
                .size(Style::SMALL),
            );
        }
    });
}

/// The guest/application lifecycle is independent from the libvirt row's
/// transport status. Missing broker evidence remains visibly unavailable.
fn lifecycle_line(ui: &mut egui::Ui, model: &AppVmRenderModel) {
    ui.horizontal(|ui| {
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new("app lifecycle").size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        let lifecycle = model
            .session
            .as_ref()
            .map(|session| lifecycle_label(session.lifecycle))
            .unwrap_or("no admitted session");
        ui.colored_label(
            lifecycle_tone(model.session.as_ref().map(|session| session.lifecycle)),
            RichText::new(lifecycle).size(Style::SMALL),
        );
        if let Some(reason) = model
            .session
            .as_ref()
            .and_then(|session| session.reason.as_deref())
        {
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("— {reason}")).size(Style::SMALL),
            );
        }
    });
}

/// The launch control is deliberately disabled until the admitted session
/// lifecycle says the guest application is connected or resumable. This keeps
/// Workloads on the existing session-broker path and cannot fall back to a host
/// `.desktop`, command, or native Flatpak launch.
fn launch_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let color = if enabled {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new("Launch app").size(Style::SMALL).color(color)),
    )
}

// ─────────────────────────── typed app projection ──────────────────────────

/// The transport half of the already-admitted VDI session contract. It is kept
/// separate from [`AppVmLifecycleState`]: a disconnected desktop link does not
/// erase guest readiness, while a connected link does not manufacture it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionTransportState {
    Requested,
    Active,
    Disconnected,
    Closed,
}

/// The session-broker read model needed by the App VM card. The broker remains
/// authoritative; this is only a validated, fail-closed UI projection of its
/// public typed action log.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppVmSessionProjection {
    transport: SessionTransportState,
    lifecycle: AppVmLifecycleState,
    catalog_revision: String,
    generation: u64,
    reason: Option<String>,
}

/// The catalog identity plus the install evidence visible to Workloads. `None`
/// for `state` means no admitted guest declaration is present, which renders as
/// not installed rather than as an ordinary host application.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppVmCatalogProjection {
    request: Option<AppVmLaunchRequest>,
    state: Option<FlatpakInstallState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchAvailability {
    Ready,
    NotInstalled,
    StaleCatalog,
    Unavailable,
    NoAdmittedSession,
    SessionClosed,
    LifecycleNotReady(AppVmLifecycleState),
}

impl LaunchAvailability {
    const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    const fn block_reason(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::NotInstalled => Some("Launch unavailable: the guest app is not installed."),
            Self::StaleCatalog => {
                Some("Launch unavailable: the admitted catalog revision is stale.")
            }
            Self::Unavailable => Some("Launch unavailable: the guest application is unavailable."),
            Self::NoAdmittedSession => {
                Some("Launch unavailable: no admitted App VM session is ready.")
            }
            Self::SessionClosed => {
                Some("Launch unavailable: the admitted App VM session is closed.")
            }
            Self::LifecycleNotReady(state) => match state {
                AppVmLifecycleState::Installing => {
                    Some("Launch unavailable: the guest application is installing.")
                }
                AppVmLifecycleState::WaitingForPlacement => {
                    Some("Launch unavailable: the App VM is waiting for placement.")
                }
                AppVmLifecycleState::StartingGuest => {
                    Some("Launch unavailable: the App VM guest is starting.")
                }
                AppVmLifecycleState::StartingApp => {
                    Some("Launch unavailable: the guest application is starting.")
                }
                AppVmLifecycleState::Connected | AppVmLifecycleState::Paused => None,
                AppVmLifecycleState::Reconnecting => {
                    Some("Launch unavailable: the guest application is reconnecting.")
                }
                AppVmLifecycleState::Unavailable => {
                    Some("Launch unavailable: the guest application is unavailable.")
                }
                AppVmLifecycleState::Denied => {
                    Some("Launch unavailable: the session was denied by policy.")
                }
                AppVmLifecycleState::StaleCatalog => {
                    Some("Launch unavailable: the admitted catalog revision is stale.")
                }
                AppVmLifecycleState::Failed => {
                    Some("Launch unavailable: the guest application failed.")
                }
            },
        }
    }
}

/// Complete App VM card model. Keeping the mapping outside egui makes every
/// availability decision testable without relying on pixels or button events.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppVmRenderModel {
    catalog: AppVmCatalogProjection,
    session: Option<AppVmSessionProjection>,
    launch: LaunchAvailability,
}

fn render_model(row: &WorkloadRow, session: Option<AppVmSessionProjection>) -> AppVmRenderModel {
    let request = row.app.clone();
    let valid_request = request.as_ref().is_some_and(valid_app_vm_request);
    let state = request.as_ref().map(|request| {
        if !valid_request {
            return FlatpakInstallState::Unavailable;
        }
        catalog_install_state(request, session.as_ref())
    });
    let catalog = AppVmCatalogProjection { request, state };
    let launch = launch_availability(&catalog, session.as_ref());
    AppVmRenderModel {
        catalog,
        session,
        launch,
    }
}

fn valid_app_vm_request(request: &AppVmLaunchRequest) -> bool {
    request.validate().is_ok()
        && is_valid_flatpak_app_id(&request.app_id)
        && request
            .requested_capabilities
            .iter()
            .all(|capability| APP_VM_ALLOWED_CAPABILITIES.contains(&capability.as_str()))
}

fn catalog_install_state(
    request: &AppVmLaunchRequest,
    session: Option<&AppVmSessionProjection>,
) -> FlatpakInstallState {
    let Some(session) = session else {
        // A desired/admitted catalog row without broker evidence is visible, but
        // must not claim that guest content is installed.
        return FlatpakInstallState::Available;
    };
    if session.catalog_revision != request.catalog_revision {
        return FlatpakInstallState::Stale;
    }
    match session.lifecycle {
        AppVmLifecycleState::Installing | AppVmLifecycleState::WaitingForPlacement => {
            FlatpakInstallState::Available
        }
        AppVmLifecycleState::StartingGuest
        | AppVmLifecycleState::StartingApp
        | AppVmLifecycleState::Connected
        | AppVmLifecycleState::Paused
        | AppVmLifecycleState::Reconnecting => FlatpakInstallState::Installed,
        AppVmLifecycleState::Unavailable
        | AppVmLifecycleState::Denied
        | AppVmLifecycleState::Failed => FlatpakInstallState::Unavailable,
        AppVmLifecycleState::StaleCatalog => FlatpakInstallState::Stale,
    }
}

fn launch_availability(
    catalog: &AppVmCatalogProjection,
    session: Option<&AppVmSessionProjection>,
) -> LaunchAvailability {
    let Some(state) = catalog.state else {
        return LaunchAvailability::NotInstalled;
    };
    match state {
        FlatpakInstallState::Stale => LaunchAvailability::StaleCatalog,
        FlatpakInstallState::Unavailable => LaunchAvailability::Unavailable,
        FlatpakInstallState::Available => match session {
            None => LaunchAvailability::NotInstalled,
            Some(session) => LaunchAvailability::LifecycleNotReady(session.lifecycle),
        },
        FlatpakInstallState::Installed => {
            let Some(session) = session else {
                return LaunchAvailability::NoAdmittedSession;
            };
            if session.transport == SessionTransportState::Closed {
                return LaunchAvailability::SessionClosed;
            }
            if matches!(
                session.lifecycle,
                AppVmLifecycleState::Connected | AppVmLifecycleState::Paused
            ) {
                LaunchAvailability::Ready
            } else {
                LaunchAvailability::LifecycleNotReady(session.lifecycle)
            }
        }
    }
}

/// Read the public broker action log and project only the session belonging to
/// this Workloads row. Invalid, unrelated, or incomplete records are ignored;
/// no local fallback state is invented when the Bus is unavailable.
fn read_session_projection(
    state: &WorkloadsState,
    row: &WorkloadRow,
) -> Option<AppVmSessionProjection> {
    let persist = BusReader::new(state.bus_root.clone()).open()?;
    let messages = persist.list_since(VDI_SESSION_ACTION_TOPIC, None).ok()?;
    let requests = messages
        .into_iter()
        .filter_map(|message| message.body)
        .filter_map(|body| serde_json::from_str::<SessionRequest>(&body).ok());
    project_session(requests, row, &crate::discovery::local_peer())
}

fn project_session<I>(
    requests: I,
    row: &WorkloadRow,
    client_peer: &str,
) -> Option<AppVmSessionProjection>
where
    I: IntoIterator<Item = SessionRequest>,
{
    let target = row.app.as_ref()?;
    if !valid_app_vm_request(target) {
        return None;
    }
    let mut projection = None;
    for request in requests {
        match request {
            SessionRequest::OpenApp {
                id,
                serving_peer,
                vm_id,
                client_peer: event_client_peer,
                app_id,
                catalog_revision,
                guest_profile,
                requested_capabilities,
                resume,
            } => {
                let event = AppVmLaunchRequest {
                    app_id,
                    catalog_revision,
                    guest_profile,
                    requested_capabilities,
                    session_id: id.clone(),
                    resume,
                };
                if id != target.session_id
                    || serving_peer != row.node
                    || vm_id != row.name
                    || event_client_peer != client_peer
                    || !valid_app_vm_request(&event)
                    || event.app_id != target.app_id
                    || event.guest_profile != target.guest_profile
                    || event.requested_capabilities != target.requested_capabilities
                    || event.resume != target.resume
                {
                    continue;
                }
                if projection
                    .as_ref()
                    .is_some_and(|current: &AppVmSessionProjection| {
                        current.transport == SessionTransportState::Closed
                    })
                {
                    continue;
                }
                if let Some(current) = projection.as_mut() {
                    // Repeated OpenApp is the broker's idempotent retry: refresh
                    // catalog intent while preserving the observed lifecycle.
                    current.catalog_revision = event.catalog_revision;
                } else {
                    projection = Some(AppVmSessionProjection {
                        transport: SessionTransportState::Requested,
                        lifecycle: AppVmLifecycleState::WaitingForPlacement,
                        catalog_revision: event.catalog_revision,
                        generation: 0,
                        reason: None,
                    });
                }
            }
            SessionRequest::AppState {
                id,
                generation,
                state,
                reason,
            } if projection.as_ref().is_some_and(|_| id == target.session_id) => {
                let Some(current) = projection.as_mut() else {
                    continue;
                };
                if current.transport == SessionTransportState::Closed {
                    continue;
                }
                let generation_is_stale = if generation == 0 {
                    current.generation != 0
                } else {
                    generation <= current.generation
                };
                if generation_is_stale || !current.lifecycle.can_transition_to(state) {
                    continue;
                }
                current.lifecycle = state;
                if generation != 0 {
                    current.generation = generation;
                }
                current.reason = bound_reason(reason);
            }
            SessionRequest::Active { id }
                if projection.as_ref().is_some_and(|_| id == target.session_id) =>
            {
                if let Some(current) = projection.as_mut() {
                    if matches!(
                        current.transport,
                        SessionTransportState::Requested
                            | SessionTransportState::Active
                            | SessionTransportState::Disconnected
                    ) {
                        current.transport = SessionTransportState::Active;
                    }
                }
            }
            SessionRequest::Disconnect { id }
                if projection.as_ref().is_some_and(|_| id == target.session_id) =>
            {
                if let Some(current) = projection.as_mut() {
                    if matches!(
                        current.transport,
                        SessionTransportState::Active | SessionTransportState::Disconnected
                    ) {
                        current.transport = SessionTransportState::Disconnected;
                    }
                }
            }
            SessionRequest::Close { id }
                if projection.as_ref().is_some_and(|_| id == target.session_id) =>
            {
                if let Some(current) = projection.as_mut() {
                    current.transport = SessionTransportState::Closed;
                }
            }
            _ => {}
        }
    }
    projection
}

fn bound_reason(reason: Option<String>) -> Option<String> {
    const MAX_CHARS: usize = 255;
    let reason = reason?;
    if reason.chars().any(char::is_control) {
        return None;
    }
    let mut bounded: String = reason.chars().take(MAX_CHARS).collect();
    if reason.chars().count() > MAX_CHARS {
        bounded.push_str("...");
    }
    Some(bounded)
}

fn catalog_state_label(state: Option<FlatpakInstallState>) -> &'static str {
    match state {
        None | Some(FlatpakInstallState::Available) => "not installed",
        Some(FlatpakInstallState::Installed) => "installed",
        Some(FlatpakInstallState::Stale) => "stale",
        Some(FlatpakInstallState::Unavailable) => "unavailable",
    }
}

fn capability_label(capabilities: &[String]) -> String {
    if capabilities.is_empty() {
        return "none".to_owned();
    }
    capabilities.join(", ")
}

fn catalog_tone(state: Option<FlatpakInstallState>) -> Color32 {
    match state {
        Some(FlatpakInstallState::Installed) => Style::SUPPORT_SUCCESS,
        Some(FlatpakInstallState::Available | FlatpakInstallState::Stale) => Style::WARN,
        Some(FlatpakInstallState::Unavailable) | None => Style::DANGER,
    }
}

const fn lifecycle_label(state: AppVmLifecycleState) -> &'static str {
    match state {
        AppVmLifecycleState::Installing => "installing",
        AppVmLifecycleState::WaitingForPlacement => "waiting for placement",
        AppVmLifecycleState::StartingGuest => "starting guest",
        AppVmLifecycleState::StartingApp => "starting app",
        AppVmLifecycleState::Connected => "connected",
        AppVmLifecycleState::Paused => "paused",
        AppVmLifecycleState::Reconnecting => "reconnecting",
        AppVmLifecycleState::Unavailable => "unavailable",
        AppVmLifecycleState::Denied => "denied",
        AppVmLifecycleState::StaleCatalog => "stale catalog",
        AppVmLifecycleState::Failed => "failed",
    }
}

fn lifecycle_tone(state: Option<AppVmLifecycleState>) -> Color32 {
    match state {
        Some(AppVmLifecycleState::Connected) => Style::SUPPORT_SUCCESS,
        Some(AppVmLifecycleState::Paused)
        | Some(
            AppVmLifecycleState::Installing
            | AppVmLifecycleState::WaitingForPlacement
            | AppVmLifecycleState::StartingGuest
            | AppVmLifecycleState::StartingApp
            | AppVmLifecycleState::Reconnecting,
        ) => Style::WARN,
        Some(
            AppVmLifecycleState::Unavailable
            | AppVmLifecycleState::Denied
            | AppVmLifecycleState::StaleCatalog
            | AppVmLifecycleState::Failed,
        ) => Style::DANGER,
        None => Style::TEXT_DIM,
    }
}

// ─────────────────────────── shared row grammar ─────────────────────────────

/// The card's identity row: name (strong), the `app-mode` delivery tag, the
/// live-status dot + word, the drift chip, then the node.
fn header_row(ui: &mut egui::Ui, row: &WorkloadRow) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&row.name)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        tag(ui, "app-mode");
        ui.add_space(Style::SP_M);
        let tone = status_tone(&row.status);
        status_dot(ui, tone);
        ui.colored_label(tone, RichText::new(&row.status).size(Style::SMALL));
        ui.add_space(Style::SP_M);
        drift_chip(ui, row.drift);
        ui.add_space(Style::SP_M);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(format!("on {}", row.node)).size(Style::SMALL),
        );
    });
}

/// A small recessed delivery tag — the `inset` well around a dim caption, so the
/// app-mode nature reads at a glance without a coloured chip.
fn tag(ui: &mut egui::Ui, label: &str) {
    inset().show(ui, |ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::ACCENT_WORKLOADS),
        );
    });
}

/// The live cpu / mem / disk metrics row (cpu toned by load).
fn metrics_line(ui: &mut egui::Ui, row: &WorkloadRow) {
    ui.horizontal(|ui| {
        field(
            ui,
            "cpu",
            &format!("{}%", row.cpu_pct),
            load_tone(row.cpu_pct),
        );
        ui.add_space(Style::SP_M);
        field(ui, "mem", &mem_label(row.mem_mb), Style::TEXT);
        ui.add_space(Style::SP_M);
        field(ui, "disk", &format!("{} GiB", row.disk_gb), Style::TEXT);
    });
}

/// A drift chip — a Style SUPPORT_* dot + word for desired-vs-actual state.
fn drift_chip(ui: &mut egui::Ui, drift: DriftFlag) {
    let tone = drift_tone(drift);
    status_dot(ui, tone);
    ui.colored_label(tone, RichText::new(drift_word(drift)).size(Style::SMALL));
}

/// The view heading — the Workloads-accent glyph + title + a one-line blurb.
fn heading(ui: &mut egui::Ui, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, DeliveryView::AppVm.icon(), Style::ICON_M);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(title)
                .size(Style::TITLE)
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
    });
    muted_note(ui, blurb);
    ui.add_space(Style::SP_S);
}

/// The "provision a workload of this type" affordance — jumps to the Provision
/// route (U14 placement + U15 form).
fn provision_cta(ui: &mut egui::Ui, state: &mut WorkloadsState, label: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, "list-add", Style::BODY);
        });
        ui.add_space(Style::SP_XS);
        if ui
            .add(egui::Button::new(
                RichText::new(label)
                    .size(Style::SMALL)
                    .color(Style::ACCENT_WORKLOADS),
            ))
            .clicked()
        {
            state.set_route(WorkloadsRoute::Provision);
        }
    });
    ui.add_space(Style::SP_S);
}

/// The Style tone a live domain status paints.
fn status_tone(status: &str) -> Color32 {
    match status.trim().to_ascii_lowercase().as_str() {
        "running" | "active" => Style::SUPPORT_SUCCESS,
        "paused" | "pmsuspended" => Style::WARN,
        s if s.contains("error") || s.contains("fail") || s.contains("crash") => Style::DANGER,
        _ => Style::TEXT_DIM,
    }
}

/// The Style tone a drift flag paints (drift chips use the SUPPORT_* tokens).
const fn drift_tone(drift: DriftFlag) -> Color32 {
    match drift {
        DriftFlag::InSync => Style::SUPPORT_SUCCESS,
        DriftFlag::Drift => Style::SUPPORT_WARNING,
        DriftFlag::Unknown => Style::TEXT_DIM,
    }
}

/// The drift chip's word.
const fn drift_word(drift: DriftFlag) -> &'static str {
    match drift {
        DriftFlag::InSync => "in sync",
        DriftFlag::Drift => "drift",
        DriftFlag::Unknown => "unplanned",
    }
}

/// The Style tone a cpu-load percentage paints (amber past 70, red past 90).
const fn load_tone(pct: u16) -> Color32 {
    if pct >= 90 {
        Style::DANGER
    } else if pct >= 70 {
        Style::WARN
    } else {
        Style::TEXT
    }
}

/// A memory figure as MiB, or one-decimal GiB past a gibibyte — integer-only so
/// clippy's cast lints stay quiet.
fn mem_label(mb: u32) -> String {
    if mb >= 1024 {
        format!("{}.{} GiB", mb / 1024, (mb % 1024) * 10 / 1024)
    } else {
        format!("{mb} MiB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cloud::{DeliveryType, DriftFlag};

    const NODE: &str = "eagle";
    const VM: &str = "appvm-eagle-org.example.Editor";
    const CLIENT: &str = "seat-1";
    const SESSION: &str = "app-session-eagle-org.example.Editor";

    fn request(revision: &str) -> AppVmLaunchRequest {
        AppVmLaunchRequest::new(
            "org.example.Editor",
            revision,
            "wayland-standard",
            vec!["audio".into(), "clipboard".into()],
            SESSION,
            true,
        )
        .expect("valid App VM request")
    }

    fn row(app: Option<AppVmLaunchRequest>) -> WorkloadRow {
        WorkloadRow {
            name: VM.into(),
            delivery_type: DeliveryType::AppVm,
            node: NODE.into(),
            status: "running".into(),
            cpu_pct: 12,
            mem_mb: 2048,
            disk_gb: 40,
            reachable: true,
            drift: DriftFlag::InSync,
            app,
        }
    }

    fn session(
        revision: &str,
        lifecycle: AppVmLifecycleState,
        transport: SessionTransportState,
    ) -> AppVmSessionProjection {
        AppVmSessionProjection {
            transport,
            lifecycle,
            catalog_revision: revision.into(),
            generation: 0,
            reason: None,
        }
    }

    fn open(revision: &str) -> SessionRequest {
        let request = request(revision);
        SessionRequest::OpenApp {
            id: request.session_id,
            serving_peer: NODE.into(),
            vm_id: VM.into(),
            client_peer: CLIENT.into(),
            app_id: request.app_id,
            catalog_revision: request.catalog_revision,
            guest_profile: request.guest_profile,
            requested_capabilities: request.requested_capabilities,
            resume: request.resume,
        }
    }

    fn app_state(
        state: AppVmLifecycleState,
        generation: u64,
        reason: Option<&str>,
    ) -> SessionRequest {
        SessionRequest::AppState {
            id: SESSION.into(),
            generation,
            state,
            reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn render_model_preserves_not_installed_stale_and_unavailable_states() {
        let missing = render_model(&row(None), None);
        assert_eq!(missing.catalog.state, None);
        assert_eq!(catalog_state_label(missing.catalog.state), "not installed");
        assert_eq!(missing.launch, LaunchAvailability::NotInstalled);

        let desired = row(Some(request("catalog-1")));
        let no_session = render_model(&desired, None);
        assert_eq!(
            no_session.catalog.state,
            Some(FlatpakInstallState::Available)
        );
        assert_eq!(
            catalog_state_label(no_session.catalog.state),
            "not installed"
        );
        assert_eq!(no_session.launch, LaunchAvailability::NotInstalled);

        let stale = render_model(
            &desired,
            Some(session(
                "catalog-1",
                AppVmLifecycleState::StaleCatalog,
                SessionTransportState::Active,
            )),
        );
        assert_eq!(stale.catalog.state, Some(FlatpakInstallState::Stale));
        assert_eq!(stale.launch, LaunchAvailability::StaleCatalog);

        let revision_mismatch = render_model(
            &desired,
            Some(session(
                "catalog-0",
                AppVmLifecycleState::Connected,
                SessionTransportState::Active,
            )),
        );
        assert_eq!(
            revision_mismatch.catalog.state,
            Some(FlatpakInstallState::Stale)
        );
        assert_eq!(revision_mismatch.launch, LaunchAvailability::StaleCatalog);

        let unavailable = render_model(
            &desired,
            Some(session(
                "catalog-1",
                AppVmLifecycleState::Unavailable,
                SessionTransportState::Disconnected,
            )),
        );
        assert_eq!(
            unavailable.catalog.state,
            Some(FlatpakInstallState::Unavailable)
        );
        assert_eq!(unavailable.launch, LaunchAvailability::Unavailable);
    }

    #[test]
    fn launch_is_enabled_only_for_connected_or_paused_admitted_lifecycle() {
        let desired = row(Some(request("catalog-1")));
        for lifecycle in [
            AppVmLifecycleState::WaitingForPlacement,
            AppVmLifecycleState::Installing,
            AppVmLifecycleState::StartingGuest,
            AppVmLifecycleState::StartingApp,
            AppVmLifecycleState::Reconnecting,
            AppVmLifecycleState::Unavailable,
            AppVmLifecycleState::Denied,
            AppVmLifecycleState::StaleCatalog,
            AppVmLifecycleState::Failed,
        ] {
            let model = render_model(
                &desired,
                Some(session(
                    "catalog-1",
                    lifecycle,
                    SessionTransportState::Active,
                )),
            );
            assert!(
                !model.launch.is_ready(),
                "{lifecycle:?} must not enable App VM launch"
            );
        }

        for lifecycle in [AppVmLifecycleState::Connected, AppVmLifecycleState::Paused] {
            let model = render_model(
                &desired,
                Some(session(
                    "catalog-1",
                    lifecycle,
                    SessionTransportState::Disconnected,
                )),
            );
            assert_eq!(model.catalog.state, Some(FlatpakInstallState::Installed));
            assert_eq!(model.launch, LaunchAvailability::Ready);
        }
    }

    #[test]
    fn render_model_keeps_the_admitted_identity_and_policy_metadata() {
        let model = render_model(
            &row(Some(request("catalog-7"))),
            Some(session(
                "catalog-7",
                AppVmLifecycleState::Connected,
                SessionTransportState::Active,
            )),
        );
        let request = model.catalog.request.expect("catalog identity");
        assert_eq!(request.app_id, "org.example.Editor");
        assert_eq!(request.catalog_revision, "catalog-7");
        assert_eq!(request.guest_profile, "wayland-standard");
        assert_eq!(request.requested_capabilities, ["audio", "clipboard"]);
        assert_eq!(
            capability_label(&request.requested_capabilities),
            "audio, clipboard"
        );
        assert_eq!(model.launch, LaunchAvailability::Ready);
    }

    #[test]
    fn session_projection_rejects_false_jumps_and_honors_admitted_readiness() {
        let target = row(Some(request("catalog-1")));
        let false_jump = project_session(
            [
                open("catalog-1"),
                app_state(AppVmLifecycleState::Connected, 1, None),
            ],
            &target,
            CLIENT,
        )
        .expect("open is retained");
        assert_eq!(
            false_jump.lifecycle,
            AppVmLifecycleState::WaitingForPlacement
        );
        assert_eq!(false_jump.generation, 0);

        let admitted = project_session(
            [
                open("catalog-1"),
                app_state(AppVmLifecycleState::Installing, 1, Some("installing")),
                app_state(AppVmLifecycleState::StartingGuest, 2, None),
                app_state(AppVmLifecycleState::StartingApp, 3, Some("portal ready")),
                app_state(AppVmLifecycleState::Connected, 4, Some("surface ready")),
                SessionRequest::Active { id: SESSION.into() },
            ],
            &target,
            CLIENT,
        )
        .expect("admitted session projection");
        assert_eq!(admitted.lifecycle, AppVmLifecycleState::Connected);
        assert_eq!(admitted.transport, SessionTransportState::Active);
        assert_eq!(admitted.generation, 4);
        assert_eq!(admitted.reason.as_deref(), Some("surface ready"));
        assert_eq!(
            render_model(&target, Some(admitted)).launch,
            LaunchAvailability::Ready
        );
    }

    #[test]
    fn session_projection_rejects_open_with_mismatched_capabilities_or_resume() {
        let target = row(Some(request("catalog-1")));
        let mut mismatched_capabilities = open("catalog-1");
        if let SessionRequest::OpenApp {
            requested_capabilities,
            ..
        } = &mut mismatched_capabilities
        {
            requested_capabilities.clear();
        }
        assert!(project_session([mismatched_capabilities], &target, CLIENT).is_none());

        let mut mismatched_resume = open("catalog-1");
        if let SessionRequest::OpenApp { resume, .. } = &mut mismatched_resume {
            *resume = false;
        }
        assert!(project_session([mismatched_resume], &target, CLIENT).is_none());
    }

    #[test]
    fn closed_admitted_session_is_never_presented_as_launchable() {
        let target = row(Some(request("catalog-1")));
        let projected = project_session(
            [
                open("catalog-1"),
                app_state(AppVmLifecycleState::Installing, 1, None),
                app_state(AppVmLifecycleState::StartingGuest, 2, None),
                app_state(AppVmLifecycleState::StartingApp, 3, None),
                app_state(AppVmLifecycleState::Connected, 4, None),
                SessionRequest::Close { id: SESSION.into() },
            ],
            &target,
            CLIENT,
        )
        .expect("closed session remains visible for honest status");
        assert_eq!(projected.transport, SessionTransportState::Closed);
        assert_eq!(
            render_model(&target, Some(projected)).launch,
            LaunchAvailability::SessionClosed
        );
    }

    #[test]
    fn application_family_classifier_uses_typed_identities_and_preserves_unknown_rows() {
        let mut chromium = row(None);
        chromium.name = BROWSER_VM_WORKLOAD_ID.into();
        chromium.delivery_type = DeliveryType::DesktopVm;
        assert_eq!(
            application_family(&chromium),
            Some(ApplicationFamily::ChromiumVm)
        );

        let mut android = row(None);
        android.name = "android-eagle".into();
        android.delivery_type = DeliveryType::AndroidVm;
        assert_eq!(
            application_family(&android),
            Some(ApplicationFamily::AndroidApplications)
        );

        assert_eq!(
            application_family(&row(Some(request("catalog-1")))),
            Some(ApplicationFamily::FlatpakAppVm)
        );

        let mut service = row(None);
        service.delivery_type = DeliveryType::ServiceVm;
        assert_eq!(application_family(&service), None);
    }

    #[test]
    fn application_family_projection_keeps_lifecycle_and_readiness_boundaries_explicit() {
        let counts = ApplicationFamilyCounts {
            chromium_vms: 1,
            android_vms: 1,
            flatpak_app_vms: 2,
            admitted_flatpak_apps: 1,
        };
        let [chromium, android, flatpak] = application_family_projections(
            Some(("eagle", BROWSER_VM_WORKLOAD_ID, "running", true)),
            counts,
        );

        assert_eq!(chromium.family.stable_id(), "browser-vm");
        assert_eq!(chromium.workload_count, 1);
        assert_eq!(chromium.lifecycle, "guest lifecycle: running");
        assert_eq!(chromium.readiness, "session gate: available");

        assert_eq!(android.family.stable_id(), "android_vm");
        assert_eq!(android.workload_count, 1);
        assert!(android
            .detail
            .starts_with("9 governed AOSP starter applications"));
        assert_eq!(
            android.readiness,
            "inventory pending · guest pending · launch integration pending"
        );

        assert_eq!(flatpak.family.stable_id(), "app_vm");
        assert_eq!(flatpak.workload_count, 2);
        assert!(flatpak.lifecycle.contains("per-app broker lifecycle"));
        assert_eq!(
            flatpak.readiness,
            "per-app launch gate: installed + connected or paused"
        );
    }

    #[test]
    fn absent_family_projection_does_not_claim_admission_or_readiness() {
        let [chromium, android, flatpak] =
            application_family_projections(None, ApplicationFamilyCounts::default());

        assert_eq!(chromium.workload_count, 0);
        assert!(chromium.lifecycle.contains("not admitted"));
        assert_eq!(chromium.readiness, "session gate: unavailable");
        assert!(android.lifecycle.contains("not admitted"));
        assert!(android.readiness.contains("inventory pending"));
        assert!(flatpak.lifecycle.contains("not admitted"));
        assert!(flatpak.readiness.contains("unavailable"));
    }
}
