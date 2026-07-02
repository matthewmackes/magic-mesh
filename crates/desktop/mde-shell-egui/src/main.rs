//! `mde-shell-egui` — the single MCNF E12 "Quasar" egui shell (E12-3).
//!
//! One eframe app on the `mde-egui` harness. A thin persistent **chrome bar**
//! (peers · sessions · status + an Expand toggle) sits over a central view that
//! is either:
//!
//! * the **session EmptyState** (collapsed) — a real session is a fullscreen VM
//!   texture from `mde-vdi`, a later unit; or
//! * the **Workbench** five-plane nav (expanded) — This Node / Controller /
//!   Network / Fleet / Provisioning.
//!
//! The expand/collapse transition eases through the shared `Motion` table and the
//! whole surface renders through the shared `Style` (governance §4/§5/§7). This is
//! the skeleton the panels (Workbench/Files/Music/Voice) and the VM session-view
//! plug into.

mod chrome;
mod clipboard;
mod controller;
mod datacenter;
mod discovery;
mod dock;
mod instances;
mod network;
mod notifications;
mod provisioning;
mod services_flow;
mod session;
mod system;
mod thisnode;
mod toast_bridge;
mod vdi;
mod workbench;

use mde_egui::eframe::CreationContext;
use mde_egui::{eframe, egui, run_client, Motion, Style};

use mde_seat::{Probe, SeatSnapshot};

use mde_files_egui::{files_panel, FileBrowser};
use mde_music_egui::{music_header, music_panel, music_pump, MusicApp};
use mde_voice_egui::{voice_header, voice_panel, voice_pump, VoiceApp};

use dock::Surface;
use workbench::Plane;

/// The shell's pure navigation state: whether the chrome bar is expanded into the
/// shell body, and — once expanded — which plane the Workbench has selected. Kept
/// separate from the surface apps (which need an eframe `CreationContext` to
/// build) so the nav invariants stay unit-testable without a GPU.
#[derive(Default)]
struct Nav {
    /// `true` while the chrome bar is expanded into the shell body.
    expanded: bool,
    /// Which surface fills the shell body (Workbench by default).
    surface: Surface,
    /// The Workbench plane shown when the Workbench surface is active.
    plane: Plane,
}

impl Nav {
    /// Flip between the collapsed session view and the expanded shell body.
    fn toggle_expand(&mut self) {
        self.expanded = !self.expanded;
    }
}

