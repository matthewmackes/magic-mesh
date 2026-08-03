//! Universal catalog cards rendered beside discovered desktops.
//!
//! The shell consumes the versioned `state/resources/catalog` contract and is
//! deliberately adapter-agnostic: a new service kind receives the same card,
//! lifecycle actions, and Local Service Stack placement without a UI code path.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use mackes_mesh_types::resources::{
    ResourceActionVerb, ResourceCard, ResourceCatalog, ResourceClass, ServiceCategory,
    ServiceConfigurationFieldKind, ServiceLifecycleStatus, ServiceStackTier,
    RESOURCE_CATALOG_TOPIC,
};
use mde_bus::persist::Persist;
use mde_egui::egui::{self, Color32, FontId, RichText, Sense, Stroke, StrokeKind};
use mde_egui::Style;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CatalogFilter {
    #[default]
    All,
    Desktops,
    Applications,
    MeshServices,
    Media,
    Communications,
    External,
}

impl CatalogFilter {
    const ALL: [Self; 7] = [
        Self::All,
        Self::Desktops,
        Self::Applications,
        Self::MeshServices,
        Self::Media,
        Self::Communications,
        Self::External,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Desktops => "Desktops",
            Self::Applications => "Apps",
            Self::MeshServices => "Mesh services",
            Self::Media => "Media",
            Self::Communications => "Communications",
            Self::External => "External providers",
        }
    }
}

/// Live universal-catalog view state owned by the chooser.
pub(super) struct ResourceBrowserState {
    bus_root: Option<PathBuf>,
    catalog: Option<ResourceCatalog>,
    filter: CatalogFilter,
    stack_expanded: bool,
    selected_resource: Option<String>,
    configuring_resource: Option<String>,
    configuration_draft: BTreeMap<String, String>,
    action_pending: Option<Receiver<Result<String, String>>>,
    action_feedback: Option<String>,
    error: Option<String>,
}

impl ResourceBrowserState {
    pub(super) fn show_desktops(&self) -> bool {
        matches!(self.filter, CatalogFilter::All | CatalogFilter::Desktops)
    }

    pub(super) fn new(bus_root: Option<PathBuf>) -> Self {
        Self {
            bus_root,
            catalog: None,
            filter: CatalogFilter::All,
            stack_expanded: false,
            selected_resource: None,
            configuring_resource: None,
            configuration_draft: BTreeMap::new(),
            action_pending: None,
            action_feedback: None,
            error: None,
        }
    }

    pub(super) fn refresh(&mut self) {
        let Some(root) = self.bus_root.as_ref() else {
            return;
        };
        let result = Persist::open(root.clone())
            .map_err(|error| error.to_string())
            .and_then(|persist| {
                persist
                    .read_latest(RESOURCE_CATALOG_TOPIC)
                    .map_err(|error| error.to_string())
            })
            .and_then(|message| {
                message
                    .and_then(|message| message.body)
                    .ok_or_else(|| "resource catalog has not published yet".to_owned())
            })
            .and_then(|body| ResourceCatalog::from_json(&body).map_err(|error| error.to_string()));
        match result {
            Ok(catalog) => {
                self.catalog = Some(catalog);
                self.error = None;
            }
            Err(error) if self.catalog.is_none() => self.error = Some(error),
            Err(_) => {}
        }
    }

