//! Construct's Browser surface.
//!
//! The host browser runtime was extracted from this crate. Construct keeps the
//! surface and a small typed controller so activation has an explicit guest
//! destination while the VDI attachment is supplied by the platform session
//! layer. Chromium, browser chrome, page execution, and guest failures remain
//! inside `browser-vm`.

use mde_egui::egui::{self, RichText};
use mde_egui::search_omnibox::SearchItem;

#[cfg(test)]
use mde_files_egui::transfers::TransfersClient;

const VM_WORKLOAD: &str = "browser-vm";

/// Shell media-key vocabulary retained at the VM boundary. The guest owns
/// playback; these actions are intentionally not translated into host controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaTransportAction {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserVmTransport {
    SunshineMoonlight,
    Rdp,
}

impl BrowserVmTransport {
    const fn label(self) -> &'static str {
        match self {
            Self::SunshineMoonlight => "Sunshine/Moonlight",
            Self::Rdp => "RDP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BrowserVmRoute {
    workload: &'static str,
    preferred: BrowserVmTransport,
    alternate: BrowserVmTransport,
    resume: bool,
}

impl BrowserVmRoute {
    const fn select_resume() -> Self {
        Self {
            workload: VM_WORKLOAD,
            preferred: BrowserVmTransport::SunshineMoonlight,
            alternate: BrowserVmTransport::Rdp,
            resume: true,
        }
    }

}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserVmConnectionState {
    WaitingForWorkload,
    WaitingForVdi,
    Unavailable,
}

impl BrowserVmConnectionState {
    const fn label(self) -> &'static str {
        match self {
            Self::WaitingForWorkload => "Waiting for Workloads",
            Self::WaitingForVdi => "Waiting for VDI",
            Self::Unavailable => "Browser VM unavailable",
        }
    }
}

/// The only Browser activation input accepted from the Workloads mirror. It is
/// deliberately an identity/status tuple: no command, URL, endpoint, or host
/// engine data can cross the Browser boundary here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserVmTarget {
    pub(crate) serving_peer: String,
    pub(crate) workload: String,
    pub(crate) status: String,
    pub(crate) reachable: bool,
}

/// A one-shot handoff from the Browser surface to the existing VDI renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserVmConnect {
    pub(crate) target: BrowserVmTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserVmDiagnostic {
    state: BrowserVmConnectionState,
    detail: String,
}

/// The shell-side state retained after the host Browser runtime extraction.
/// This deliberately contains no page, tab, engine, process, or pixel state.
pub(crate) struct WebState {
    route: BrowserVmRoute,
    diagnostic: BrowserVmDiagnostic,
    requested_target: Option<String>,
    browser_vm_connect: Option<BrowserVmConnect>,
    browser_vm_request_issued: bool,
    #[cfg(test)]
    _transfers: Option<Box<dyn TransfersClient>>,
}

impl Default for WebState {
    fn default() -> Self {
        let route = BrowserVmRoute::select_resume();
        Self {
            route,
            diagnostic: BrowserVmDiagnostic {
                state: BrowserVmConnectionState::WaitingForVdi,
                detail: "The guest Browser VM is selected; attach its VDI transport to continue."
                    .to_owned(),
            },
            requested_target: None,
            browser_vm_connect: None,
            browser_vm_request_issued: false,
            #[cfg(test)]
            _transfers: None,
        }
    }
}

impl WebState {
    /// Keep the shell's existing test seam without reinstating a Browser-owned
    /// transfer ledger. Transfers remain owned by Files/Transfers.
    #[cfg(test)]
    pub(crate) fn with_transfers(mut self, transfers: Box<dyn TransfersClient>) -> Self {
        self._transfers = Some(transfers);
        self
    }

    /// Browser no longer owns host download jobs; the shared shell operation
    /// summary therefore has no Browser-local contribution.
    pub(crate) fn operation_progress_summary(
        &self,
    ) -> Option<mde_files_egui::model::OperationProgressSummary> {
        None
    }

    pub(crate) fn pump_downloads_for_shell_chrome(&mut self) {}

    #[cfg(test)]
    pub(crate) fn mark_downloads_poll_due_for_test(&mut self) {}

    pub(crate) fn open_search_omnibox_target(&mut self, target: &str) {
        let target = target.trim();
        self.requested_target = (!target.is_empty()).then(|| target.chars().take(512).collect());
    }

    /// Fold the typed Workloads projection into Browser activation. A VM that
    /// is not yet reachable remains unavailable; this path never starts a host
    /// helper as a fallback.
    pub(crate) fn sync_browser_vm_target(&mut self, target: Option<BrowserVmTarget>) {
        if self.browser_vm_request_issued || self.browser_vm_connect.is_some() {
            return;
        }
        let Some(target) = target else {
            self.diagnostic = BrowserVmDiagnostic {
                state: BrowserVmConnectionState::WaitingForWorkload,
                detail: "No admitted browser-vm workload is present in the Workloads mirror."
                    .to_owned(),
            };
            return;
        };
        if !target.reachable || target.status != "running" {
            self.diagnostic = BrowserVmDiagnostic {
                state: BrowserVmConnectionState::WaitingForWorkload,
                detail: format!(
                    "Workload `{}` on `{}` is {} and not reachable; waiting for guest readiness.",
                    target.workload, target.serving_peer, target.status
                ),
            };
            return;
        }
        self.diagnostic = BrowserVmDiagnostic {
            state: BrowserVmConnectionState::WaitingForVdi,
            detail: "The Browser VM is ready; requesting its brokered VDI session.".to_owned(),
        };
        self.browser_vm_connect = Some(BrowserVmConnect { target });
    }

