//! DATACENTER-18 — the New-Mesh **genesis** wizard ("give birth to a new Nebula").
//!
//! A stepped wizard that PLANS the genesis of a brand-new mesh and writes the
//! founding lighthouse's Tofu, leaving the irreversible live spend (the real
//! `tofu apply` of a DigitalOcean droplet) + the live `mackesd found` of the new
//! mesh as operator-gated steps. It mirrors the VM-Spawner ([`super::provisioning`])
//! shape — a state struct + a `Message` enum + an `update`→[`Task`] reducer + a
//! `view` — and talks to `mackesd` over the mde-bus action lane (`action/dc/<verb>`)
//! through the same `dbus` round-trip helpers the sibling panels use:
//!   * step 1 reuses `action/dc/do-regions` (DC-19) to populate the first-lighthouse
//!     region picker;
//!   * step 2 fires `action/dc/genesis-plan` (read-only) for the ordered plan +
//!     the Tofu resource preview + a `secrets_ready` warning (the `do-token`
//!     presence boolean — never the token);
//!   * step 3, behind an explicit **arm → confirm** gate, fires
//!     `action/dc/genesis-write` (`confirm:true`) to WRITE the founding lighthouse
//!     into `infra/tofu/zone1-do/dc-lighthouses.tf`.
//!
//! The wizard NEVER runs the live `tofu apply` or `mackesd found` — those are the
//! gated steps (the operator runs the apply from the Datacenter panel's Infra tab,
//! and the founding `mackesd found` runs on the booted droplet via the founding
//! cloud-init). This is the same DC-19/DC-20 honesty: plan + Tofu-write here, the
//! real spend stays operator-gated. No secret is ever rendered, logged, or echoed.

use std::time::Duration;

use cosmic::iced::widget::{column, pick_list, row, text, Space};
use cosmic::iced::{Length, Task};
use cosmic::Element;
use mde_theme::{EmptyState, Icon};

use crate::controls::{styled_text_input, variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{
    card, empty_state, error_state, panel_container, section_block, status_badge, BadgeSeverity,
};

/// Read budget for the `do-regions` probe — a local `doctl region list`.
const REGIONS_TIMEOUT: Duration = Duration::from_secs(12);
/// Read budget for the `genesis-plan` probe — validation + a secret-store presence
/// probe (a `bash -lc` shell-out to the mesh secret helper). A few seconds is ample.
const PLAN_TIMEOUT: Duration = Duration::from_secs(12);
/// Write budget for the `genesis-write` op — a small file write under the repo.
const WRITE_TIMEOUT: Duration = Duration::from_secs(12);

/// Action topic the region picker probes (DC-19, reused).
const REGIONS_TOPIC: &str = "action/dc/do-regions";
/// Action topic the plan step fires (read-only).
const PLAN_TOPIC: &str = "action/dc/genesis-plan";
/// Action topic the execute step fires (structural Tofu-write, confirm-gated).
const WRITE_TOPIC: &str = "action/dc/genesis-write";
/// DAR-45 — action topic the backoffice step fires (read-only planner probe).
const BACKOFFICE_TOPIC: &str = "action/dc/backoffice-plan";
/// Read budget for the `backoffice-plan` probe — a `bash` shell-out that reads the
/// tier manifest + a secret-store presence probe. A few seconds is ample.
const BACKOFFICE_TIMEOUT: Duration = Duration::from_secs(12);

/// One DO region decoded from a `do-regions` reply.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Region {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub available: bool,
}

/// The resolved genesis plan decoded from a `genesis-plan` reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The ordered step labels (generate CA → provision → found → seed → DNS → token).
    pub steps: Vec<String>,
    /// The Tofu resource address the write would land.
    pub resource: String,
    /// The repo-relative `.tf` path the write targets.
    pub path: String,
    /// The gated workspace the live apply runs against (`zone1-do`).
    pub workspace: String,
    /// Whether the `do-token` secret is already in the mesh store — a false here
    /// means a live apply would fail for lack of the DO credential (the wizard
    /// warns, but still lets the plan/write proceed; the apply is gated anyway).
    pub secrets_ready: bool,
}

/// DAR-45 — one rendered backoffice unit decoded from a `backoffice-plan` reply.
/// Mirrors `backoffice-plan.sh`'s JSON unit row (the REAL rendered plan, not canned).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct BackofficeUnit {
    pub id: String,
    #[serde(default)]
    pub phase: u32,
    #[serde(default)]
    pub live_gated: bool,
    #[serde(default)]
    pub via_script: String,
}

/// DAR-45 — the resolved backoffice plan decoded from a `backoffice-plan` reply
/// (the genesis-wizard's backoffice step). The ordered unit list is the EXACT
/// output of `backoffice-plan.sh --tier <t>` — a real probe, never a canned list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackofficePlan {
    /// The tier this plan was rendered for (`minimal` | `full`).
    pub tier: String,
    /// The ordered units the orchestrator would enable (precheck → … → DR).
    pub units: Vec<BackofficeUnit>,
    /// Whether the `do-token` secret is already in the mesh store (the same probe
    /// the genesis-plan step uses) — a false warns the operator before a live apply.
    pub secrets_ready: bool,
}

/// The wizard's current step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Step {
    /// Step 1 — name the new mesh + pick the first-lighthouse region.
    #[default]
    Name,
    /// Step 2 — review the resolved plan (the `genesis-plan` reply).
    Review,
    /// Step 3 (DAR-45) — DevOps backoffice: pick a tier (off|minimal|full) and
    /// preview the rendered unit list the backoffice would enable. Optional.
    Backoffice,
    /// Step 4 — execute: arm → confirm → write the founding Tofu.
    Execute,
}