    /// Render catalog content. Returns `true` when a validated catalog exists.
    pub(super) fn show(&mut self, ui: &mut egui::Ui) -> bool {
        self.poll_service_action(ui.ctx());
        let Some(catalog) = self.catalog.clone() else {
            return false;
        };
        let cards: Vec<_> = catalog
            .cards
            .iter()
            .filter(|card| self.matches_filter(card))
            .cloned()
            .collect();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("RESOURCE CATALOG")
                    .font(FontId::monospace(Style::SMALL))
                    .color(Style::TEXT_DIM),
            );
            ui.label(
                RichText::new(format!(
                    "REV {} · {} CARDS",
                    catalog.revision,
                    catalog.cards.len()
                ))
                .font(FontId::monospace(Style::SMALL))
                .color(Style::ACCENT),
            );
        });
        ui.add_space(Style::SP_XS);
        self.stack_hero(ui, &cards);
        ui.add_space(Style::SP_S);
        ui.horizontal_wrapped(|ui| {
            for filter in CatalogFilter::ALL {
                if ui
                    .selectable_label(self.filter == filter, filter.label())
                    .clicked()
                {
                    self.filter = filter;
                }
            }
        });
        ui.add_space(Style::SP_S);

        if cards.is_empty() {
            ui.label(RichText::new("No resources match this filter.").color(Style::TEXT_DIM));
        } else {
            let columns: usize = if ui.available_width() >= 680.0 { 2 } else { 1 };
            let gutters = Style::SP_S * (columns.saturating_sub(1) as f32);
            let width = ((ui.available_width() - gutters) / columns as f32).max(240.0);
            for row in cards.chunks(columns) {
                ui.horizontal(|ui| {
                    for card in row {
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 220.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| self.resource_card(ui, card),
                        );
                    }
                });
                ui.add_space(Style::SP_S);
            }
        }
        self.selected_detail(ui, &catalog);
        true
    }

    fn matches_filter(&self, card: &ResourceCard) -> bool {
        match self.filter {
            CatalogFilter::All => true,
            CatalogFilter::Desktops => card.identity.class == ResourceClass::Desktop,
            CatalogFilter::Applications => card.identity.class == ResourceClass::Application,
            CatalogFilter::MeshServices => card
                .service
                .as_ref()
                .is_some_and(|service| !service.stack.external),
            CatalogFilter::Media => card
                .service
                .as_ref()
                .is_some_and(|service| service.category == ServiceCategory::Media),
            CatalogFilter::Communications => card
                .service
                .as_ref()
                .is_some_and(|service| service.category == ServiceCategory::Communications),
            CatalogFilter::External => card
                .service
                .as_ref()
                .is_some_and(|service| service.stack.external),
        }
    }

    fn stack_hero(&mut self, ui: &mut egui::Ui, cards: &[ResourceCard]) {
        let height = if self.stack_expanded { 250.0 } else { 132.0 };
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 16.0, Style::SURFACE);
        painter.rect_stroke(
            rect,
            16.0,
            Stroke::new(1.0, Style::ACCENT_COMMS),
            StrokeKind::Inside,
        );
        let title = if self.stack_expanded {
            "LOCAL SERVICE STACK / LIVE TOPOLOGY · SELECT TO NEST"
        } else {
            "LOCAL SERVICE STACK / LIVE TOPOLOGY · SELECT TO UNNEST"
        };
        painter.text(
            rect.left_top() + egui::vec2(18.0, 15.0),
            egui::Align2::LEFT_TOP,
            title,
            FontId::monospace(13.0),
            Style::ACCENT_COMMS,
        );

        let tiers = [
            (ServiceStackTier::DesktopShell, "01  DESKTOP SHELL"),
            (ServiceStackTier::PlatformServices, "02  PLATFORM SERVICES"),
            (ServiceStackTier::MeshSubstrate, "03  MESH SUBSTRATE"),
        ];
        let lane_top = rect.top() + 43.0;
        let lane_height = (height - 56.0) / 3.0;
        for (index, (tier, label)) in tiers.iter().enumerate() {
            let lane = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 16.0, lane_top + index as f32 * lane_height),
                egui::vec2(rect.width() - 32.0, lane_height - 5.0),
            );
            painter.rect_stroke(
                lane,
                5.0,
                Stroke::new(0.75, Style::BORDER),
                StrokeKind::Inside,
            );
            painter.text(
                lane.left_center() + egui::vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                *label,
                FontId::monospace(10.0),
                Style::TEXT_DIM,
            );
            let mut x = lane.left() + 155.0;
            for card in cards.iter().filter(|card| {
                card.service
                    .as_ref()
                    .is_some_and(|service| service.stack.tier == *tier)
            }) {
                let service = card.service.as_ref().expect("filtered service");
                let node = egui::Rect::from_min_size(
                    egui::pos2(x, lane.top() + 7.0),
                    egui::vec2(92.0, lane.height() - 14.0),
                );
                painter.rect_filled(
                    node,
                    4.0,
                    lifecycle_color(service.lifecycle).gamma_multiply(0.16),
                );
                painter.rect_stroke(
                    node,
                    4.0,
                    Stroke::new(1.0, lifecycle_color(service.lifecycle)),
                    StrokeKind::Inside,
                );
                painter.text(
                    node.center(),
                    egui::Align2::CENTER_CENTER,
                    truncate(&card.display_name, 13),
                    FontId::monospace(9.0),
                    Style::TEXT_STRONG,
                );
                x += 100.0;
                if x + 92.0 > lane.right() {
                    break;
                }
            }
        }
        if response.clicked() {
            self.stack_expanded = !self.stack_expanded;
        }
    }

    fn resource_card(&mut self, ui: &mut egui::Ui, card: &ResourceCard) {
        let Some(service) = card.service.as_ref() else {
            self.plain_resource_card(ui, card);
            return;
        };
        let selected = self.selected_resource.as_deref() == Some(card.resource_id());
        let frame = egui::Frame::new()
            .fill(Style::SURFACE)
            .stroke(Stroke::new(
                if selected { 1.8 } else { 1.0 },
                lifecycle_color(service.lifecycle),
            ))
            .corner_radius(12.0)
            .inner_margin(14.0);
        let response = frame
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(service.service_kind.to_ascii_uppercase())
                            .font(FontId::monospace(12.0))
                            .color(Style::ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{:?}", service.lifecycle).to_ascii_uppercase())
                                .font(FontId::monospace(10.0))
                                .color(lifecycle_color(service.lifecycle)),
                        );
                    });
                });
                ui.label(RichText::new(&card.display_name).strong().color(Style::TEXT_STRONG));
                if let Some(summary) = &card.summary {
                    ui.label(RichText::new(summary).small().color(Style::TEXT_DIM));
                }
                ui.add_space(Style::SP_XS);
                mono_row(ui, "TIER", &format!("{:?}", service.stack.tier));
                mono_row(ui, "PLANE", &format!("{:?}", service.stack.plane));
                mono_row(
                    ui,
                    "WORKER",
                    service.stack.adapter_worker.as_deref().unwrap_or("direct/native"),
                );
                mono_row(
                    ui,
                    "HOSTS",
                    &if service.stack.external {
                        "EXTERNAL · LOCAL ADAPTER".to_owned()
                    } else if service.stack.hosting_nodes.is_empty() {
                        "DISCOVERING".to_owned()
                    } else {
                        service.stack.hosting_nodes.join(", ")
                    },
                );
                ui.add_space(Style::SP_XS);
                ui.horizontal_wrapped(|ui| {
                    for action in &card.actions {
                        let contract_ready = matches!(
                            action.availability.status,
                            mackes_mesh_types::resources::ActionAvailabilityStatus::Ready
                        );
                        let locally_handled = matches!(
                            action.verb,
                            ResourceActionVerb::Inspect
                                | ResourceActionVerb::Configure
                                | ResourceActionVerb::Test
                                | ResourceActionVerb::Enable
                                | ResourceActionVerb::Disable
                                | ResourceActionVerb::Remove
                        );
                        let button = ui.add_enabled(
                            contract_ready && locally_handled && self.action_pending.is_none(),
                            egui::Button::new(
                                RichText::new(format!("{:?}", action.verb).to_ascii_uppercase())
                                    .font(FontId::monospace(9.0)),
                            ),
                        );
                        if button.clicked() {
                            self.selected_resource = Some(card.resource_id().to_owned());
                            if action.verb == ResourceActionVerb::Configure {
                                self.configuration_draft.clear();
                                self.configuring_resource = Some(card.resource_id().to_owned());
                            } else if !matches!(action.verb, ResourceActionVerb::Inspect) {
                                self.start_service_action(
                                    action.verb,
                                    &service.service_kind,
                                    None,
                                );
                            }
                        }
                        if !locally_handled {
                            let _ = button.on_hover_text(
                                "This action remains disabled until its typed daemon provider publishes readiness.",
                            );
                        }
                    }
                });
            })
            .response;
        if response.clicked() {
            self.selected_resource = Some(card.resource_id().to_owned());
        }
    }

    fn plain_resource_card(&mut self, ui: &mut egui::Ui, card: &ResourceCard) {
        let selected = self.selected_resource.as_deref() == Some(card.resource_id());
        let color = match card.health.status {
            mackes_mesh_types::resources::HealthStatus::Available => Style::OK,
            mackes_mesh_types::resources::HealthStatus::Unavailable => Style::DANGER,
            _ => Style::TEXT_DIM,
        };
        let response = egui::Frame::new()
            .fill(Style::SURFACE)
            .stroke(Stroke::new(if selected { 1.8 } else { 1.0 }, color))
            .corner_radius(12.0)
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("{:?}", card.identity.class).to_ascii_uppercase())
                        .font(FontId::monospace(12.0))
                        .color(Style::ACCENT),
                );
                ui.label(
                    RichText::new(&card.display_name)
                        .strong()
                        .color(Style::TEXT_STRONG),
                );
                if let Some(summary) = &card.summary {
                    ui.label(RichText::new(summary).small().color(Style::TEXT_DIM));
                }
                ui.add_space(Style::SP_XS);
                mono_row(ui, "IDENTITY", card.resource_id());
                mono_row(ui, "HEALTH", &format!("{:?}", card.health.status));
                mono_row(
                    ui,
                    "ROLES",
                    &card
                        .operating_roles
                        .iter()
                        .map(|role| format!("{role:?}"))
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
                if ui.button("INSPECT").clicked() {
                    self.selected_resource = Some(card.resource_id().to_owned());
                }
            })
            .response;
        if response.clicked() {
            self.selected_resource = Some(card.resource_id().to_owned());
        }
    }

    fn selected_detail(&mut self, ui: &mut egui::Ui, catalog: &ResourceCatalog) {
        let Some(selected_id) = self.selected_resource.clone() else {
            return;
        };
        let Some(card) = catalog
            .cards
            .iter()
            .find(|candidate| candidate.resource_id() == selected_id)
        else {
            self.selected_resource = None;
            self.configuring_resource = None;
            self.configuration_draft.clear();
            return;
        };
        let Some(service) = card.service.as_ref() else {
            ui.add_space(Style::SP_M);
            egui::Frame::new()
                .fill(Style::SURFACE)
                .stroke(Stroke::new(1.0, Style::BORDER))
                .corner_radius(14.0)
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} / RESOURCE BLUEPRINT", card.display_name))
                                .font(FontId::monospace(13.0))
                                .color(Style::TEXT_STRONG),
                        );
                        if ui.button("Close").clicked() {
                            self.selected_resource = None;
                        }
                    });
                    mono_row(ui, "CLASS", &format!("{:?}", card.identity.class));
                    mono_row(ui, "IDENTITY", card.resource_id());
                    mono_row(ui, "HEALTH", &format!("{:?}", card.health.status));
                    mono_row(
                        ui,
                        "ROLES",
                        &card
                            .operating_roles
                            .iter()
                            .map(|role| format!("{role:?}"))
                            .collect::<Vec<_>>()
                            .join(" · "),
                    );
                });
            return;
        };

        ui.add_space(Style::SP_M);
        egui::Frame::new()
            .fill(Style::SURFACE)
            .stroke(Stroke::new(1.0, lifecycle_color(service.lifecycle)))
            .corner_radius(14.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} / LOCAL SERVICE STACK", card.display_name))
                            .font(FontId::monospace(13.0))
                            .color(Style::TEXT_STRONG),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.selected_resource = None;
                            self.configuring_resource = None;
                            self.configuration_draft.clear();
                        }
                    });
                });
                ui.add_space(Style::SP_XS);
                mono_row(ui, "TIER", &format!("{:?}", service.stack.tier));
                mono_row(ui, "PLANE", &format!("{:?}", service.stack.plane));
                mono_row(
                    ui,
                    "WORKER",
                    service.stack.adapter_worker.as_deref().unwrap_or("direct/native"),
                );

                if let Some(feedback) = &self.action_feedback {
                    ui.add_space(Style::SP_XS);
                    ui.label(
                        RichText::new(feedback)
                            .font(FontId::monospace(10.0))
                            .color(if feedback.contains("FAILED") {
                                Style::DANGER
                            } else {
                                Style::ACCENT
                            }),
                    );
                }
                mono_row(
                    ui,
                    "TRANSPORT",
                    service.stack.transport.as_deref().unwrap_or("not advertised"),
                );
                mono_row(
                    ui,
                    "CREDENTIAL",
                    service.stack.credential_ref.as_deref().unwrap_or("not required"),
                );
                mono_row(
                    ui,
                    "HOSTING",
                    &if service.stack.hosting_nodes.is_empty() {
                        "not placed".to_owned()
                    } else {
                        service.stack.hosting_nodes.join(", ")
                    },
                );
                mono_row(
                    ui,
                    "BUS",
                    &if service.stack.bus_topics.is_empty() {
                        "none".to_owned()
                    } else {
                        service.stack.bus_topics.join(" · ")
                    },
                );

                if !service.stack.dependencies.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("DEPENDS ")
                                .font(FontId::monospace(9.0))
                                .color(Style::TEXT_DIM),
                        );
                        for dependency in &service.stack.dependencies {
                            if ui.link(dependency).clicked() {
                                self.selected_resource = Some(dependency.clone());
                                self.configuring_resource = None;
                                self.configuration_draft.clear();
                            }
                        }
                    });
                }

                if self.configuring_resource.as_deref() == Some(selected_id.as_str()) {
                    ui.add_space(Style::SP_S);
                    ui.separator();
                    ui.add_space(Style::SP_S);
                    ui.label(
                        RichText::new("CONFIGURATION / SEALED VALUES")
                            .font(FontId::monospace(11.0))
                            .color(Style::ACCENT),
                    );
                    for field in &service.configuration_fields {
                        let key = format!("{selected_id}/{}", field.key);
                        let value = self.configuration_draft.entry(key).or_default();
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&field.label)
                                    .font(FontId::monospace(10.0))
                                    .color(Style::TEXT_DIM),
                            );
                            let mut edit = egui::TextEdit::singleline(value)
                                .desired_width((ui.available_width() - 12.0).max(120.0));
                            if field.kind == ServiceConfigurationFieldKind::Secret {
                                edit = edit.password(true);
                            }
                            ui.add(edit);
                        });
                    }
                    ui.label(
                        RichText::new(
                            "Secret values are masked and are never read back from their credential reference.",
                        )
                        .small()
                        .color(Style::TEXT_DIM),
                    );
                    if ui
                        .add_enabled(
                            self.action_pending.is_none(),
                            egui::Button::new("SAVE SEALED CONFIGURATION"),
                        )
                        .clicked()
                    {
                        let values = service
                            .configuration_fields
                            .iter()
                            .filter_map(|field| {
                                let key = format!("{selected_id}/{}", field.key);
                                self.configuration_draft
                                    .get(&key)
                                    .cloned()
                                    .map(|value| (field.key.clone(), value))
                            })
                            .collect::<BTreeMap<_, _>>();
                        let submission = serde_json::json!({
                            "service_kind": service.service_kind,
                            "values": values,
                        })
                        .to_string();
                        self.start_service_action(
                            ResourceActionVerb::Configure,
                            &service.service_kind,
                            Some(&submission),
                        );
                        // Secret inputs leave UI memory as soon as the bounded
                        // stdin payload has been handed to the worker thread.
                        self.configuration_draft.clear();
                        self.configuring_resource = None;
                    }
                }
            });
    }

    fn start_service_action(
        &mut self,
        verb: ResourceActionVerb,
        service_kind: &str,
        submission: Option<&str>,
    ) {
        if self.action_pending.is_some() {
            return;
        }
        let service_kind = service_kind.to_owned();
        let submission = submission.map(str::to_owned);
        let (sender, receiver) = mpsc::channel();
        self.action_feedback = Some("ACTION RUNNING · WAITING FOR ADAPTER".to_owned());
        self.action_pending = Some(receiver);
        thread::spawn(move || {
            let result = run_service_card_command(verb, &service_kind, submission.as_deref());
            let _ = sender.send(result);
        });
    }

    fn poll_service_action(&mut self, ctx: &egui::Context) {
        let Some(receiver) = self.action_pending.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(detail)) => {
                self.action_feedback = Some(detail);
                self.action_pending = None;
                self.refresh();
                ctx.request_repaint();
            }
            Ok(Err(error)) => {
                self.action_feedback = Some(format!("ACTION FAILED · {error}"));
                self.action_pending = None;
                self.refresh();
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.action_feedback = Some("ACTION FAILED · ADAPTER EXITED".to_owned());
                self.action_pending = None;
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100))
            }
        }
    }
}

