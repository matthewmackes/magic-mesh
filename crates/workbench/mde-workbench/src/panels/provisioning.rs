//! XCP-4 — the Provisioning plane: the Workbench VM Spawner.
//!
//! Opens the A-plane `MDE-VM` surface as a real iced panel. On entry it
//! queries the `xcp_provision` worker over the mde-bus:
//!   * `action/provision/list`  → renders one card per VM (start + destroy),
//!   * `action/provision/hosts` → fills the dom0 target picker (+ capacity),
//! and the spawn form fires the existing `action/provision/spawn` to clone the
//! golden template onto the chosen dom0. The three list/destroy/hosts responders
//! reply on the generic `reply/<request-ulid>` RPC lane (consumed via
//! [`crate::dbus::action_request`] / [`crate::dbus::action_request_with_body`]);
//! spawn acks on its own `action/provision/spawn-ack/<request-ulid>` topic, so
//! that one round-trip uses [`crate::dbus::action_request_reply_on`] keyed by a
//! caller-minted `request_ulid`.
//!
//! Mirrors the snapshots panel's load/update/view shape: a state struct with a
//! `Loaded` result + per-op `OperationFinished`, all Bus round-trips run on
//! `spawn_blocking` (the Bus client builds its own current-thread runtime), and
//! every surface renders through the `panel_chrome` + `mde-theme` Carbon tokens.

use std::time::Duration;

use cosmic::iced::widget::{column, pick_list, row, scrollable, text, text_input, Space};
use cosmic::iced::{Length, Padding, Task};
use cosmic::Element;
use mde_theme::{EmptyState, Icon};

use crate::components::connect_progress::{self, ConnectProgress};
use crate::controls::{styled_text_input, variant_button, ButtonVariant};
// `.colr()` (TextSty) + `.into_cosmic_color()` (IntoIcedColor) extension traits —
// the same Carbon-token color path the sibling panels thread through.
use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{
    card, empty_state, error_state, panel_container, section_block, status_badge, BadgeSeverity,
};

/// Read budget for the `list` + `hosts` probes — a couple of `xe` calls per
/// dom0 over SSH, so a touch more headroom than the connectivity panel's 2 s.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Destroy budget — a force-shutdown + `vm-uninstall` over SSH can run several
/// seconds; give it headroom over the read budget.
const DESTROY_TIMEOUT: Duration = Duration::from_secs(30);

/// Start budget — `xe vm-start` over SSH returns once the VM is booting; a few
/// seconds is ample.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// Spawn budget — the worker clones the golden, attaches the seed, starts, and
/// polls for the guest IP (up to its own 90 s window). Cover the full flow.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(120);

/// Set/rotate-credential budget — an age-encrypt + a store write (etcd or local
/// AEAD). A few seconds is ample; give the same headroom as a destroy.
const SET_CREDS_TIMEOUT: Duration = Duration::from_secs(30);

/// Action topic the `list` probe queries.
const LIST_TOPIC: &str = "action/provision/list";
/// Action topic the `hosts` probe queries.
const HOSTS_TOPIC: &str = "action/provision/hosts";
/// Action topic a destroy fires.
const DESTROY_TOPIC: &str = "action/provision/destroy";
/// Action topic a start fires (`xe vm-start` on an existing VM).
const START_TOPIC: &str = "action/provision/start";
/// Action topic a spawn fires.
const SPAWN_TOPIC: &str = "action/provision/spawn";
/// XCP-7 — action topic the set/rotate-credential button fires.
const SET_CREDS_TOPIC: &str = "action/provision/set-creds";
/// Reply-topic prefix the spawn responder acks on (suffix = the request ULID we
/// mint and put in the body, so we know where to await).
const SPAWN_ACK_PREFIX: &str = "action/provision/spawn-ack/";

/// One VM row decoded from an `action/provision/list` reply.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct VmCard {
    pub uuid: String,
    pub name: String,
    pub power_state: String,
    pub host: String,
}

impl VmCard {
    /// Whether the VM is currently running (drives the Start affordance + badge).
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.power_state == "running"
    }
}

/// One dom0 row decoded from an `action/provision/hosts` reply.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct HostCard {
    pub host: String,
    #[serde(default)]
    pub cpu_count: u32,
    #[serde(default)]
    pub mem_total_kib: u64,
    #[serde(default)]
    pub mem_free_kib: u64,
    #[serde(default)]
    pub sr_free_bytes: u64,
    #[serde(default)]
    pub running_vms: u32,
    #[serde(default)]
    pub error: Option<String>,
}

impl HostCard {
    /// Whether the host answered its capacity probe (pickable as a spawn target).
    #[must_use]
    pub fn reachable(&self) -> bool {
        self.error.is_none()
    }
}

/// The full snapshot loaded each refresh: the VM roster + the host roster, or a
/// load error (mackesd unreachable / probe failed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub vms: Vec<VmCard>,
    pub hosts: Vec<HostCard>,
}