#[derive(Debug, Clone, Default)]
pub struct GenesisPanel {
    /// Current wizard step.
    pub step: Step,
    /// Step-1 form: the new mesh id as typed (sanitized to a DNS-ish label).
    pub mesh_id: String,
    /// Step-1 form: the selected first-lighthouse DO region slug.
    pub region: Option<String>,
    /// The region roster (from `do-regions`); only `available` slugs are pickable.
    pub regions: Vec<Region>,
    /// The resolved plan (step 2 onward), once `genesis-plan` has answered.
    pub plan: Option<Plan>,
    /// Step-3 arm gate: the operator armed the destructive write (a second,
    /// explicit Confirm then fires it). Reset whenever the form changes.
    pub armed: bool,
    /// An op (plan / write) is in flight — buttons disabled while set.
    pub busy: bool,
    /// Last status / outcome line.
    pub status: String,
    /// Set when the regions LOAD itself failed (vs an empty roster) — the view
    /// renders the error state, never a misleading empty picker.
    pub load_error: Option<String>,
    /// Set once the founding Tofu has been written (the gated apply is next).
    pub written: bool,
    /// DAR-45 — the chosen DevOps backoffice tier: `None` = OFF (default, behavior
    /// unchanged), `Some("minimal")` / `Some("full")` = opt in. Carried into the
    /// `genesis-write` body as `backoffice_tier` so the reply echoes the intent.
    pub backoffice_tier: Option<String>,
    /// DAR-45 — the rendered backoffice plan (from `backoffice-plan`), once a tier
    /// has been selected. None when OFF or not yet probed.
    pub backoffice_plan: Option<BackofficePlan>,
    /// DAR-45 — a backoffice-plan probe is in flight.
    pub backoffice_busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The `do-regions` roster loaded (or failed) on panel entry.
    RegionsLoaded(Result<Vec<Region>, String>),
    /// Reload the region roster.
    RefreshClicked,
    /// Step-1 mesh-id input changed (sanitized).
    MeshIdChanged(String),
    /// Step-1 region picked.
    RegionSelected(String),
    /// Step 1 → 2: fire `genesis-plan` for the typed name + region.
    PlanClicked,
    /// The `genesis-plan` reply arrived (or failed).
    PlanFinished(Result<Plan, String>),
    /// Back to step 1 (re-edit the name/region).
    BackClicked,
    /// Step 3: arm the destructive write (first half of arm→confirm).
    ArmClicked,
    /// Step 3: confirm + fire `genesis-write` (`confirm:true`).
    ConfirmWriteClicked,
    /// Disarm without writing.
    CancelArmClicked,
    /// The `genesis-write` reply arrived (or failed).
    WriteFinished(Result<String, String>),
    /// Reset the wizard to step 1 (start another genesis).
    StartOverClicked,
    /// DAR-45 — Review → Backoffice: advance to the backoffice tier step.
    ContinueToBackofficeClicked,
    /// DAR-45 — pick the backoffice tier (`None` = OFF). A non-off pick fires the
    /// read-only `backoffice-plan` RPC to render the unit list.
    BackofficeTierSelected(Option<String>),
    /// DAR-45 — the `backoffice-plan` reply arrived (or failed).
    BackofficePlanFinished(Result<BackofficePlan, String>),
    /// DAR-45 — Backoffice → Execute: proceed to the founding Tofu write.
    ContinueToExecuteClicked,
}