fn run_service_card_command(
    verb: ResourceActionVerb,
    service_kind: &str,
    submission: Option<&str>,
) -> Result<String, String> {
    let mut command = Command::new("/usr/bin/mackesd");
    command.arg("service-card");
    match verb {
        ResourceActionVerb::Configure => {
            command.arg("save");
        }
        ResourceActionVerb::Test => {
            command.args(["test", service_kind]);
        }
        ResourceActionVerb::Enable => {
            command.args(["enable", service_kind]);
        }
        ResourceActionVerb::Disable => {
            command.args(["disable", service_kind]);
        }
        ResourceActionVerb::Remove => {
            command.args(["remove", service_kind]);
        }
        _ => return Err("service action has no privileged adapter".into()),
    }
    command
        .stdin(if submission.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start service-card responder: {error}"))?;
    if let Some(body) = submission {
        child
            .stdin
            .take()
            .ok_or_else(|| "service-card responder stdin was unavailable".to_owned())?
            .write_all(body.as_bytes())
            .map_err(|error| format!("write bounded service configuration: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for service-card responder: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let body: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "service-card responder returned malformed status".to_owned())?;
    let detail = body
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("service action completed");
    Ok(detail.to_ascii_uppercase())
}

fn mono_row(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{key:<7}"))
                .font(FontId::monospace(9.0))
                .color(Style::TEXT_DIM),
        );
        ui.label(
            RichText::new(value)
                .font(FontId::monospace(9.0))
                .color(Style::TEXT_STRONG),
        );
    });
}

fn lifecycle_color(status: ServiceLifecycleStatus) -> Color32 {
    match status {
        ServiceLifecycleStatus::Unconfigured => Style::ACCENT_COMMS,
        ServiceLifecycleStatus::Connecting => Style::ACCENT,
        ServiceLifecycleStatus::Healthy => Style::OK,
        ServiceLifecycleStatus::Degraded => Style::WARN,
        ServiceLifecycleStatus::Offline => Style::DANGER,
        ServiceLifecycleStatus::Disabled => Style::TEXT_DIM,
    }
}

fn truncate(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