#[derive(Debug, Clone, Default)]
pub struct ProvisioningPanel {
    pub vms: Vec<VmCard>,
    pub hosts: Vec<HostCard>,
    /// Spawn-form VM name.
    pub spawn_name: String,
    /// Spawn-form selected target dom0; `None` ⇒ the worker's first dom0.
    pub spawn_host: Option<String>,
    /// Last operation / status line.
    pub status: String,
    /// A spawn / destroy is in flight (buttons disabled while set).
    pub busy: bool,
    /// Set when the LOAD itself failed (vs a legitimately empty roster) — the
    /// view then renders the error state, never the misleading empty state.
    pub load_error: Option<String>,
    /// XCP-7 — set/rotate-credential form: the dom0 the credential is for; `None`
    /// ⇒ default to the first reachable host (mirrors `spawn_host`).
    pub cred_host: Option<String>,
    /// XCP-7 — the password typed into the credential form (masked input). Held
    /// only until the Set button fires it to the daemon, then cleared.
    pub cred_password: String,
    /// XCP-7 — the set/rotate-credential progress modal (pending → success /
    /// failure), reusing the shared `connect_progress` chrome.
    pub cred_modal: ConnectProgress,
    /// DATACENTER-21 — running ephemeral test VMs (`action/dc/testbed-list`).
    pub testbed: Vec<TestVmRow>,
    /// DATACENTER-21 — tear-down confirm arm (destroys all test VMs).
    pub testbed_armed: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Loaded, String>),
    RefreshClicked,
    SpawnNameChanged(String),
    SpawnHostSelected(String),
    SpawnClicked,
    DestroyClicked {
        name: String,
        host: String,
    },
    StartClicked {
        name: String,
        host: String,
    },
    OperationFinished(Result<String, String>),
    // XCP-7 — set/rotate a dom0's XAPI/root credential.
    CredHostSelected(String),
    CredPasswordChanged(String),
    SetCredsClicked,
    /// The store write finished — `Ok(host)` / `Err(message)`, resolving the modal.
    CredFinished(Result<String, String>),
    /// Dismiss the credential modal (Dismiss button / backdrop click).
    CredDismiss,
    /// Retry the credential write from the modal's Failure state.
    CredRetry,
    // DATACENTER-21 — ephemeral test-mesh + build-farm autoscale controls.
    TestbedLoaded(Result<Vec<TestVmRow>, String>),
    TestbedProvision,
    TestbedTeardownArm,
    TestbedTeardownConfirm,
    TestbedTeardownCancel,
    FarmScale,
    TestbedOp(Result<String, String>),
}