impl GenesisPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Probe the `do-regions` roster on panel entry so the region picker lands
    /// populated. Blocking Bus round-trip on `spawn_blocking`.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_regions)
                    .await
                    .unwrap_or_else(|_| Err("genesis region probe task panicked".into()))
            },
            |result| crate::Message::Genesis(Message::RegionsLoaded(result)),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::RegionsLoaded(Ok(regions)) => {
                self.regions = regions;
                self.load_error = None;
                self.busy = false;
                // Default the region to the first AVAILABLE one if unpicked / stale.
                let valid = self
                    .region
                    .as_deref()
                    .is_some_and(|r| self.regions.iter().any(|reg| reg.slug == r));
                if !valid {
                    self.region = self
                        .regions
                        .iter()
                        .find(|r| r.available)
                        .map(|r| r.slug.clone());
                }
                if self.status.is_empty() {
                    self.status = format!("{} DO region(s) available.", self.regions.len());
                }
                Task::none()
            }
            Message::RegionsLoaded(Err(e)) => {
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                self.status = "Refreshing regions…".into();
                Self::load()
            }
            Message::MeshIdChanged(v) => {
                self.mesh_id = sanitize_mesh_id(&v);
                // Any form change invalidates a stale plan / arm / backoffice plan.
                self.plan = None;
                self.armed = false;
                self.backoffice_plan = None;
                Task::none()
            }
            Message::RegionSelected(r) => {
                self.region = Some(r);
                self.plan = None;
                self.armed = false;
                self.backoffice_plan = None;
                Task::none()
            }
            Message::PlanClicked => {
                if self.busy {
                    return Task::none();
                }
                let mesh_id = self.mesh_id.trim().to_string();
                if !mesh_id_valid(&mesh_id) {
                    self.status =
                        "Mesh id must be a DNS label: lowercase letters, digits, hyphens.".into();
                    return Task::none();
                }
                let Some(region) = self.region.clone() else {
                    self.status = "Pick a first-lighthouse region first.".into();
                    return Task::none();
                };
                self.busy = true;
                self.status = format!("Planning genesis of \"{mesh_id}\"…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || plan_genesis(&mesh_id, &region))
                            .await
                            .unwrap_or_else(|_| Err("genesis-plan task panicked".into()))
                    },
                    |res| crate::Message::Genesis(Message::PlanFinished(res)),
                )
            }
            Message::PlanFinished(Ok(plan)) => {
                self.busy = false;
                let warn = if plan.secrets_ready {
                    String::new()
                } else {
                    "  (warning: do-token not in the secret store — provision it before \
                     a live apply)"
                        .to_string()
                };
                self.status = format!("Plan ready: {} step(s).{warn}", plan.steps.len());
                self.plan = Some(plan);
                self.step = Step::Review;
                Task::none()
            }
            Message::PlanFinished(Err(e)) => {
                self.busy = false;
                self.status = e;
                Task::none()
            }
            Message::BackClicked => {
                // Step-relative back: Execute → Backoffice → Review → Name. Esc/back
                // walks the wizard one step at a time (DAR-45 — back works).
                self.step = match self.step {
                    Step::Execute => Step::Backoffice,
                    Step::Backoffice => Step::Review,
                    Step::Review | Step::Name => Step::Name,
                };
                self.armed = false;
                Task::none()
            }
            Message::ArmClicked => {
                // Move to the execute step (or arm in place if already there).
                self.step = Step::Execute;
                self.armed = true;
                self.status =
                    "Armed — Confirm to WRITE the founding Tofu (no live apply yet).".into();
                Task::none()
            }
            Message::CancelArmClicked => {
                self.armed = false;
                self.status = "Disarmed.".into();
                Task::none()
            }
            Message::ConfirmWriteClicked => {
                if self.busy || !self.armed {
                    return Task::none();
                }
                let mesh_id = self.mesh_id.trim().to_string();
                let Some(region) = self.region.clone() else {
                    self.status = "No region selected.".into();
                    return Task::none();
                };
                if !mesh_id_valid(&mesh_id) {
                    self.status = "Mesh id is no longer valid — go back and fix it.".into();
                    return Task::none();
                }
                self.busy = true;
                self.armed = false;
                self.status = format!("Writing founding Tofu for \"{mesh_id}\"…");
                // DAR-45 — carry the chosen backoffice tier into the write body so
                // the `genesis-write` reply echoes `backoffice_intent {tier}`.
                let backoffice_tier = self.backoffice_tier.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            write_genesis(&mesh_id, &region, backoffice_tier.as_deref())
                        })
                        .await
                        .unwrap_or_else(|_| Err("genesis-write task panicked".into()))
                    },
                    |res| crate::Message::Genesis(Message::WriteFinished(res)),
                )
            }
            Message::WriteFinished(Ok(msg)) => {
                self.busy = false;
                self.written = true;
                self.status = msg;
                Task::none()
            }
            Message::WriteFinished(Err(e)) => {
                self.busy = false;
                self.status = e;
                Task::none()
            }
            Message::StartOverClicked => {
                // Keep the loaded region roster; reset the form + flow.
                self.step = Step::Name;
                self.mesh_id.clear();
                self.plan = None;
                self.armed = false;
                self.written = false;
                self.backoffice_tier = None;
                self.backoffice_plan = None;
                self.backoffice_busy = false;
                self.status = "Ready for a new mesh.".into();
                Task::none()
            }
            Message::ContinueToBackofficeClicked => {
                // Review → Backoffice (DAR-45). The backoffice opt-in is OPTIONAL;
                // the operator can leave it OFF and continue to Execute unchanged.
                self.step = Step::Backoffice;
                self.armed = false;
                self.status = "Optional: deploy the DevOps backoffice with this mesh?".into();
                Task::none()
            }
            Message::BackofficeTierSelected(tier) => {
                self.backoffice_tier = tier.clone();
                match tier {
                    None => {
                        // OFF — no plan to render; clear any stale one.
                        self.backoffice_plan = None;
                        self.status =
                            "Backoffice OFF (genesis writes the founding Tofu only).".into();
                        Task::none()
                    }
                    Some(t) => {
                        if self.backoffice_busy {
                            return Task::none();
                        }
                        self.backoffice_busy = true;
                        self.backoffice_plan = None;
                        self.status = format!("Planning the {t} backoffice…");
                        let tier_for_task = t.clone();
                        Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || plan_backoffice(&tier_for_task))
                                    .await
                                    .unwrap_or_else(|_| Err("backoffice-plan task panicked".into()))
                            },
                            |res| crate::Message::Genesis(Message::BackofficePlanFinished(res)),
                        )
                    }
                }
            }
            Message::BackofficePlanFinished(Ok(plan)) => {
                self.backoffice_busy = false;
                let warn = if plan.secrets_ready {
                    String::new()
                } else {
                    "  (warning: do-token not in the secret store — provision it before \
                     the backoffice can apply infra)"
                        .to_string()
                };
                self.status = format!(
                    "Backoffice plan ready: {} {}-tier unit(s).{warn}",
                    plan.units.len(),
                    plan.tier
                );
                self.backoffice_plan = Some(plan);
                Task::none()
            }
            Message::BackofficePlanFinished(Err(e)) => {
                self.backoffice_busy = false;
                self.backoffice_plan = None;
                self.status = e;
                Task::none()
            }
            Message::ContinueToExecuteClicked => {
                // Backoffice → Execute. Carries the chosen tier into the write body.
                self.step = Step::Execute;
                self.armed = false;
                self.status = match &self.backoffice_tier {
                    Some(t) => format!("Backoffice={t} will be recorded with the genesis."),
                    None => "Backoffice OFF — Arm, then Confirm to write the founding Tofu.".into(),
                };
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;

        // A failed region probe renders as failure, never as an empty picker.
        if let Some(err) = &self.load_error {
            return panel_container(
                error_state(err.clone(), palette, || {
                    crate::Message::Genesis(Message::RefreshClicked)
                }),
                density,
            );
        }

        let header = row![
            text("New Mesh — Genesis").size(20).width(Length::Fill),
            variant_button(
                "Refresh",
                ButtonVariant::Ghost,
                (!self.busy).then_some(crate::Message::Genesis(Message::RefreshClicked)),
                palette,
            ),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let body = column![
            header,
            self.stepper(palette),
            self.step_body(palette, density),
            text(&self.status).size(13),
        ]
        .spacing(16)
        .width(Length::Fill);

        panel_container(body.into(), density)
    }

    /// A compact step indicator (1 Name · 2 Review · 3 Execute) — the current
    /// step badged Info, the others Neutral.
    fn stepper(&self, palette: mde_theme::Palette) -> Element<'_, crate::Message> {
        let badge = |label: &str, on: bool| {
            status_badge(
                label,
                if on {
                    BadgeSeverity::Info
                } else {
                    BadgeSeverity::Neutral
                },
                palette,
            )
        };
        row![
            badge("1 · Name the mesh", self.step == Step::Name),
            badge("2 · Review plan", self.step == Step::Review),
            badge("3 · Backoffice", self.step == Step::Backoffice),
            badge("4 · Execute", self.step == Step::Execute),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .into()
    }

    /// Render the body for the current step.
    fn step_body(
        &self,
        palette: mde_theme::Palette,
        density: mde_theme::Density,
    ) -> Element<'_, crate::Message> {
        match self.step {
            Step::Name => self.name_step(palette, density),
            Step::Review => self.review_step(palette, density),
            Step::Backoffice => self.backoffice_step(palette, density),
            Step::Execute => self.execute_step(palette, density),
        }
    }

    /// Step 1 — name the new mesh + pick the first-lighthouse region.
    fn name_step(
        &self,
        palette: mde_theme::Palette,
        density: mde_theme::Density,
    ) -> Element<'_, crate::Message> {
        let name_input = styled_text_input(
            "new mesh id (e.g. home-mesh)",
            &self.mesh_id,
            |v| crate::Message::Genesis(Message::MeshIdChanged(v)),
            palette,
        );

        // Only AVAILABLE regions are pickable first-lighthouse targets.
        let region_choices: Vec<String> = self
            .regions
            .iter()
            .filter(|r| r.available)
            .map(|r| r.slug.clone())
            .collect();

        let region_picker: Element<'_, crate::Message> = if region_choices.is_empty() {
            text("no available DO region (is doctl authed? Refresh)")
                .size(13)
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            pick_list(region_choices, self.region.clone(), |v| {
                crate::Message::Genesis(Message::RegionSelected(v))
            })
            .into()
        };

        let can_plan = !self.busy && mesh_id_valid(self.mesh_id.trim()) && self.region.is_some();
        let plan_btn = variant_button(
            "Plan genesis →",
            ButtonVariant::Primary,
            can_plan.then_some(crate::Message::Genesis(Message::PlanClicked)),
            palette,
        );

        let form = column![
            text(
                "Give birth to a new Nebula: name the mesh and pick the region for its \
                 first (founding) lighthouse. The wizard plans the genesis — generate \
                 CA, provision the lighthouse droplet, found the mesh, seed it, register \
                 DNS, and emit the first join token — then writes the founding Tofu. The \
                 live droplet spend (tofu apply) and the `mackesd found` of a real mesh \
                 stay operator-gated."
            )
            .size(13),
            row![name_input, region_picker, plan_btn]
                .spacing(12)
                .align_y(cosmic::iced::alignment::Vertical::Center),
        ]
        .spacing(12);

        section_block("Name the new mesh", form.into(), palette, density)
    }

    /// Step 2 — review the resolved plan.
    fn review_step(
        &self,
        palette: mde_theme::Palette,
        density: mde_theme::Density,
    ) -> Element<'_, crate::Message> {
        let Some(plan) = &self.plan else {
            // No plan resolved (shouldn't happen on this step) — offer a way back.
            let state =
                EmptyState::with_cta("No plan yet", "Go back and plan the genesis first.", "Back")
                    .with_icon(Icon::Fleet);
            return empty_state(state, palette, || {
                crate::Message::Genesis(Message::BackClicked)
            });
        };

        let mesh_id = self.mesh_id.trim();
        let region = self.region.as_deref().unwrap_or("?");

        let steps = plan
            .steps
            .iter()
            .enumerate()
            .fold(column![].spacing(6), |col, (i, s)| {
                col.push(text(format!("{}. {s}", i + 1)).size(13))
            });

        let secrets_badge = if plan.secrets_ready {
            status_badge("do-token ready", BadgeSeverity::Success, palette)
        } else {
            status_badge("do-token MISSING", BadgeSeverity::Warning, palette)
        };

        let summary = column![
            text(format!("Mesh \"{mesh_id}\" — first lighthouse in {region}")).size(15),
            row![
                text("genesis secret:")
                    .size(13)
                    .colr(palette.text_muted.into_cosmic_color()),
                secrets_badge,
            ]
            .spacing(8)
            .align_y(cosmic::iced::alignment::Vertical::Center),
            text(format!("Tofu resource: {}", plan.resource)).size(13),
            text(format!("writes: {}", plan.path)).size(13),
            text(format!("gated apply workspace: {}", plan.workspace)).size(13),
        ]
        .spacing(6);

        let actions = row![
            variant_button(
                "← Back",
                ButtonVariant::Ghost,
                (!self.busy).then_some(crate::Message::Genesis(Message::BackClicked)),
                palette,
            ),
            Space::new().width(Length::Fill),
            variant_button(
                "Continue →",
                ButtonVariant::Primary,
                (!self.busy).then_some(crate::Message::Genesis(
                    Message::ContinueToBackofficeClicked
                )),
                palette,
            ),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let block = column![
            summary,
            section_block("Genesis steps", steps.into(), palette, density),
            actions,
        ]
        .spacing(16);

        section_block("Review the genesis plan", block.into(), palette, density)
    }

    /// Step 3 (DAR-45) — the DevOps backoffice opt-in: pick a tier and preview the
    /// REAL rendered unit list (`action/dc/backoffice-plan`, never a canned list).
    /// Optional — OFF is the default and leaves genesis behavior unchanged.
    fn backoffice_step(
        &self,
        palette: mde_theme::Palette,
        density: mde_theme::Density,
    ) -> Element<'_, crate::Message> {
        // The tier toggle: Off | Minimal | Full. The current selection is Primary,
        // the others Secondary — mde-theme variant buttons (no raw hex / metrics).
        let tier_btn = |label: &str, val: Option<&str>| {
            let selected = self.backoffice_tier.as_deref() == val;
            let variant = if selected {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Secondary
            };
            let owned = val.map(ToString::to_string);
            variant_button(
                label,
                variant,
                (!self.backoffice_busy).then_some(crate::Message::Genesis(
                    Message::BackofficeTierSelected(owned),
                )),
                palette,
            )
        };

        let toggle = row![
            text("Deploy DevOps backoffice?")
                .size(13)
                .colr(palette.text_muted.into_cosmic_color()),
            tier_btn("Off", None),
            tier_btn("Minimal", Some("minimal")),
            tier_btn("Full", Some("full")),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        // The rendered plan (when a non-off tier was probed): the secrets chip + the
        // ordered unit list, each badged live-gated vs plain.
        let plan_block: Element<'_, crate::Message> = if self.backoffice_busy {
            text("Planning the backoffice…").size(13).into()
        } else if let Some(plan) = &self.backoffice_plan {
            let secrets_badge = if plan.secrets_ready {
                status_badge("do-token ready", BadgeSeverity::Success, palette)
            } else {
                status_badge("do-token MISSING", BadgeSeverity::Warning, palette)
            };
            let units = plan
                .units
                .iter()
                .enumerate()
                .fold(column![].spacing(6), |col, (i, u)| {
                    let gate = if u.live_gated {
                        status_badge("live-gated", BadgeSeverity::Warning, palette)
                    } else {
                        status_badge("safe", BadgeSeverity::Neutral, palette)
                    };
                    col.push(
                        row![
                            text(format!("{}. p{} {}", i + 1, u.phase, u.id)).size(13),
                            gate,
                        ]
                        .spacing(8)
                        .align_y(cosmic::iced::alignment::Vertical::Center),
                    )
                });
            column![
                row![
                    text("secret store:")
                        .size(13)
                        .colr(palette.text_muted.into_cosmic_color()),
                    secrets_badge,
                ]
                .spacing(8)
                .align_y(cosmic::iced::alignment::Vertical::Center),
                section_block(
                    format!("{}-tier units (rendered by backoffice-plan)", plan.tier),
                    units.into(),
                    palette,
                    density,
                ),
            ]
            .spacing(12)
            .into()
        } else {
            text(
                "Off: genesis writes the founding Tofu only. Pick Minimal or Full to \
                 preview the backoffice units that would be enabled — the live bring-up \
                 (`backoffice-up.sh`) stays operator-gated on the control VM.",
            )
            .size(13)
            .colr(palette.text_muted.into_cosmic_color())
            .into()
        };

        let actions = row![
            variant_button(
                "← Back",
                ButtonVariant::Ghost,
                (!self.backoffice_busy).then_some(crate::Message::Genesis(Message::BackClicked)),
                palette,
            ),
            Space::new().width(Length::Fill),
            variant_button(
                "Continue to execute →",
                ButtonVariant::Primary,
                (!self.backoffice_busy)
                    .then_some(crate::Message::Genesis(Message::ContinueToExecuteClicked)),
                palette,
            ),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let block = column![toggle, plan_block, actions].spacing(16);
        section_block(
            "DevOps backoffice (optional)",
            block.into(),
            palette,
            density,
        )
    }

    /// Step 4 — execute: arm → confirm → write the founding Tofu (the only thing
    /// that runs here). The live `tofu apply` + `mackesd found` stay gated.
    fn execute_step(
        &self,
        palette: mde_theme::Palette,
        density: mde_theme::Density,
    ) -> Element<'_, crate::Message> {
        let mesh_id = self.mesh_id.trim();

        // Once written, show the completion + the gated next steps.
        if self.written {
            let next = column![
                text(format!("Founding Tofu for \"{mesh_id}\" written.")).size(15),
                text(
                    "Next (operator-gated — NOT run by the wizard):\n  \
                     1. Provision: run `tofu apply` on the zone1-do workspace from the \
                     Datacenter → Infra tab (real DO droplet spend).\n  \
                     2. Found: the founding cloud-init runs `mackesd found` on the booted \
                     droplet, minting the CA + the first join token.\n  \
                     3. The DNS A-record + join token come from those steps."
                )
                .size(13),
                variant_button(
                    "Start another mesh",
                    ButtonVariant::Secondary,
                    (!self.busy).then_some(crate::Message::Genesis(Message::StartOverClicked)),
                    palette,
                ),
            ]
            .spacing(12);
            return section_block("Genesis written", next.into(), palette, density);
        }

        let warning = text(
            "This WRITES the founding lighthouse into the zone1-do Tofu. It does NOT \
             apply (no droplet is created) and does NOT found a mesh — those are the \
             gated live steps. Arm, then Confirm to write.",
        )
        .size(13)
        .colr(palette.text_muted.into_cosmic_color());

        let controls: Element<'_, crate::Message> = if self.armed {
            row![
                status_badge("ARMED", BadgeSeverity::Danger, palette),
                Space::new().width(Length::Fill),
                variant_button(
                    "Cancel",
                    ButtonVariant::Ghost,
                    (!self.busy).then_some(crate::Message::Genesis(Message::CancelArmClicked)),
                    palette,
                ),
                variant_button(
                    "Confirm — write founding Tofu",
                    ButtonVariant::Primary,
                    (!self.busy).then_some(crate::Message::Genesis(Message::ConfirmWriteClicked)),
                    palette,
                ),
            ]
            .spacing(12)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
        } else {
            row![
                variant_button(
                    "← Back",
                    ButtonVariant::Ghost,
                    (!self.busy).then_some(crate::Message::Genesis(Message::BackClicked)),
                    palette,
                ),
                Space::new().width(Length::Fill),
                variant_button(
                    "Arm",
                    ButtonVariant::Secondary,
                    (!self.busy).then_some(crate::Message::Genesis(Message::ArmClicked)),
                    palette,
                ),
            ]
            .spacing(12)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
        };

        let block = column![warning, controls].spacing(16);
        card(
            section_block("Execute — found a new mesh", block.into(), palette, density),
            palette,
            density,
        )
    }
}