/// The whole shell: the nav state, the live chrome/Fleet Bus state, and the three
/// embedded mesh-control surfaces it owns and drives per frame (E12-3b EMBED).
struct Shell {
    /// Expand state + the selected Workbench plane.
    nav: Nav,
    /// Fleet plane — live per-node KVM host health + VM roster, and the
    /// host-targeted VM lifecycle controls (MV-6). Subscribes to the Bus.
    datacenter: datacenter::DatacenterState,
    /// This Node plane — this host's live status (role, overlay IP, presence +
    /// heartbeat freshness, daemon health, peer/leader context), folded from the
    /// world-readable mesh-status snapshot (WB-ThisNode). Reads no `mackesd` IPC.
    thisnode: thisnode::ThisNodeState,
    /// Network plane — the mesh network fabric's live status (overlay IP + cipher,
    /// the elected leader, the peer directory as network links, network-scoped
    /// service health, overlay routing), folded from the same world-readable
    /// mesh-status snapshot (WB-Network). Reads no `mackesd` IPC.
    network: network::NetworkState,
    /// Controller plane — the mesh control plane's live status (the elected
    /// controller + its leader lease, and the fleet-wide control-service health
    /// rollup: which nodes run the mesh daemon / Syncthing / Bus), folded from the
    /// same world-readable mesh-status snapshot (WB-Controller). Reads no `mackesd`
    /// IPC.
    controller: controller::ControllerState,
    /// Provisioning plane — the mesh's live onboarding / deployment posture
    /// (per-node deployment tier + role rollup, the fleet version target vs each
    /// node's build + update flag, and per-node enrollment readiness), folded from
    /// the same world-readable mesh-status snapshot (WB-Provisioning). Reads no
    /// `mackesd` IPC.
    provisioning: provisioning::ProvisioningState,
    /// The Services flow (OW-11) — the Provisioning plane's day-2 service adds:
    /// pick Music/Files/Voice, preview the daemon's plan (dry-run), apply over
    /// the Bus, and render the `service_onboard` worker's typed answer.
    services: services_flow::ServicesFlowState,
    /// The always-visible chrome bar's live state — peers + mesh status folded
    /// from the world-readable mesh-status snapshot, polled on the shared cadence
    /// (self-gating inside `chrome::show`).
    chrome: chrome::ChromeState,
    /// The Music surface, owned + built once (its worker thread wakes the shell's
    /// egui context on every update). Rendered via `mde_music_egui::music_panel`.
    music: MusicApp,
    /// The Files surface model, owned + built once over the production backend.
    /// Rendered via `mde_files_egui::files_panel`.
    files: FileBrowser,
    /// The Voice surface, owned + built once (its SIP agent wakes the shell's egui
    /// context on every update). Rendered via `mde_voice_egui::voice_panel`.
    voice: VoiceApp,
    /// The VDI Desktop surface — a brokered VM desktop decoded by `mde-vdi-rdp` /
    /// `mde-vdi-vnc` and uploaded to an egui texture. Holds no live session until
    /// the gated wire transport (E12-4) attaches one; the panel shows its honest
    /// "no desktop" EmptyState until then.
    vdi: vdi::VdiState,
    /// The remote-desktop picker (E12-5b) — the Desktop surface's no-session face:
    /// lists the mesh's advertised VMs (reusing the Fleet inventory) and, on
    /// Connect, emits the broker `Open` request + hands the target to `vdi`.
    discovery: discovery::DiscoveryState,
    /// The Instances surface — this workstation's local cloud-hypervisor VMs via
    /// the `mde-kvm` broker (E12-7). Create / boot / shutdown drive mde-kvm's real
    /// lifecycle; with no live VMM the ops surface mde-kvm's typed gated error, and
    /// an empty roster shows the honest "No local VMs" EmptyState.
    instances: instances::InstancesState,
    /// The Notifications surface — tails the Bus alert lanes (security, presence,
    /// firewall, compute, FDO) and accumulates mesh-wide alerts newest-first.
    notifications: notifications::NotificationsState,
    /// The Clipboard surface — tails `event/clipboard/clip` and shows recent mesh
    /// clipboard entries captured by the clipboard_sync worker, newest first.
    clipboard: clipboard::ClipboardState,
    /// The System surface — this seat's host controls, folded from the ONE
    /// `mde-seat` `Seat` (lock 1): mixer / Bluetooth / displays / power & battery /
    /// backlight / hotkeys. Its cached snapshot also feeds the three read-only
    /// chrome status icons (E12-15). Absent backends render honestly (§7).
    system: system::SystemState,
    /// The KIRON chyron bridge (KIRON-2) — the shell's one `ToastHost` plus its
    /// `event/toast/show` Bus subscription, suppression posture, and the single
    /// notification-sound seam. Driven every frame; its lower-third band + OSD
    /// float above whatever surface (or fullscreen guest) is in view.
    toasts: toast_bridge::ToastBridge,
}

impl Shell {
    /// Build the shell and its three embedded surfaces once, off the eframe
    /// creation context (the surfaces' workers clone its egui `Context` so their
    /// off-thread updates repaint the one shell). This is the single "built once"
    /// mount point of E12-3b.
    fn new(cc: &CreationContext<'_>) -> Self {
        Self::new_for_ctx(&cc.egui_ctx)
    }

    /// Build the shell + its embedded surfaces over a bare egui [`egui::Context`] —
    /// the DRM-seat path (`run_drm`) has no eframe `CreationContext`, so `new` (the
    /// windowed `run_client` path) delegates here and both runners build one shell.
    fn new_for_ctx(ctx: &egui::Context) -> Self {
        Self {
            nav: Nav::default(),
            datacenter: datacenter::DatacenterState::default(),
            thisnode: thisnode::ThisNodeState::default(),
            network: network::NetworkState::default(),
            controller: controller::ControllerState::default(),
            provisioning: provisioning::ProvisioningState::default(),
            services: services_flow::ServicesFlowState::default(),
            chrome: chrome::ChromeState::default(),
            music: MusicApp::new_with_ctx(ctx),
            files: mde_files_egui::real_browser(),
            voice: VoiceApp::new_with_ctx(ctx),
            vdi: vdi::VdiState::default(),
            discovery: discovery::DiscoveryState::default(),
            instances: instances::InstancesState::default(),
            notifications: notifications::NotificationsState::default(),
            clipboard: clipboard::ClipboardState::default(),
            system: system::SystemState::default(),
            toasts: toast_bridge::ToastBridge::default(),
        }
    }