    /// Consume the one-shot handoff. Once consumed, Browser remains bound to
    /// this guest session until the user leaves the surface or the VDI layer
    /// reports a bounded failure.
    pub(crate) fn take_browser_vm_connect(&mut self) -> Option<BrowserVmConnect> {
        let request = self.browser_vm_connect.take()?;
        self.browser_vm_request_issued = true;
        Some(request)
    }

    /// Surface an explicit VDI/broker failure without inventing a page or a
    /// host-rendered fallback.
    pub(crate) fn browser_vm_unavailable(&mut self, detail: impl Into<String>) {
        self.diagnostic = BrowserVmDiagnostic {
            state: BrowserVmConnectionState::Unavailable,
            detail: detail.into(),
        };
    }

    /// Browser search suggestions are guest-owned after the host runtime
    /// extraction; the shell contributes no page/history candidates.
    pub(crate) fn search_omnibox_items(&self, _query: &str) -> Vec<SearchItem<String>> {
        Vec::new()
    }

    /// Media hotkeys remain accepted by the shell dispatcher but are forwarded
    /// only after a future typed VDI input lane exists.
    pub(crate) fn selected_media_transport(&mut self, _action: MediaTransportAction) {}

    /// Surface occlusion has no host page/pixel worker to pause. Keep this
    /// compatibility seam so callers do not invent a second Browser lifecycle.
    pub(crate) fn note_surface_foreground(&mut self, _foreground: bool) {}

    pub(crate) fn take_bookmarks_manager_request(&mut self) -> bool {
        false
    }

    fn diagnostic(&self) -> &BrowserVmDiagnostic {
        &self.diagnostic
    }
}

/// Render the Construct-owned VM boundary. The guest supplies the Browser UI
/// after a real VDI attachment; this placeholder never pretends to be a page.
pub(crate) fn web_panel(ui: &mut egui::Ui, state: &mut WebState) {
    let route = state.route;
    ui.vertical_centered(|ui| {
        ui.heading("Browser VM");
        ui.add_space(8.0);
        ui.label(
            RichText::new("Guest-owned Chromium is available through the dedicated VM.")
                .strong(),
        );
        ui.add_space(4.0);
        ui.label(format!("Workload: {}", route.workload));
        ui.label(format!(
            "Preferred transport: {}; alternate: {}",
            route.preferred.label(),
            route.alternate.label()
        ));
        ui.add_space(8.0);
        ui.colored_label(
            mde_egui::Style::TEXT_DIM,
            format!("{}: {}", state.diagnostic().state.label(), state.diagnostic().detail),
        );
        if let Some(target) = state.requested_target.as_deref() {
            ui.add_space(4.0);
            ui.label(format!("Requested after VM attachment: {target}"));
        }
        ui.add_space(8.0);
        ui.label("No host page engine or host-rendered Browser UI is available.");
    });
}

/// Retained as a stable shell construction seam; Browser media control is now
/// guest-owned and has no host process to expose through MPRIS.
#[derive(Debug, Default)]
pub(crate) struct BrowserMprisHandle;

pub(crate) fn spawn_browser_mpris() -> BrowserMprisHandle {
    BrowserMprisHandle
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::Context;

    #[test]
    fn default_browser_state_selects_only_the_typed_guest_route() {
        let state = WebState::default();
        assert_eq!(state.route, BrowserVmRoute::select_resume());
        assert_eq!(state.route.workload, VM_WORKLOAD);
        assert!(state.route.resume);
        assert_eq!(
            state.diagnostic().state,
            BrowserVmConnectionState::WaitingForVdi
        );
    }

    #[test]
    fn browser_panel_paints_guest_boundary_without_host_runtime() {
        let ctx = Context::default();
        let mut state = WebState::default();
        let out = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| web_panel(ui, &mut state));
        });
        assert!(!out.shapes.is_empty());
        assert_eq!(state.requested_target, None);
    }

    #[test]
    fn front_door_target_is_retained_until_guest_attachment() {
        let mut state = WebState::default();
        state.open_search_omnibox_target("  https://example.test/  ");
        assert_eq!(state.requested_target.as_deref(), Some("https://example.test/"));
    }

    #[test]
    fn browser_vm_waits_for_workloads_before_requesting_vdi() {
        let mut state = WebState::default();
        state.sync_browser_vm_target(None);
        assert!(state.browser_vm_connect.is_none());
        assert_eq!(
            state.diagnostic().state,
            BrowserVmConnectionState::WaitingForWorkload
        );
    }

    #[test]
    fn reachable_running_browser_vm_crosses_only_the_typed_vdi_seam() {
        let mut state = WebState::default();
        state.sync_browser_vm_target(Some(BrowserVmTarget {
            serving_peer: "eagle".to_owned(),
            workload: "browser-vm".to_owned(),
            status: "running".to_owned(),
            reachable: true,
        }));
        let request = state.take_browser_vm_connect().expect("VDI handoff");
        assert_eq!(request.target.workload, "browser-vm");
        assert!(state.browser_vm_connect.is_none());
    }
}