// ---- bus I/O (all blocking — call from spawn_blocking) -----------------------

/// Probe `action/dc/do-regions` for the region roster. Blocking — call from
/// `spawn_blocking`. An unreachable mackesd / a doctl failure is a load error.
fn fetch_regions() -> Result<Vec<Region>, String> {
    let raw = crate::dbus::action_request(REGIONS_TOPIC, REGIONS_TIMEOUT)
        .ok_or("mackesd not reachable over the Bus — genesis unavailable")?;
    if let Some(e) = crate::dbus::reply_error(&raw) {
        return Err(format!("do-regions failed: {e}"));
    }
    Ok(parse_regions(&raw))
}

/// Fire `action/dc/genesis-plan` for `mesh_id` + `region` (read-only). Blocking.
fn plan_genesis(mesh_id: &str, region: &str) -> Result<Plan, String> {
    let body = serde_json::json!({ "mesh_id": mesh_id, "region": region });
    let reply =
        crate::dbus::action_request_with_body(PLAN_TOPIC, Some(&body.to_string()), PLAN_TIMEOUT)
            .ok_or("mackesd not reachable over the Bus (genesis-plan)")?;
    if let Some(e) = crate::dbus::reply_error(&reply) {
        return Err(format!("genesis-plan failed: {e}"));
    }
    parse_plan(&reply)
}