    /// The expanded shell body: the dock rail plus the one active surface.
    ///
    /// The shell owns the frame loop, so it drives the active surface itself —
    /// its per-frame **pump** (the worker-update drain the surface kept out of the
    /// panel fn), then its **header**, then its central **panel** — because the
    /// surface's own `App::update` is never called here. Each mounted surface is
    /// scoped under a unique [`egui::Ui::push_id`] so its internal egui ids (esp.
    /// Files' fixed `files-top` / `files-side` panels) can't collide with another
    /// surface's in the shell's one `Context`. The Workbench keeps its live Fleet
    /// plane (MV-6).
    fn body(&mut self, ui: &mut egui::Ui) {
        egui::SidePanel::left("shell-dock")
            .resizable(false)
            .exact_width(Style::SP_XL * 4.0)
            .show_inside(ui, |ui| {
                dock::rail(ui, &mut self.nav.surface);
            });

        match self.nav.surface {
            Surface::Workbench => {
                workbench::show(
                    ui,
                    &mut self.nav.plane,
                    &mut self.datacenter,
                    &self.thisnode,
                    &self.network,
                    &self.controller,
                    &self.provisioning,
                    &mut self.services,
                );
            }
            Surface::Desktop => {
                // The Desktop surface's no-session face IS the E12-5b remote-desktop
                // picker: with nothing requested it lists the mesh's VMs; Connect
                // hands a target to `vdi`, and the surface flips to the desktop
                // (connecting caption until the gated E12-4 wire transport attaches
                // the live decoder). This mirrors E12-5a driving the surface.
                if self.vdi.requested_target().is_none() {
                    let discovery = &mut self.discovery;
                    let picked = ui
                        .push_id("shell-discovery", |ui| {
                            discovery::discovery_panel(ui, discovery);
                            discovery.take_connect()
                        })
                        .inner;
                    if let Some(target) = picked {
                        self.vdi.request_target(target);
                    }
                } else {
                    // The VDI desktop fills the body. It reserves an Esc chord that
                    // asks to return to the mesh-control chrome — honour it by
                    // clearing the pending target (back to the picker) and falling
                    // back to the Workbench so a session is never a trap.
                    let vdi = &mut self.vdi;
                    let leave = ui
                        .push_id("shell-desktop", |ui| {
                            vdi::vdi_panel(ui, vdi);
                            vdi.take_return_to_chrome()
                        })
                        .inner;
                    if leave {
                        self.vdi.clear_target();
                        self.nav.surface = Surface::Workbench;
                    }
                }
            }
            Surface::Instances => {
                // The local cloud-hypervisor VM broker (E12-7). Scoped under its
                // own `push_id` like every mounted surface so its egui ids can't
                // collide in the shell's one `Context`.
                let instances = &mut self.instances;
                ui.push_id("shell-instances", |ui| {
                    instances::instances_panel(ui, instances);
                });
            }
            Surface::Music => {
                music_pump(&mut self.music);
                let music = &mut self.music;
                ui.push_id("shell-music", |ui| {
                    music_header(ui, music);
                    ui.separator();
                    music_panel(ui, music);
                });
            }
            Surface::Files => {
                let files = &mut self.files;
                ui.push_id("shell-files", |ui| {
                    files_panel(ui, files);
                });
            }
            Surface::Voice => {
                voice_pump(&mut self.voice);
                let voice = &mut self.voice;
                ui.push_id("shell-voice", |ui| {
                    voice_header(ui, voice);
                    ui.separator();
                    voice_panel(ui, voice);
                });
            }
            Surface::Notifications => {
                let notifications = &mut self.notifications;
                ui.push_id("shell-notifications", |ui| {
                    notifications.show(ui);
                });
            }
            Surface::Clipboard => {
                let clipboard = &mut self.clipboard;
                ui.push_id("shell-clipboard", |ui| {
                    clipboard.show(ui);
                });
            }
            Surface::System => {
                // This seat's host controls, folded from the one `mde-seat` Seat
                // (E12-15). Scoped under its own `push_id` like every mounted
                // surface so its egui ids can't collide in the shell's one
                // `Context`. The snapshot is refreshed in `render` (it also feeds
                // the chrome icons), so the panel only renders here.
                // The System panel drives Displays + Power live (E12-18); its
                // per-VM power rows reuse the Instances broker (§6), so it takes a
                // `&mut` to that roster — two disjoint field borrows of the shell.
                let system = &mut self.system;
                let instances = &mut self.instances;
                ui.push_id("shell-system", |ui| {
                    system.show(ui, instances);
                });
            }
        }
    }
}