impl ProvisioningPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Probe the worker for the VM + host rosters on panel entry. Both Bus
    /// round-trips run on `spawn_blocking` (the client builds its own runtime).
    pub fn load() -> Task<crate::Message> {
        // The MDE-VM roster and the ephemeral test-mesh list are independent
        // probes; fetch both on open so the test-mesh section paints with live
        // counts rather than waiting for the first operator action (DATACENTER-21).
        Task::batch([
            Task::perform(
                async {
                    tokio::task::spawn_blocking(fetch)
                        .await
                        .unwrap_or_else(|_| Err("provisioning probe task panicked".into()))
                },
                |result| crate::Message::Provisioning(Message::Loaded(result)),
            ),
            Task::perform(
                async {
                    tokio::task::spawn_blocking(testbed_list_rpc)
                        .await
                        .unwrap_or_else(|_| Err("testbed-list task panicked".into()))
                },
                |res| crate::Message::Provisioning(Message::TestbedLoaded(res)),
            ),
        ])
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(loaded)) => {
                self.vms = loaded.vms;
                self.hosts = loaded.hosts;
                self.load_error = None;
                self.busy = false;
                // Default the spawn target to the first reachable dom0 if the
                // operator hasn't picked one (or the pick vanished on refresh).
                let pick_valid = self
                    .spawn_host
                    .as_deref()
                    .is_some_and(|h| self.hosts.iter().any(|host| host.host == h));
                if !pick_valid {
                    self.spawn_host = self
                        .hosts
                        .iter()
                        .find(|h| h.reachable())
                        .map(|h| h.host.clone());
                }
                // XCP-7: same defaulting for the credential-form host picker. A
                // credential can target ANY allow-listed host (even one whose
                // capacity probe failed — that's often exactly why it needs a
                // password), so the pick is validated against the full roster.
                let cred_valid = self
                    .cred_host
                    .as_deref()
                    .is_some_and(|h| self.hosts.iter().any(|host| host.host == h));
                if !cred_valid {
                    self.cred_host = self.hosts.first().map(|h| h.host.clone());
                }
                if self.status.is_empty() {
                    self.status = format!(
                        "{} VM(s) across {} dom0(s).",
                        self.vms.len(),
                        self.hosts.len()
                    );
                }
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                self.status = "Refreshing…".into();
                Self::load()
            }
            Message::SpawnNameChanged(v) => {
                self.spawn_name = sanitize_name(&v);
                Task::none()
            }
            Message::SpawnHostSelected(h) => {
                self.spawn_host = Some(h);
                Task::none()
            }
            Message::SpawnClicked => {
                if self.busy {
                    return Task::none();
                }
                let name = self.spawn_name.trim().to_string();
                if !name_valid(&name) {
                    self.status = "VM name must be non-empty (letters, digits, hyphens).".into();
                    return Task::none();
                }
                let host = self.spawn_host.clone();
                self.busy = true;
                self.status = format!("Spawning \"{name}\"…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || spawn_vm(&name, host.as_deref()))
                            .await
                            .unwrap_or_else(|_| Err("spawn task panicked".into()))
                    },
                    |res| crate::Message::Provisioning(Message::OperationFinished(res)),
                )
            }
            Message::StartClicked { name, host } => {
                if self.busy {
                    return Task::none();
                }
                // Start the EXISTING halted VM via `action/provision/start`
                // (`xe vm-start` on its uuid) — NOT spawn, which would clone a
                // new VM from the golden and leave the halted one untouched.
                self.busy = true;
                self.status = format!("Starting \"{name}\"…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || start_vm(&name, &host))
                            .await
                            .unwrap_or_else(|_| Err("start task panicked".into()))
                    },
                    |res| crate::Message::Provisioning(Message::OperationFinished(res)),
                )
            }
            Message::DestroyClicked { name, host } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Destroying \"{name}\"…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || destroy_vm(&name, &host))
                            .await
                            .unwrap_or_else(|_| Err("destroy task panicked".into()))
                    },
                    |res| crate::Message::Provisioning(Message::OperationFinished(res)),
                )
            }
            Message::OperationFinished(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(msg) => {
                        self.spawn_name.clear();
                        msg
                    }
                    Err(msg) => msg,
                };
                // Reload the rosters to reflect the new state.
                Self::load()
            }
            Message::CredHostSelected(h) => {
                self.cred_host = Some(h);
                Task::none()
            }
            Message::CredPasswordChanged(v) => {
                self.cred_password = v;
                Task::none()
            }
            Message::SetCredsClicked => self.fire_set_creds(),
            Message::CredRetry => self.fire_set_creds(),
            // DATACENTER-21 — ephemeral test-mesh + build-farm autoscale.
            Message::TestbedLoaded(Ok(vms)) => {
                self.testbed = vms;
                Task::none()
            }
            Message::TestbedLoaded(Err(e)) => {
                self.status = format!("test-mesh list: {e}");
                Task::none()
            }
            Message::TestbedProvision => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Provisioning 2 test VMs…".into();
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(|| testbed_up_rpc(2))
                            .await
                            .unwrap_or_else(|_| Err("testbed-up task panicked".into()))
                    },
                    |res| crate::Message::Provisioning(Message::TestbedOp(res)),
                )
            }
            Message::TestbedTeardownArm => {
                self.testbed_armed = true;
                Task::none()
            }
            Message::TestbedTeardownCancel => {
                self.testbed_armed = false;
                Task::none()
            }
            Message::TestbedTeardownConfirm => {
                self.testbed_armed = false;
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Tearing down test mesh…".into();
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(testbed_down_rpc)
                            .await
                            .unwrap_or_else(|_| Err("testbed-down task panicked".into()))
                    },
                    |res| crate::Message::Provisioning(Message::TestbedOp(res)),
                )
            }
            Message::FarmScale => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Running autoscale reconcile (plan only)…".into();
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(farm_scale_rpc)
                            .await
                            .unwrap_or_else(|_| Err("farm-scale task panicked".into()))
                    },
                    |res| {
                        crate::Message::Provisioning(Message::TestbedOp(res.map(|plan| {
                            let first = plan.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                            format!("autoscale reconcile done — {first}")
                        })))
                    },
                )
            }
            Message::TestbedOp(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(m) | Err(m) => m,
                };
                // refresh the test-VM list after any test-mesh op
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(testbed_list_rpc)
                            .await
                            .unwrap_or_else(|_| Err("testbed-list task panicked".into()))
                    },
                    |res| crate::Message::Provisioning(Message::TestbedLoaded(res)),
                )
            }
            Message::CredFinished(result) => {
                // Ignore a late result for a modal the operator already dismissed
                // (Dismiss → Closed before the in-flight store write landed) — a
                // resolved-then-closed modal must not pop back open titleless.
                if !self.cred_modal.is_pending() {
                    return Task::none();
                }
                self.cred_modal = match result {
                    Ok(host) => {
                        // Stored — clear the password now (and only now), so a
                        // failure path keeps it for an in-place Retry.
                        self.cred_password.clear();
                        self.cred_modal
                            .success(format!("Credential stored for {host}."))
                    }
                    Err(e) => self.cred_modal.failure(e),
                };
                Task::none()
            }
            Message::CredDismiss => {
                self.cred_modal = ConnectProgress::Closed;
                // Drop the password on dismiss too — the operator walked away, so
                // the GUI keeps no copy of an un-stored credential.
                self.cred_password.clear();
                Task::none()
            }
        }
    }

    /// XCP-7 — fire `action/provision/set-creds` for the chosen dom0 with the
    /// typed password, opening the progress modal. Shared by the Set button and
    /// the modal's Retry. The password is CLONED into the blocking task (not
    /// taken), so the field survives a failure for an in-place Retry; it is
    /// cleared only once the store write succeeds ([`Message::CredFinished`]) or
    /// the modal is dismissed, so the GUI never retains a stored/abandoned secret.
    fn fire_set_creds(&mut self) -> Task<crate::Message> {
        let Some(host) = self.cred_host.clone() else {
            self.cred_modal = ConnectProgress::pending("Set dom0 credential", "")
                .failure("Pick a dom0 host first.");
            return Task::none();
        };
        if self.cred_password.is_empty() {
            self.cred_modal = ConnectProgress::pending("Set dom0 credential", "")
                .failure("Enter the dom0 password first.");
            return Task::none();
        }
        let password = self.cred_password.clone();
        self.cred_modal = ConnectProgress::pending(
            "Set dom0 credential",
            format!("Encrypting + storing the credential for {host}…"),
        );
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || set_dom0_creds(&host, &password))
                    .await
                    .unwrap_or_else(|_| Err("set-creds task panicked".into()))
            },
            |res| crate::Message::Provisioning(Message::CredFinished(res)),
        )
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;

        // A failed probe renders as failure, never as an empty roster.
        if let Some(err) = &self.load_error {
            return panel_container(
                error_state(err.clone(), palette, || {
                    crate::Message::Provisioning(Message::RefreshClicked)
                }),
                density,
            );
        }

        let spawn_form = section_block("Spawn an MDE-VM", self.spawn_form(), palette, density);
        // XCP-7 — the set/rotate dom0-credential form.
        let cred_form = section_block(
            "dom0 credential (XAPI / root)",
            self.cred_form(),
            palette,
            density,
        );

        let roster: Element<'_, crate::Message> = if self.vms.is_empty() {
            let state = EmptyState::with_cta(
                "No MDE-VMs yet",
                "Clone the golden template onto a dom0: name the VM, pick a host, \
                 and Spawn. The new clone boots with a fresh identity (hostname, \
                 host keys, machine-id) and the operator key.",
                "Spawn",
            )
            .with_icon(Icon::Compute);
            empty_state(state, palette, || {
                crate::Message::Provisioning(Message::SpawnClicked)
            })
        } else {
            let cards = self.vms.iter().fold(column![].spacing(8), |col, vm| {
                col.push(self.vm_card(vm, palette, density))
            });
            scrollable(cards).height(Length::Fill).into()
        };

        let header = row![
            text("Provisioning").size(20).width(Length::Fill),
            variant_button(
                "Refresh",
                ButtonVariant::Ghost,
                (!self.busy).then_some(crate::Message::Provisioning(Message::RefreshClicked)),
                palette,
            ),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let testbed = section_block(
            "Test mesh & farm-scale",
            self.testbed_section(),
            palette,
            density,
        );

        let body = column![
            header,
            spawn_form,
            cred_form,
            testbed,
            roster,
            text(&self.status).size(13),
        ]
        .spacing(16)
        .width(Length::Fill);

        // XCP-7 — stack the set/rotate-credential progress modal over the panel
        // body while a store write is in flight or showing its outcome.
        connect_progress::overlay(
            &self.cred_modal,
            panel_container(body.into(), density),
            palette,
            crate::Message::Provisioning(Message::CredRetry),
            crate::Message::Provisioning(Message::CredDismiss),
        )
    }

    /// DATACENTER-21 — the ephemeral test-mesh + farm-autoscale controls: a live
    /// count of running test VMs, a Provision button, a confirm-gated Tear-down,
    /// and an Autoscale-plan (dry-run) trigger. All route through the
    /// `action/dc/testbed-*` / `action/dc/farm-scale` host_ops verbs.
    fn testbed_section(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();

        let summary = if self.testbed.is_empty() {
            "No ephemeral test VMs running.".to_string()
        } else {
            let names: Vec<String> = self
                .testbed
                .iter()
                .map(|v| {
                    if v.ip.is_empty() {
                        v.name.clone()
                    } else {
                        format!("{} ({})", v.name, v.ip)
                    }
                })
                .collect();
            format!("{} test VM(s): {}", self.testbed.len(), names.join(", "))
        };

        let provision = variant_button(
            "Provision 2 test VMs",
            ButtonVariant::Primary,
            (!self.busy).then_some(crate::Message::Provisioning(Message::TestbedProvision)),
            palette,
        );

        let scale = variant_button(
            "Autoscale plan",
            ButtonVariant::Secondary,
            (!self.busy).then_some(crate::Message::Provisioning(Message::FarmScale)),
            palette,
        );

        // Tear-down destroys every test VM, so it is arm → confirm gated.
        let teardown: Element<'_, crate::Message> = if self.testbed_armed {
            row![
                variant_button(
                    "Confirm tear-down",
                    ButtonVariant::Primary,
                    (!self.busy).then_some(crate::Message::Provisioning(
                        Message::TestbedTeardownConfirm
                    )),
                    palette,
                ),
                variant_button(
                    "Cancel",
                    ButtonVariant::Ghost,
                    Some(crate::Message::Provisioning(Message::TestbedTeardownCancel)),
                    palette,
                ),
            ]
            .spacing(8)
            .into()
        } else {
            variant_button(
                "Tear down test mesh",
                ButtonVariant::Ghost,
                (!self.busy && !self.testbed.is_empty())
                    .then_some(crate::Message::Provisioning(Message::TestbedTeardownArm)),
                palette,
            )
        };

        column![
            text(summary).size(13),
            row![provision, teardown, scale].spacing(8),
        ]
        .spacing(12)
        .into()
    }

    /// The name input + host picker + Spawn button.
    fn spawn_form(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();

        let name_input = styled_text_input(
            "VM name (e.g. web1)",
            &self.spawn_name,
            |v| crate::Message::Provisioning(Message::SpawnNameChanged(v)),
            palette,
        );

        // Only reachable dom0s are pickable spawn targets.
        let host_choices: Vec<String> = self
            .hosts
            .iter()
            .filter(|h| h.reachable())
            .map(|h| h.host.clone())
            .collect();

        let host_picker: Element<'_, crate::Message> = if host_choices.is_empty() {
            text("no reachable dom0 (set MCNF_XEN_DOM0S)")
                .size(13)
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            pick_list(host_choices, self.spawn_host.clone(), |v| {
                crate::Message::Provisioning(Message::SpawnHostSelected(v))
            })
            .into()
        };

        let can_spawn =
            !self.busy && name_valid(self.spawn_name.trim()) && self.spawn_host.is_some();
        let spawn_btn = variant_button(
            "Spawn",
            ButtonVariant::Primary,
            can_spawn.then_some(crate::Message::Provisioning(Message::SpawnClicked)),
            palette,
        );

        row![name_input, host_picker, spawn_btn,]
            .spacing(12)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
    }

    /// XCP-7 — the host picker + masked password input + Set button to store /
    /// rotate a dom0's XAPI/root credential into the mesh secret store. The
    /// credential is needed for hosts the mesh key can't reach (a freshly
    /// enrolled dom0); once stored, any authorized node drives that host without
    /// the password ever touching a process listing.
    fn cred_form(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();

        // A credential can target ANY allow-listed dom0 (even one whose capacity
        // probe failed — that's often why it needs a password), so the picker
        // offers the full roster, not just the reachable subset.
        let host_choices: Vec<String> = self.hosts.iter().map(|h| h.host.clone()).collect();

        let host_picker: Element<'_, crate::Message> = if host_choices.is_empty() {
            text("no dom0 configured (set MCNF_XEN_DOM0S)")
                .size(13)
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            pick_list(host_choices, self.cred_host.clone(), |v| {
                crate::Message::Provisioning(Message::CredHostSelected(v))
            })
            .into()
        };

        // Masked password input (mirrors the SIP-gateway secret field). The value
        // lives in panel state only until Set fires it, then it is cleared.
        let password_input = text_input("dom0 password", &self.cred_password)
            .secure(true)
            .on_input(|v| crate::Message::Provisioning(Message::CredPasswordChanged(v)))
            .padding(Padding {
                top: 0.0,
                right: 10.0,
                bottom: 0.0,
                left: 10.0,
            })
            .size(13);

        let can_set = !self.cred_password.is_empty()
            && self.cred_host.is_some()
            && !self.cred_modal.is_pending();
        let set_btn = variant_button(
            "Set / rotate",
            ButtonVariant::Secondary,
            can_set.then_some(crate::Message::Provisioning(Message::SetCredsClicked)),
            palette,
        );

        row![host_picker, password_input, set_btn,]
            .spacing(12)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
    }

    /// One VM card: name + host + a power-state badge + Start (when halted) /
    /// Destroy actions.
    fn vm_card<'a>(
        &self,
        vm: &VmCard,
        palette: mde_theme::Palette,
        density: mde_theme::Density,
    ) -> Element<'a, crate::Message> {
        let (badge_label, severity) = if vm.is_running() {
            ("running", BadgeSeverity::Success)
        } else {
            (vm.power_state.as_str(), BadgeSeverity::Neutral)
        };

        // Start only when not already running; Destroy always (disabled while
        // another op is in flight).
        let mut actions = row![].spacing(8);
        if !vm.is_running() {
            actions = actions.push(variant_button(
                "Start",
                ButtonVariant::Secondary,
                (!self.busy).then(|| {
                    crate::Message::Provisioning(Message::StartClicked {
                        name: vm.name.clone(),
                        host: vm.host.clone(),
                    })
                }),
                palette,
            ));
        }
        actions = actions.push(variant_button(
            "Destroy", // voice-allow:destroy (VM teardown is destroy, not set-removal)
            ButtonVariant::Ghost,
            (!self.busy).then(|| {
                crate::Message::Provisioning(Message::DestroyClicked {
                    name: vm.name.clone(),
                    host: vm.host.clone(),
                })
            }),
            palette,
        ));

        let body = row![
            text(vm.name.clone()).size(14).width(Length::Fixed(220.0)),
            status_badge(badge_label, severity, palette),
            text(vm.host.clone())
                .size(13)
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::Fixed(160.0)),
            Space::new().width(Length::Fill),
            actions,
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .into();

        card(body, palette, density)
    }
}