/// Fire `action/dc/genesis-write` (`confirm:true`) for `mesh_id` + `region` — the
/// structural Tofu-write. Blocking. `backoffice_tier` (DAR-45) is carried into the
/// body when the operator opted into the backoffice (`minimal`/`full`), so the
/// reply echoes `backoffice_intent {tier}`. Returns a status line on success.
fn write_genesis(
    mesh_id: &str,
    region: &str,
    backoffice_tier: Option<&str>,
) -> Result<String, String> {
    let mut body = serde_json::json!({ "mesh_id": mesh_id, "region": region, "confirm": true });
    if let Some(tier) = backoffice_tier {
        body["backoffice_tier"] = serde_json::Value::String(tier.to_string());
    }
    let reply =
        crate::dbus::action_request_with_body(WRITE_TOPIC, Some(&body.to_string()), WRITE_TIMEOUT)
            .ok_or("mackesd not reachable over the Bus (genesis-write)")?;
    if let Some(e) = crate::dbus::reply_error(&reply) {
        return Err(format!("genesis-write failed: {e}"));
    }
    let v: serde_json::Value = serde_json::from_str(&reply)
        .map_err(|e| format!("genesis-write reply not decodable: {e}"))?;
    let path = v["path"]
        .as_str()
        .unwrap_or("infra/tofu/zone1-do/dc-lighthouses.tf");
    let bo = v["backoffice_intent"]["tier"]
        .as_str()
        .map(|t| format!(" Backoffice intent ({t}) recorded."))
        .unwrap_or_default();
    Ok(format!(
        "Wrote founding lighthouse to {path}. Apply (gated) to provision.{bo}"
    ))
}