impl eframe::App for Shell {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.render(ctx);
    }
}

impl Shell {
    /// The shell's per-frame render — every panel drawn into `ctx`. Driven by the
    /// eframe `App::update` (windowed `run_client`) AND directly by the DRM runner
    /// (`run_drm`), which owns the seat with a bare `Context` and no eframe `Frame`.
    /// The body never touched `Frame`, so both runners render identically.
    fn render(&mut self, ctx: &egui::Context) {
        // The Fleet, This Node, Network, Controller + Provisioning planes subscribe
        // to live mesh state. Poll on the shared cadence while the Workbench surface
        // is in view (the reads are cheap local scans) so a host health flip, a new
        // VM, this node's own heartbeat / service flip, a peer join / leader change,
        // a control-service flip, or a node enrolling / an update landing surfaces
        // without operator input; the polls self-gate and keep the repaint heartbeat
        // alive. The app surfaces drive their own repaints from their workers.
        if self.nav.expanded && self.nav.surface == Surface::Workbench {
            self.datacenter.poll(ctx);
            self.thisnode.poll(ctx);
            self.network.poll(ctx);
            self.controller.poll(ctx);
            self.provisioning.poll(ctx);
            // The Services flow only actually reads while a request is in
            // flight (it self-gates on `pending`), so this is free otherwise.
            self.services.poll(ctx);
        }

        // The Desktop surface's picker (E12-5b) subscribes to the same live VM
        // roster while it's in view with no session requested — the cheap local
        // scan surfaces a new/removed remote desktop without operator input.
        if self.nav.expanded
            && self.nav.surface == Surface::Desktop
            && self.vdi.requested_target().is_none()
        {
            self.discovery.poll(ctx);
        }

        // The Notifications and Clipboard surfaces tail their respective bus
        // topics whenever the shell is expanded — cheap incremental reads that
        // keep the panels live so data is ready the instant the operator switches
        // to either surface (no cold-start 5-second wait).
        if self.nav.expanded {
            self.notifications.poll(ctx);
            self.clipboard.poll(ctx);
        }

        // The seat snapshot feeds BOTH the System surface and the always-visible
        // chrome status icons, so poll it every frame (self-gating on the shared
        // cadence) — the chrome's Bluetooth/Volume icons stay live even while the
        // System surface isn't the one in view.
        self.system.poll(ctx);

        // The thin persistent chrome bar (48px = SP_XL + SP_M).
        egui::TopBottomPanel::top("mcnf-chrome")
            .exact_height(Style::SP_XL + Style::SP_M)
            .show(ctx, |ui| {
                if chrome::show(
                    ui,
                    &mut self.chrome,
                    self.system.snapshot(),
                    self.nav.expanded,
                ) {
                    self.nav.toggle_expand();
                }
            });

        // Expand transition: 0.0 = collapsed (session), 1.0 = expanded (shell body
        // — the dock + the active surface).
        let t = Motion::animate(ctx, "shell-expand", self.nav.expanded, Motion::BASE);

        egui::CentralPanel::default().show(ctx, |ui| {
            // Cross-fade the two central views through the midpoint so they never
            // fight for layout: the session fades out over the first half, the
            // shell body fades in over the second.
            if t < 0.5 {
                ui.set_opacity((1.0 - t * 2.0).clamp(0.0, 1.0));
                session::show(ui);
            } else {
                let a = (t * 2.0 - 1.0).clamp(0.0, 1.0);
                ui.set_opacity(a);
                // A small rise as the shell body settles in.
                ui.add_space((1.0 - a) * Style::SP_S);
                self.body(ui);
            }
        });

        // Keep painting while the transition is in flight.
        if t > 0.001 && t < 0.999 {
            ctx.request_repaint();
        }

        // The KIRON chyron (KIRON-2) — driven last so its lower-third band + OSD
        // float (Foreground order) above the chrome, the surface, and any
        // fullscreen guest. Refresh the suppression posture (lock 10) first: a
        // fullscreen VDI guest in front is a per-session focus mute, and the seat's
        // audio-mute hushes a non-critical's sound. (DND has no shell toggle yet —
        // NOTIFY-CHAT owns it; a Critical breaks through regardless.)
        let focus_mute =
            self.nav.surface == Surface::Desktop && self.vdi.requested_target().is_some();
        let muted = self.system.snapshot().is_some_and(seat_master_muted);
        self.toasts.set_suppression(false, focus_mute, muted);
        if let Some(nav) = self.toasts.drive(ctx) {
            // A clicked chyron action navigates — THIS is where the verb executes
            // (KIRON-1 deliberately only reported it). Any target expands the shell.
            self.nav.expanded = true;
            match nav {
                toast_bridge::Navigate::Surface(surface) => self.nav.surface = surface,
                toast_bridge::Navigate::Plane(plane) => {
                    self.nav.surface = Surface::Workbench;
                    self.nav.plane = plane;
                }
            }
        }
    }
}