// ---- bus I/O (all blocking — call from spawn_blocking) -----------------------

/// Probe the worker for the VM + host rosters. Blocking (the Bus client builds
/// its own current-thread runtime) — call from `spawn_blocking`, never the iced
/// executor. An unreachable `list` is a load failure; an unreachable `hosts` is
/// tolerated (the roster still renders, just with no spawn targets).
fn fetch() -> Result<Loaded, String> {
    let list_json = crate::dbus::action_request(LIST_TOPIC, PROBE_TIMEOUT)
        .ok_or("mackesd not reachable over the Bus — provisioning unavailable")?;
    if let Some(e) = reply_error(&list_json) {
        return Err(format!("list failed: {e}"));
    }
    let vms = parse_vms(&list_json);
    let hosts = crate::dbus::action_request(HOSTS_TOPIC, PROBE_TIMEOUT)
        .as_deref()
        .map(parse_hosts)
        .unwrap_or_default();
    Ok(Loaded { vms, hosts })
}

/// Fire `action/provision/spawn` for `name` on `host` and await the worker's
/// ack on its custom `action/provision/spawn-ack/<request_ulid>` topic.
fn spawn_vm(name: &str, host: Option<&str>) -> Result<String, String> {
    let request_ulid = mint_request_id();
    let body = serde_json::json!({
        "request_ulid": request_ulid,
        "name": name,
        "host": host,
    });
    let ack_topic = format!("{SPAWN_ACK_PREFIX}{request_ulid}");
    let reply = crate::dbus::action_request_reply_on(
        SPAWN_TOPIC,
        Some(&body.to_string()),
        &ack_topic,
        SPAWN_TIMEOUT,
    )
    .ok_or("mackesd not reachable over the Bus (spawn)")?;
    if let Some(e) = reply_error(&reply) {
        return Err(format!("spawn failed: {e}"));
    }
    // A non-JSON ack means the spawn outcome is unknown — surface it, don't
    // fabricate a success.
    let v: serde_json::Value =
        serde_json::from_str(&reply).map_err(|e| format!("spawn ack not decodable: {e}"))?;
    let hostname = v["hostname"].as_str().unwrap_or(name);
    match v["ip"].as_str() {
        Some(ip) => Ok(format!("Spawned {hostname} ({ip}).")),
        None => Ok(format!("Spawned {hostname} (no IP yet).")),
    }
}