/// DAR-45 — fire `action/dc/backoffice-plan` for `tier` (read-only). Blocking —
/// call from `spawn_blocking`. Returns the REAL rendered plan (the same units
/// `backoffice-plan.sh --tier <t>` emits), never a canned list.
fn plan_backoffice(tier: &str) -> Result<BackofficePlan, String> {
    let body = serde_json::json!({ "tier": tier });
    let reply = crate::dbus::action_request_with_body(
        BACKOFFICE_TOPIC,
        Some(&body.to_string()),
        BACKOFFICE_TIMEOUT,
    )
    .ok_or("mackesd not reachable over the Bus (backoffice-plan)")?;
    if let Some(e) = crate::dbus::reply_error(&reply) {
        return Err(format!("backoffice-plan failed: {e}"));
    }
    parse_backoffice_plan(&reply)
}

// ---- pure helpers (parse / validate) -----------------------------------------

/// Decode the `{"regions":[…]}` `do-regions` reply.
#[must_use]
fn parse_regions(raw: &str) -> Vec<Region> {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    v.get("regions")
        .and_then(|a| serde_json::from_value::<Vec<Region>>(a.clone()).ok())
        .unwrap_or_default()
}

/// Decode a `genesis-plan` reply into a [`Plan`].
fn parse_plan(raw: &str) -> Result<Plan, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("genesis-plan reply not decodable: {e}"))?;
    let steps = v
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if steps.is_empty() {
        return Err("genesis-plan returned no steps".into());
    }
    Ok(Plan {
        steps,
        resource: v
            .get("resource")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        path: v
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        workspace: v
            .get("workspace")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        secrets_ready: v
            .get("secrets_ready")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// DAR-45 — decode a `backoffice-plan` reply into a [`BackofficePlan`]. The
/// `units` array is the EXACT shape `backoffice-plan.sh` emits (id/phase/
/// live_gated/via_script); `secrets_ready` is the daemon's re-stamped bool.
fn parse_backoffice_plan(raw: &str) -> Result<BackofficePlan, String> {
    let v: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| format!("backoffice-plan reply not decodable: {e}"))?;
    let units = v
        .get("units")
        .and_then(|a| serde_json::from_value::<Vec<BackofficeUnit>>(a.clone()).ok())
        .unwrap_or_default();
    if units.is_empty() {
        return Err("backoffice-plan returned no units".into());
    }
    Ok(BackofficePlan {
        tier: v
            .get("tier")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        units,
        secrets_ready: v
            .get("secrets_ready")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// Sanitize a mesh id as typed: keep only the DNS-label charset (ASCII lowercase
/// alphanumeric + hyphen). Uppercase is folded to lowercase as a convenience.
#[must_use]
fn sanitize_mesh_id(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            if c.is_ascii_uppercase() {
                Some(c.to_ascii_lowercase())
            } else if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

/// Validate a mesh id: a DNS-ish label — non-empty, `[a-z0-9-]`, no
/// leading/trailing hyphen, at most 63 chars. Mirrors the backend's
/// `genesis_mesh_id_valid` so the button arms only when the daemon will accept it.
#[must_use]
fn mesh_id_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 63
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_regions_decodes_the_reply() {
        let raw = r#"{"ok":true,"regions":[
            {"slug":"nyc3","name":"New York 3","available":true},
            {"slug":"ams2","name":"Amsterdam 2","available":false}
        ]}"#;
        let regions = parse_regions(raw);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].slug, "nyc3");
        assert!(regions[0].available);
        assert!(!regions[1].available);
    }

    #[test]
    fn parse_regions_tolerates_garbage_and_missing_key() {
        assert!(parse_regions("not json").is_empty());
        assert!(parse_regions("{}").is_empty());
        assert!(parse_regions(r#"{"regions":[]}"#).is_empty());
    }

    #[test]
    fn parse_plan_decodes_the_reply() {
        let raw = r#"{"ok":true,"mesh_id":"home-mesh","region":"nyc3",
            "steps":["generate CA","provision","found","seed","DNS","token"],
            "resource":"digitalocean_droplet.lighthouse_lh_home_mesh_01",
            "path":"infra/tofu/zone1-do/dc-lighthouses.tf",
            "workspace":"zone1-do","secrets_ready":false}"#;
        let plan = parse_plan(raw).unwrap();
        assert_eq!(plan.steps.len(), 6);
        assert_eq!(
            plan.resource,
            "digitalocean_droplet.lighthouse_lh_home_mesh_01"
        );
        assert_eq!(plan.path, "infra/tofu/zone1-do/dc-lighthouses.tf");
        assert_eq!(plan.workspace, "zone1-do");
        assert!(!plan.secrets_ready);
    }

    #[test]
    fn parse_plan_errors_on_empty_steps() {
        assert!(parse_plan(r#"{"ok":true,"steps":[]}"#).is_err());
        assert!(parse_plan("not json").is_err());
    }

    #[test]
    fn mesh_id_validation_matches_backend_rules() {
        assert!(mesh_id_valid("home-mesh"));
        assert!(mesh_id_valid("m1"));
        assert!(!mesh_id_valid(""));
        assert!(!mesh_id_valid("Home-Mesh")); // uppercase rejected
        assert!(!mesh_id_valid("home_mesh")); // underscore rejected
        assert!(!mesh_id_valid("-mesh"));
        assert!(!mesh_id_valid("mesh-"));
        assert!(!mesh_id_valid(&"a".repeat(64)));
    }

    #[test]
    fn sanitize_mesh_id_folds_and_filters() {
        assert_eq!(sanitize_mesh_id("Home Mesh!"), "homemesh");
        assert_eq!(sanitize_mesh_id("home-mesh-1"), "home-mesh-1");
        assert_eq!(sanitize_mesh_id("a_b.c"), "abc");
    }

    #[test]
    fn regions_loaded_defaults_to_first_available_region() {
        let mut p = GenesisPanel::new();
        let _ = p.update(Message::RegionsLoaded(Ok(vec![
            Region {
                slug: "ams2".into(),
                name: "Amsterdam".into(),
                available: false,
            },
            Region {
                slug: "nyc3".into(),
                name: "New York".into(),
                available: true,
            },
        ])));
        // The unavailable region is skipped; the first available one is picked.
        assert_eq!(p.region.as_deref(), Some("nyc3"));
        assert!(!p.busy);
    }

    #[test]
    fn regions_loaded_error_sets_load_error() {
        let mut p = GenesisPanel::new();
        let _ = p.update(Message::RegionsLoaded(Err("doctl down".into())));
        assert_eq!(p.load_error.as_deref(), Some("doctl down"));
    }

    #[test]
    fn plan_clicked_with_invalid_name_surfaces_validation_no_fire() {
        let mut p = GenesisPanel::new();
        p.region = Some("nyc3".into());
        p.mesh_id = "Bad_Id".into();
        let _ = p.update(Message::PlanClicked);
        assert!(p.status.to_lowercase().contains("mesh id"), "{}", p.status);
        assert!(!p.busy);
        assert_eq!(p.step, Step::Name);
    }

    #[test]
    fn plan_clicked_without_region_surfaces_validation() {
        let mut p = GenesisPanel::new();
        p.mesh_id = "home-mesh".into();
        p.region = None;
        let _ = p.update(Message::PlanClicked);
        assert!(p.status.to_lowercase().contains("region"), "{}", p.status);
        assert!(!p.busy);
    }

    #[test]
    fn plan_finished_advances_to_review_and_warns_on_missing_secret() {
        let mut p = GenesisPanel::new();
        p.busy = true;
        let _ = p.update(Message::PlanFinished(Ok(Plan {
            steps: vec!["a".into(), "b".into()],
            resource: "digitalocean_droplet.lighthouse_lh_home_mesh_01".into(),
            path: "infra/tofu/zone1-do/dc-lighthouses.tf".into(),
            workspace: "zone1-do".into(),
            secrets_ready: false,
        })));
        assert_eq!(p.step, Step::Review);
        assert!(p.plan.is_some());
        assert!(p.status.to_lowercase().contains("do-token"), "{}", p.status);
        assert!(!p.busy);
    }

    #[test]
    fn changing_the_form_invalidates_a_stale_plan_and_arm() {
        let mut p = GenesisPanel::new();
        p.plan = Some(Plan::default());
        p.armed = true;
        let _ = p.update(Message::MeshIdChanged("new-mesh".into()));
        assert!(p.plan.is_none(), "a form edit clears the stale plan");
        assert!(!p.armed, "a form edit disarms");
        // A region change does the same.
        p.plan = Some(Plan::default());
        p.armed = true;
        let _ = p.update(Message::RegionSelected("fra1".into()));
        assert!(p.plan.is_none());
        assert!(!p.armed);
    }

    #[test]
    fn confirm_write_requires_arm_first() {
        // An un-armed Confirm is a no-op (the arm→confirm gate): no op fires.
        let mut p = GenesisPanel::new();
        p.mesh_id = "home-mesh".into();
        p.region = Some("nyc3".into());
        p.armed = false;
        let _ = p.update(Message::ConfirmWriteClicked);
        assert!(!p.busy, "an un-armed confirm must not fire the write");
    }

    #[test]
    fn arm_then_confirm_fires_and_disarms() {
        let mut p = GenesisPanel::new();
        p.mesh_id = "home-mesh".into();
        p.region = Some("nyc3".into());
        let _ = p.update(Message::ArmClicked);
        assert!(p.armed);
        assert_eq!(p.step, Step::Execute);
        // Confirm fires (busy set) and disarms (so a stray second click can't refire).
        let _ = p.update(Message::ConfirmWriteClicked);
        assert!(p.busy, "an armed confirm fires the write");
        assert!(!p.armed, "the confirm consumes the arm");
    }

    #[test]
    fn write_finished_marks_written_and_surfaces_path() {
        let mut p = GenesisPanel::new();
        p.busy = true;
        let _ = p.update(Message::WriteFinished(Ok(
            "Wrote founding lighthouse to infra/tofu/zone1-do/dc-lighthouses.tf. \
             Apply (gated) to provision."
                .into(),
        )));
        assert!(p.written);
        assert!(!p.busy);
        assert!(p.status.contains("dc-lighthouses.tf"));
    }

    #[test]
    fn write_finished_err_surfaces_error_no_written() {
        let mut p = GenesisPanel::new();
        p.busy = true;
        let _ = p.update(Message::WriteFinished(Err(
            "genesis-write failed: etcd down".into(),
        )));
        assert!(!p.written);
        assert!(!p.busy);
        assert!(p.status.contains("etcd down"));
    }

    #[test]
    fn start_over_resets_the_flow_but_keeps_regions() {
        let mut p = GenesisPanel::new();
        p.regions = vec![Region {
            slug: "nyc3".into(),
            name: "NY".into(),
            available: true,
        }];
        p.step = Step::Execute;
        p.mesh_id = "home-mesh".into();
        p.written = true;
        p.backoffice_tier = Some("full".into());
        p.backoffice_plan = Some(BackofficePlan::default());
        let _ = p.update(Message::StartOverClicked);
        assert_eq!(p.step, Step::Name);
        assert!(p.mesh_id.is_empty());
        assert!(!p.written);
        assert!(
            p.backoffice_tier.is_none(),
            "start-over clears the backoffice tier"
        );
        assert!(p.backoffice_plan.is_none());
        assert_eq!(p.regions.len(), 1, "the loaded region roster is kept");
    }

    // ── DAR-45 — the backoffice wizard step ──

    #[test]
    fn review_continue_goes_to_the_backoffice_step() {
        // The Review step's Continue now lands on the Backoffice step (was Execute).
        let mut p = GenesisPanel::new();
        p.step = Step::Review;
        let _ = p.update(Message::ContinueToBackofficeClicked);
        assert_eq!(p.step, Step::Backoffice);
    }

    #[test]
    fn backoffice_off_clears_the_plan_and_fires_nothing() {
        let mut p = GenesisPanel::new();
        p.step = Step::Backoffice;
        p.backoffice_plan = Some(BackofficePlan::default());
        let _ = p.update(Message::BackofficeTierSelected(None));
        assert!(p.backoffice_tier.is_none());
        assert!(p.backoffice_plan.is_none(), "OFF clears any rendered plan");
        assert!(!p.backoffice_busy, "OFF fires no probe");
    }

    #[test]
    fn selecting_a_tier_sets_busy_and_records_the_tier() {
        let mut p = GenesisPanel::new();
        p.step = Step::Backoffice;
        let _ = p.update(Message::BackofficeTierSelected(Some("full".into())));
        assert_eq!(p.backoffice_tier.as_deref(), Some("full"));
        assert!(
            p.backoffice_busy,
            "picking a tier fires the read-only probe"
        );
        assert!(
            p.backoffice_plan.is_none(),
            "the stale plan is cleared while probing"
        );
    }

    #[test]
    fn backoffice_plan_finished_renders_units_and_warns_on_missing_secret() {
        let mut p = GenesisPanel::new();
        p.step = Step::Backoffice;
        p.backoffice_busy = true;
        let _ = p.update(Message::BackofficePlanFinished(Ok(BackofficePlan {
            tier: "minimal".into(),
            units: vec![
                BackofficeUnit {
                    id: "precheck".into(),
                    phase: 0,
                    live_gated: false,
                    via_script: "automation/state-backend/state-backend-bootstrap.sh".into(),
                },
                BackofficeUnit {
                    id: "tofu-roots".into(),
                    phase: 3,
                    live_gated: true,
                    via_script: "automation/state-backend/state-backend-bootstrap.sh".into(),
                },
            ],
            secrets_ready: false,
        })));
        assert!(!p.backoffice_busy);
        let plan = p.backoffice_plan.as_ref().expect("plan rendered");
        assert_eq!(plan.units.len(), 2);
        assert_eq!(plan.tier, "minimal");
        assert!(p.status.to_lowercase().contains("do-token"), "{}", p.status);
    }

    #[test]
    fn backoffice_plan_error_clears_the_plan_and_surfaces_it() {
        let mut p = GenesisPanel::new();
        p.step = Step::Backoffice;
        p.backoffice_busy = true;
        let _ = p.update(Message::BackofficePlanFinished(Err(
            "backoffice-plan failed: planner not found".into(),
        )));
        assert!(!p.backoffice_busy);
        assert!(p.backoffice_plan.is_none());
        assert!(p.status.contains("planner not found"), "{}", p.status);
    }

    #[test]
    fn continue_to_execute_advances_and_back_walks_steps() {
        let mut p = GenesisPanel::new();
        p.step = Step::Backoffice;
        p.backoffice_tier = Some("full".into());
        let _ = p.update(Message::ContinueToExecuteClicked);
        assert_eq!(p.step, Step::Execute);
        assert!(p.status.contains("full"), "{}", p.status);
        // Back from Execute → Backoffice → Review → Name (one step at a time).
        let _ = p.update(Message::BackClicked);
        assert_eq!(p.step, Step::Backoffice);
        let _ = p.update(Message::BackClicked);
        assert_eq!(p.step, Step::Review);
        let _ = p.update(Message::BackClicked);
        assert_eq!(p.step, Step::Name);
    }

    #[test]
    fn parse_backoffice_plan_decodes_the_reply() {
        let raw = r#"{"ok":true,"tier":"full","secrets_ready":true,"units":[
            {"id":"precheck","phase":0,"ready":false,"live_gated":false,
             "via_script":"automation/state-backend/state-backend-bootstrap.sh"},
            {"id":"build-farm","phase":6,"ready":false,"live_gated":true,
             "via_script":"install-helpers/farm-autoscale.sh"}
        ]}"#;
        let plan = parse_backoffice_plan(raw).unwrap();
        assert_eq!(plan.tier, "full");
        assert!(plan.secrets_ready);
        assert_eq!(plan.units.len(), 2);
        assert_eq!(plan.units[0].id, "precheck");
        assert!(!plan.units[0].live_gated);
        assert_eq!(plan.units[1].id, "build-farm");
        assert!(plan.units[1].live_gated);
        assert_eq!(plan.units[1].phase, 6);
    }

    #[test]
    fn parse_backoffice_plan_errors_on_empty_units() {
        assert!(parse_backoffice_plan(r#"{"ok":true,"units":[]}"#).is_err());
        assert!(parse_backoffice_plan("not json").is_err());
    }
}