/// The seat's master-output mute, if the mixer probe answered — gates a
/// non-critical chyron's notification sound (KIRON lock 8). No mixer backend reads
/// as *not* muted (an absent probe never silences an alert).
fn seat_master_muted(snap: &SeatSnapshot) -> bool {
    matches!(&snap.mixer, Probe::Present(status) if status.master.muted)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // E12-3 — the shell OWNS the DRM/KMS seat directly (no compositor, no display
    // manager) when built `--features drm` and a seat is available. It falls back to
    // the windowed eframe client only when there is no DRM master (a dev host, or a
    // compositor already holds the seat) — the exact fallback E12-2 designed in.
    #[cfg(feature = "drm")]
    {
        let mut shell: Option<Shell> = None;
        match mde_egui::run_drm("org.magicmesh.Shell", |ctx| {
            shell
                .get_or_insert_with(|| Shell::new_for_ctx(ctx))
                .render(ctx);
        }) {
            Ok(()) => return Ok(()),
            Err(mde_egui::drm::DrmError::NoDrmMaster(why)) => {
                eprintln!(
                    "mde-shell-egui: no DRM seat ({why}); falling back to the windowed client"
                );
            }
            Err(e) => return Err(Box::new(e)),
        }
    }
    run_client("org.magicmesh.Shell", "MCNF", Shell::new).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{dock, files_panel, Nav, Plane, Surface};
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    #[test]
    fn shell_starts_collapsed_on_the_workbench() {
        let n = Nav::default();
        assert!(
            !n.expanded,
            "the shell opens to the session view, not the shell body"
        );
        assert_eq!(n.surface, Surface::Workbench);
        assert_eq!(n.plane, Plane::ThisNode);
    }

    #[test]
    fn toggle_expand_flips_the_shell_body() {
        let mut s = Nav::default();
        assert!(!s.expanded);
        s.toggle_expand();
        assert!(s.expanded);
        s.toggle_expand();
        assert!(!s.expanded);
    }

    /// Drive one headless frame that reproduces the shell's expanded **body mount**
    /// — the dock rail plus a surface scoped under `push_id`, inside the shell's
    /// `CentralPanel` — then tessellate it on the CPU so any paint-path fault
    /// surfaces as a failure. This is the same `Context::run` → `tessellate` path
    /// the DRM runner drives, minus the GPU (no window, no wgpu).
    ///
    /// Files is the surface a unit test can build (`MusicApp`/`VoiceApp` need an
    /// eframe `CreationContext`, which only `eframe::run_native` supplies, and
    /// Voice would spawn its SIP agent). It renders over the **real** backend — no
    /// demo data; with no `mackesd` Bus on the build host it shows its honest
    /// "standalone / no mesh" state, which is still a full paint path. This proves
    /// the shell's mount mechanism (dock + `push_id` scoping + the surface's own
    /// `files-top`/`files-side` panels nested in the shell's one `Context`) is
    /// runtime-reachable and actually draws. Music and Voice mount through the
    /// identical `body` path with their own headless render tests proving
    /// `music_panel`/`voice_panel` + header tessellate.
    #[test]
    fn shell_mounts_and_renders_a_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut files = mde_files_egui::real_browser();
        let mut active = Surface::Files;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::SidePanel::left("shell-dock")
                    .resizable(false)
                    .exact_width(Style::SP_XL * 4.0)
                    .show_inside(ui, |ui| dock::rail(ui, &mut active));
                ui.push_id("shell-files", |ui| files_panel(ui, &mut files));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted surface produced no draw primitives"
        );
    }
}