/// Fire a name-on-a-dom0 verb (`destroy` / `start`) on the generic RPC lane and
/// map the reply to a status line. Shared by [`destroy_vm`] + [`start_vm`].
fn named_vm_action(
    topic: &str,
    name: &str,
    host: &str,
    timeout: Duration,
    done: &str,
) -> Result<String, String> {
    let body = serde_json::json!({ "name": name, "host": host });
    let reply = crate::dbus::action_request_with_body(topic, Some(&body.to_string()), timeout)
        .ok_or_else(|| format!("mackesd not reachable over the Bus ({done})"))?;
    if let Some(e) = reply_error(&reply) {
        return Err(format!("{done} failed: {e}"));
    }
    Ok(format!("{} {name}.", capitalize(done)))
}

/// XCP-7 — fire `action/provision/set-creds` to seal `password` for dom0 `host`
/// into the mesh secret store. Blocking (the Bus client builds its own runtime) —
/// call from `spawn_blocking`. Returns the host on success.
///
/// The password rides only in the request body (the bus is the local replicated
/// store, not a process argv); on success the reply carries the host, never the
/// secret, so nothing here logs or echoes the credential.
fn set_dom0_creds(host: &str, password: &str) -> Result<String, String> {
    let body = serde_json::json!({ "host": host, "password": password });
    let reply = crate::dbus::action_request_with_body(
        SET_CREDS_TOPIC,
        Some(&body.to_string()),
        SET_CREDS_TIMEOUT,
    )
    .ok_or("mackesd not reachable over the Bus (set-creds)")?;
    if let Some(e) = reply_error(&reply) {
        return Err(format!("set-creds failed: {e}"));
    }
    // The reply only echoes the host we already sent (the daemon stores no new
    // info we need back), so — like the destroy/start verbs (`named_vm_action`) —
    // we return the requested host without re-parsing the success payload.
    Ok(host.to_string())
}

/// Fire `action/provision/destroy` for `name` on `host`.
fn destroy_vm(name: &str, host: &str) -> Result<String, String> {
    named_vm_action(DESTROY_TOPIC, name, host, DESTROY_TIMEOUT, "destroyed")
}

/// Fire `action/provision/start` for `name` on `host` (`xe vm-start` on the
/// existing VM — distinct from spawn, which clones a new one).
fn start_vm(name: &str, host: &str) -> Result<String, String> {
    named_vm_action(START_TOPIC, name, host, START_TIMEOUT, "started")
}

/// DATACENTER-21 — one ephemeral test VM from `action/dc/testbed-list`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TestVmRow {
    pub name: String,
    #[serde(default)]
    pub ip: String,
}

/// Fire an `action/dc/<verb>` RPC (the DATACENTER-21 host_ops verbs) with `body`,
/// returning the parsed reply (or its `error`). Mirrors the panel's Bus round-trip.
fn dc_action(verb: &str, body: &str, timeout: Duration) -> Result<serde_json::Value, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let topic = format!("action/dc/{verb}");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let p = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &p,
            &topic,
            mde_bus::hooks::config::Priority::Default,
            Some(verb),
            Some(body),
            timeout,
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad {verb} reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(|x| x.as_str()) {
        return Err(err.to_string());
    }
    Ok(v)
}

/// DATACENTER-21 — list running ephemeral test VMs.
fn testbed_list_rpc() -> Result<Vec<TestVmRow>, String> {
    let v = dc_action("testbed-list", "{}", Duration::from_secs(20))?;
    Ok(v.get("vms")
        .and_then(|a| serde_json::from_value::<Vec<TestVmRow>>(a.clone()).ok())
        .unwrap_or_default())
}

/// DATACENTER-21 — provision `n` test VMs (async clone; returns immediately).
fn testbed_up_rpc(n: u32) -> Result<String, String> {
    dc_action(
        "testbed-up",
        &format!("{{\"n\":{n}}}"),
        Duration::from_secs(20),
    )?;
    Ok(format!("provisioning {n} test VM(s) — refresh shortly"))
}

/// DATACENTER-21 — tear down ALL test VMs (confirm-gated on the handler).
fn testbed_down_rpc() -> Result<String, String> {
    dc_action(
        "testbed-down",
        "{\"confirm\":true}",
        Duration::from_secs(120),
    )?;
    Ok("test mesh torn down".to_string())
}

/// DATACENTER-21 — run the autoscale reconcile (plan only); return the plan text.
fn farm_scale_rpc() -> Result<String, String> {
    let v = dc_action("farm-scale", "{}", Duration::from_secs(120))?;
    Ok(v.get("plan")
        .and_then(|x| x.as_str())
        .unwrap_or("(no plan output)")
        .to_string())
}

// ---- pure helpers (parse / validate) -----------------------------------------

/// Decode the `{"vms":[…]}` list reply.
#[must_use]
fn parse_vms(raw: &str) -> Vec<VmCard> {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    v.get("vms")
        .and_then(|a| serde_json::from_value::<Vec<VmCard>>(a.clone()).ok())
        .unwrap_or_default()
}

/// Decode the `{"hosts":[…]}` hosts reply.
#[must_use]
fn parse_hosts(raw: &str) -> Vec<HostCard> {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    v.get("hosts")
        .and_then(|a| serde_json::from_value::<Vec<HostCard>>(a.clone()).ok())
        .unwrap_or_default()
}

/// Pull a `{"error":…}` message out of a mackesd reply envelope, if present —
/// the shared decoder over [`crate::dbus::reply_error`].
fn reply_error(raw: &str) -> Option<String> {
    crate::dbus::reply_error(raw)
}

/// Title-case the first ASCII letter of a lowercase verb for a status line
/// (`"started"` → `"Started"`).
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// Sanitize a VM name as typed: ASCII alphanumeric + hyphen only (the
/// `MDE-VM-<name>` convention the worker enforces accepts only these).
#[must_use]
fn sanitize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// Validate a VM name: non-empty, ASCII alphanumeric + hyphens only.
#[must_use]
fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A unique-ish request id (UNIX-nanos hex) correlating a spawn to its ack
/// topic — no `ulid` dep, monotone enough for a per-request key.
fn mint_request_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vms_decodes_the_list_reply() {
        let raw = r#"{"vms":[
            {"uuid":"u-1","name":"MDE-VM-web1","power_state":"running","host":"172.20.0.4"},
            {"uuid":"u-2","name":"MDE-VM-db","power_state":"halted","host":"172.20.0.5"}
        ]}"#;
        let vms = parse_vms(raw);
        assert_eq!(vms.len(), 2);
        assert_eq!(vms[0].name, "MDE-VM-web1");
        assert!(vms[0].is_running());
        assert_eq!(vms[1].host, "172.20.0.5");
        assert!(!vms[1].is_running());
    }

    #[test]
    fn parse_vms_tolerates_garbage_and_missing_key() {
        assert!(parse_vms("not json").is_empty());
        assert!(parse_vms("{}").is_empty());
        assert!(parse_vms(r#"{"vms":[]}"#).is_empty());
    }

    #[test]
    fn parse_hosts_splits_reachable_from_failed() {
        let raw = r#"{"hosts":[
            {"host":"172.20.0.4","cpu_count":8,"mem_total_kib":1024,"mem_free_kib":512,"sr_free_bytes":9000,"running_vms":3},
            {"host":"172.20.0.5","error":"unreachable"}
        ]}"#;
        let hosts = parse_hosts(raw);
        assert_eq!(hosts.len(), 2);
        assert!(hosts[0].reachable());
        assert_eq!(hosts[0].cpu_count, 8);
        assert!(!hosts[1].reachable());
        assert_eq!(hosts[1].error.as_deref(), Some("unreachable"));
    }

    #[test]
    fn reply_error_extracts_only_when_present() {
        assert_eq!(reply_error(r#"{"error":"boom"}"#).as_deref(), Some("boom"));
        assert!(reply_error(r#"{"destroyed":"x"}"#).is_none());
        assert!(reply_error(r#"{"started":"x"}"#).is_none());
        assert!(reply_error("not json").is_none());
    }

    #[test]
    fn capitalize_titlecases_verb() {
        assert_eq!(capitalize("started"), "Started");
        assert_eq!(capitalize("destroyed"), "Destroyed");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn start_clicked_while_busy_is_a_noop() {
        let mut p = ProvisioningPanel::new();
        p.busy = true;
        p.status = "Starting…".into();
        let _ = p.update(Message::StartClicked {
            name: "MDE-VM-web1".into(),
            host: "172.20.0.4".into(),
        });
        // A busy panel ignores the click (no new op kicked off).
        assert_eq!(p.status, "Starting…");
    }

    #[test]
    fn name_validation_and_sanitize() {
        assert!(name_valid("web-01"));
        assert!(!name_valid(""));
        assert!(!name_valid("bad name"));
        assert!(!name_valid("under_score"));
        assert_eq!(sanitize_name("my vm!_01"), "myvm01");
        assert_eq!(sanitize_name("web-1"), "web-1");
    }

    #[test]
    fn spawn_name_change_sanitizes_input() {
        let mut p = ProvisioningPanel::new();
        let _ = p.update(Message::SpawnNameChanged("my vm!_01".into()));
        assert_eq!(p.spawn_name, "myvm01");
    }

    #[test]
    fn spawn_clicked_with_invalid_name_surfaces_validation() {
        let mut p = ProvisioningPanel::new();
        p.spawn_host = Some("172.20.0.4".into());
        p.spawn_name = "  ".into();
        let _ = p.update(Message::SpawnClicked);
        assert!(p.status.contains("name"), "{}", p.status);
        assert!(!p.busy);
    }

    #[test]
    fn spawn_clicked_while_busy_is_a_noop() {
        let mut p = ProvisioningPanel::new();
        p.busy = true;
        p.status = "Spawning…".into();
        let _ = p.update(Message::SpawnClicked);
        assert_eq!(p.status, "Spawning…");
    }

    #[test]
    fn loaded_defaults_spawn_host_to_first_reachable() {
        let mut p = ProvisioningPanel::new();
        let _ = p.update(Message::Loaded(Ok(Loaded {
            vms: vec![],
            hosts: vec![
                HostCard {
                    host: "172.20.0.4".into(),
                    error: Some("unreachable".into()),
                    ..HostCard::default()
                },
                HostCard {
                    host: "172.20.0.5".into(),
                    cpu_count: 4,
                    ..HostCard::default()
                },
            ],
        })));
        // The unreachable host is skipped; the first reachable one is picked.
        assert_eq!(p.spawn_host.as_deref(), Some("172.20.0.5"));
        assert!(!p.busy);
    }

    #[test]
    fn loaded_preserves_a_still_valid_operator_pick() {
        let mut p = ProvisioningPanel::new();
        p.spawn_host = Some("172.20.0.5".into());
        let _ = p.update(Message::Loaded(Ok(Loaded {
            vms: vec![],
            hosts: vec![
                HostCard {
                    host: "172.20.0.4".into(),
                    ..HostCard::default()
                },
                HostCard {
                    host: "172.20.0.5".into(),
                    ..HostCard::default()
                },
            ],
        })));
        assert_eq!(p.spawn_host.as_deref(), Some("172.20.0.5"));
    }

    #[test]
    fn loaded_error_sets_load_error_not_empty_roster() {
        let mut p = ProvisioningPanel::new();
        let _ = p.update(Message::Loaded(Err("mackesd down".into())));
        assert_eq!(p.load_error.as_deref(), Some("mackesd down"));
        assert!(!p.busy);
    }

    #[test]
    fn operation_finished_ok_clears_name_and_reloads() {
        let mut p = ProvisioningPanel::new();
        p.busy = true;
        p.spawn_name = "web1".into();
        let _ = p.update(Message::OperationFinished(
            Ok("Spawned MDE-VM-web1.".into()),
        ));
        assert!(!p.busy);
        assert_eq!(p.status, "Spawned MDE-VM-web1.");
        assert!(p.spawn_name.is_empty());
    }

    #[test]
    fn operation_finished_err_keeps_name_and_surfaces_error() {
        let mut p = ProvisioningPanel::new();
        p.busy = true;
        p.spawn_name = "web1".into();
        let _ = p.update(Message::OperationFinished(Err("clone failed".into())));
        assert!(!p.busy);
        assert_eq!(p.status, "clone failed");
        assert_eq!(p.spawn_name, "web1");
    }

    #[test]
    fn mint_request_id_is_hex_and_nonempty() {
        let id = mint_request_id();
        assert!(!id.is_empty());
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── XCP-7 set/rotate dom0 credential ──

    #[test]
    fn loaded_defaults_cred_host_to_first_host_even_if_unreachable() {
        let mut p = ProvisioningPanel::new();
        let _ = p.update(Message::Loaded(Ok(Loaded {
            vms: vec![],
            hosts: vec![HostCard {
                host: "172.20.0.4".into(),
                // An UNREACHABLE host is still a valid credential target — that's
                // typically exactly why it needs a password set.
                error: Some("unreachable".into()),
                ..HostCard::default()
            }],
        })));
        assert_eq!(p.cred_host.as_deref(), Some("172.20.0.4"));
    }

    #[test]
    fn set_creds_without_password_opens_modal_failure_no_fire() {
        let mut p = ProvisioningPanel::new();
        p.cred_host = Some("172.20.0.4".into());
        // No password typed → an immediate, honest modal failure (no bus fire).
        let _ = p.update(Message::SetCredsClicked);
        assert!(matches!(p.cred_modal, ConnectProgress::Failure { .. }));
    }

    #[test]
    fn set_creds_without_host_opens_modal_failure() {
        let mut p = ProvisioningPanel::new();
        p.cred_host = None;
        p.cred_password = "pw".into();
        let _ = p.update(Message::SetCredsClicked);
        assert!(matches!(p.cred_modal, ConnectProgress::Failure { .. }));
    }

    #[test]
    fn set_creds_keeps_password_on_fire_for_retry_clears_on_success() {
        // With a host + password the modal goes Pending; the password is RETAINED
        // (cloned, not taken) so a Failure → Retry can re-fire it in place.
        let mut p = ProvisioningPanel::new();
        p.cred_host = Some("172.20.0.4".into());
        p.cred_password = "dom0-secret".into();
        let _ = p.update(Message::SetCredsClicked);
        assert!(p.cred_modal.is_pending(), "modal should be pending");
        assert_eq!(
            p.cred_password, "dom0-secret",
            "the password must survive the fire so Retry works"
        );
        // A success clears the field (the daemon now holds the sealed copy).
        let _ = p.update(Message::CredFinished(Ok("172.20.0.4".into())));
        assert!(matches!(p.cred_modal, ConnectProgress::Success { .. }));
        assert!(
            p.cred_password.is_empty(),
            "the password is cleared once it's been stored"
        );
    }

    #[test]
    fn cred_retry_after_failure_re_fires_the_still_present_password() {
        // The failure path keeps the password so Retry works in place (the modal
        // is back to Pending after Retry, NOT a 'enter the password first' bounce).
        let mut p = ProvisioningPanel::new();
        p.cred_host = Some("172.20.0.4".into());
        p.cred_password = "dom0-secret".into();
        let _ = p.update(Message::SetCredsClicked);
        let _ = p.update(Message::CredFinished(Err(
            "set-creds failed: etcd down".into()
        )));
        assert!(matches!(p.cred_modal, ConnectProgress::Failure { .. }));
        assert_eq!(p.cred_password, "dom0-secret", "password kept for retry");
        // Retry re-fires (modal back to Pending), not a validation bounce.
        let _ = p.update(Message::CredRetry);
        assert!(
            p.cred_modal.is_pending(),
            "retry should re-fire, not bounce"
        );
    }

    #[test]
    fn cred_finished_for_a_dismissed_modal_is_ignored() {
        // A late result for a modal the operator already dismissed must NOT pop a
        // (titleless) dialog back open.
        let mut p = ProvisioningPanel::new();
        p.cred_modal = ConnectProgress::Closed;
        let _ = p.update(Message::CredFinished(Ok("172.20.0.4".into())));
        assert!(!p.cred_modal.is_open(), "a closed modal stays closed");
        let _ = p.update(Message::CredFinished(Err("etcd down".into())));
        assert!(!p.cred_modal.is_open());
    }

    #[test]
    fn cred_finished_resolves_the_modal_success_and_failure() {
        let mut p = ProvisioningPanel::new();
        p.cred_modal = ConnectProgress::pending("Set dom0 credential", "Storing…");
        let _ = p.update(Message::CredFinished(Ok("172.20.0.4".into())));
        match &p.cred_modal {
            ConnectProgress::Success { message, .. } => {
                assert!(message.contains("172.20.0.4"));
            }
            other => panic!("expected Success, got {other:?}"),
        }
        // A failure resolves to the Failure state surfacing the error.
        p.cred_modal = ConnectProgress::pending("Set dom0 credential", "Storing…");
        let _ = p.update(Message::CredFinished(Err(
            "set-creds failed: etcd down".into()
        )));
        match &p.cred_modal {
            ConnectProgress::Failure { error, .. } => assert!(error.contains("etcd down")),
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn cred_dismiss_closes_the_modal_and_drops_the_password() {
        let mut p = ProvisioningPanel::new();
        p.cred_password = "unsaved-secret".into();
        p.cred_modal =
            ConnectProgress::pending("Set dom0 credential", "Storing…").failure("etcd down");
        let _ = p.update(Message::CredDismiss);
        assert!(!p.cred_modal.is_open());
        assert!(
            p.cred_password.is_empty(),
            "dismiss drops the un-stored credential"
        );
    }
}
