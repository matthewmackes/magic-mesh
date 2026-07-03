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

mod auth;
mod backdrop;
mod chat;
mod chooser;
mod chrome;
mod controller;
mod datacenter;
mod discovery;
mod dock;
mod formfactor;
mod host_mirror;
mod hotkeys;
mod instances;
mod keyboard;
mod network;
mod provisioning;
mod services_flow;
mod session;
mod storage;
mod surface_card;
mod system;
mod thisnode;
mod toast_bridge;
mod vdi;
mod web;
mod workbench;

use mde_egui::eframe::CreationContext;
use mde_egui::{eframe, egui, run_client, Density, Motion, Style};

use mde_seat::hotkeys::HotkeyAction;
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
    /// The This Node plane's SURFACE-6 "Surface / Hardware Enablement" card — a
    /// model-gated card that renders the `mackesd` surface workers' typed state
    /// (SURFACE-2/3/4/5/7) off the Bus and drives their typed verbs. Appears only
    /// on a detected Surface (the summary topic is the gate); inert otherwise.
    surface_card: surface_card::SurfaceCardState,
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
    /// The Desktop Chooser (CHOOSER-2) — the Desktop surface's no-session face:
    /// the card grid of every discovered desktop (mesh peers · LAN mDNS · local
    /// VMs · manual), grouped by node over the BRAND-1 backdrop, rendered from
    /// the CHOOSER-1 worker's `state/desktops/sources` roster. A card connect
    /// emits the broker `Open` request (via the `discovery` wire path) + hands
    /// the target to `vdi`; its seen-set fold auto-pops the Chooser when a new
    /// source is discovered (design lock 1).
    chooser: chooser::ChooserState,
    /// The Instances surface — this workstation's local cloud-hypervisor VMs via
    /// the `mde-kvm` broker (E12-7). Create / boot / shutdown drive mde-kvm's real
    /// lifecycle; with no live VMM the ops surface mde-kvm's typed gated error, and
    /// an empty roster shows the honest "No local VMs" EmptyState.
    instances: instances::InstancesState,
    /// The Chat surface (NOTIFY-CHAT-3) — the ICQ roster + conversation panes over
    /// the chat worker's `state/chat/roster` + `state/chat/conversation/<key>`
    /// read-model. A pure renderer; sends via `action/chat/send`.
    chat: chat::ChatState,
    /// The System surface — this seat's host controls, folded from the ONE
    /// `mde-seat` `Seat` (lock 1): mixer / Bluetooth / displays / power & battery /
    /// backlight / hotkeys. Its cached snapshot also feeds the three read-only
    /// chrome status icons (E12-15). Absent backends render honestly (§7).
    system: system::SystemState,
    /// The Storage surface — GParted-authentic disk/partition management (E12-21).
    /// Folds `state/storage/<node>` mirrors (UDisks2 topology + backend availability)
    /// per peer, renders segment bars + partition tables + a typed-armed pending-op
    /// queue, and drives `action/storage/<node>` back onto the Bus. The `mackesd`
    /// storage worker owns the hard walls + the executor (live apply is E12-23).
    storage: storage::StorageState,
    /// The KIRON chyron bridge (KIRON-2) — the shell's one `ToastHost` plus its
    /// `event/toast/show` Bus subscription, suppression posture, and the single
    /// notification-sound seam. Driven every frame; its lower-third band + OSD
    /// float above whatever surface (or fullscreen guest) is in view.
    toasts: toast_bridge::ToastBridge,
    /// The hotkey dispatcher (E12-19) — the fixed `mde_seat` table on the shell
    /// input path (lock 8/9). Carries only the leader latch; each frame it folds the
    /// seat's forwarded host keys (XF86 media + Super) and this frame's egui key
    /// presses into typed actions the shell applies to the seat / nav.
    hotkeys: hotkeys::HotkeyRouter,
    /// SURFACE-9 (lock 9): republishes the seat's debounced `SW_TABLET_MODE` /
    /// Type-Cover formfactor transitions to the mesh Bus (`event/hardware/formfactor`)
    /// so the tablet-mode UX reacts. Empty on the windowed fallback (self-gates).
    formfactor: formfactor::FormfactorPublisher,
    /// SURFACE-10 (lock 14): the native on-screen keyboard overlay. Auto-raises when
    /// the formfactor (fed from the publisher above) is Tablet and a text field has
    /// focus, injecting key presses into the same egui input pipeline. Inert on Laptop.
    keyboard: keyboard::Keyboard,
    /// The Browser surface (BOOKMARKS-6) — the sandboxed `mde-web-preview` Servo
    /// helper driven over the per-session IPC socket and displayed by uploading its
    /// shm frames to an egui texture (`mde-web-preview-client`). Holds no live tab
    /// until the gated `live-helper` spawn attaches one; the panel shows its honest
    /// gated EmptyState until then, exactly like the VDI Desktop surface.
    web: web::WebState,
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
            surface_card: surface_card::SurfaceCardState::default(),
            network: network::NetworkState::default(),
            controller: controller::ControllerState::default(),
            provisioning: provisioning::ProvisioningState::default(),
            services: services_flow::ServicesFlowState::default(),
            chrome: chrome::ChromeState::default(),
            music: MusicApp::new_with_ctx(ctx),
            files: mde_files_egui::real_browser(),
            voice: VoiceApp::new_with_ctx(ctx),
            vdi: vdi::VdiState::default(),
            chooser: chooser::ChooserState::default(),
            instances: instances::InstancesState::default(),
            chat: chat::ChatState::default(),
            system: system::SystemState::default(),
            storage: storage::StorageState::default(),
            toasts: toast_bridge::ToastBridge::default(),
            hotkeys: hotkeys::HotkeyRouter::default(),
            formfactor: formfactor::FormfactorPublisher::default(),
            keyboard: keyboard::Keyboard::default(),
            web: web::WebState::default(),
        }
    }

    /// Apply one dispatched hotkey action (E12-19). Hardware actions act on the ONE
    /// seat through the System state (volume/brightness flash the KIRON OSD tier);
    /// navigation actions move the shell itself — leaving a fullscreen guest is the
    /// Esc-chord reservation generalized (lock 8).
    fn apply_hotkey(&mut self, action: HotkeyAction) {
        match action {
            HotkeyAction::SessionSwitch | HotkeyAction::MonitorFocusSwitch => {
                // Bring the guest session to the front. One desktop session exists
                // today; cycling across multiple sessions / monitors is the gated
                // multi-session broker (E12-4/E12-10), so this shows the Desktop
                // surface rather than silently doing nothing.
                self.nav.expanded = true;
                self.nav.surface = Surface::Desktop;
            }
            HotkeyAction::ReturnToChrome => {
                // Leave a fullscreen guest for the mesh-control chrome — release any
                // VDI target and show the Workbench (a session is never a trap).
                self.vdi.clear_target();
                self.nav.expanded = true;
                self.nav.surface = Surface::Workbench;
            }
            HotkeyAction::OpenSystem => {
                self.nav.expanded = true;
                self.nav.surface = Surface::System;
            }
            // Hardware — act on the seat; a volume/brightness change flashes the OSD.
            hardware => {
                if let Some(level) = self.system.dispatch_hotkey(hardware) {
                    self.toasts.flash_osd(level);
                }
            }
        }
    }

    /// Apply a resolved [`toast_bridge::Navigate`] to the shell nav — the ONE place
    /// a `shell/goto/<surface>` / `shell/plane/<plane>` verb executes, shared by the
    /// KIRON chyron action and the chrome unread indicator (NOTIFY-CHAT-6). Any
    /// target expands the shell (a navigation is never a no-op behind the session).
    fn apply_nav(&mut self, nav: toast_bridge::Navigate) {
        self.nav.expanded = true;
        match nav {
            toast_bridge::Navigate::Surface(surface) => self.nav.surface = surface,
            toast_bridge::Navigate::Plane(plane) => {
                self.nav.surface = Surface::Workbench;
                self.nav.plane = plane;
            }
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
                    &mut self.surface_card,
                    &self.network,
                    &self.controller,
                    &self.provisioning,
                    &mut self.services,
                );
            }
            Surface::Desktop => {
                // The Desktop surface's no-session face IS the Desktop Chooser
                // (CHOOSER-2, superseding the E12-5b flat picker): with nothing
                // requested it shows the discovered-desktop card grid over the
                // BRAND-1 backdrop; the CHOOSER-4 picker hands a `ConnectRequest`
                // (protocol + display + monitors) to `vdi`, and the surface flips
                // to the desktop (connecting caption until the gated E12-4 wire
                // transport attaches the live decoder).
                if self.vdi.requested_target().is_none() {
                    let chooser = &mut self.chooser;
                    let picked = ui
                        .push_id("shell-chooser", |ui| {
                            chooser::chooser_panel(ui, chooser);
                            chooser.take_connect()
                        })
                        .inner;
                    if let Some(request) = picked {
                        self.vdi.request_connect(request);
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
            Surface::Browser => {
                // The sandboxed Servo browser (BOOKMARKS-6) — the `mde-web-preview`
                // helper driven over IPC and displayed by uploading its shm frames
                // to an egui texture. Scoped under its own `push_id` like every
                // mounted surface. The panel polls + drives its own tabs; a live
                // session attaches only via the gated `live-helper` spawn (else the
                // honest gated EmptyState), so nothing is faked here (§7).
                let web = &mut self.web;
                ui.push_id("shell-web", |ui| {
                    web::web_panel(ui, web);
                });
                // Respawn-on-reload: a crashed tab's Reload asked to restart. The
                // live helper spawn is the client crate's gated `live-helper` path
                // (honest-gated to a GPU seat), so the shell drains + acknowledges
                // the request here; a live build swaps in a fresh session. No live
                // tabs exist in the default build, so this is inert (never a faked
                // page, §7).
                let _restart_requested = self.web.take_respawn_request();
            }
            Surface::Chat => {
                let chat = &mut self.chat;
                ui.push_id("shell-chat", |ui| {
                    chat.show(ui);
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
            Surface::Storage => {
                // GParted disk/partition management (E12-21) — scoped under its own
                // `push_id` like every surface; the storage worker owns the walls.
                let storage = &mut self.storage;
                ui.push_id("shell-storage", |ui| storage.show(ui));
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
        // SURFACE-10: flush any key the OSK queued last frame into THIS frame's input,
        // before the focused field draws, so it consumes them exactly like a hardware
        // key (a no-op when nothing is queued).
        self.keyboard.flush_pending(ctx);

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
            self.surface_card.poll(ctx);
            self.network.poll(ctx);
            self.controller.poll(ctx);
            self.provisioning.poll(ctx);
            // The Services flow only actually reads while a request is in
            // flight (it self-gates on `pending`), so this is free otherwise.
            self.services.poll(ctx);
        }

        // The Desktop Chooser (CHOOSER-2) tails the CHOOSER-1 worker's
        // `state/desktops/sources` roster on its shared cadence EVERY frame, not
        // just while in view: the auto-popup lock (design lock 1) needs the fold
        // to see a newly-discovered source id whenever it lands. The read is the
        // same cheap local spool scan the other planes poll (it self-gates).
        self.chooser.poll(ctx);
        if self.chooser.take_popup() && self.vdi.requested_target().is_none() {
            // A new desktop source surfaces the Chooser through the same
            // central-view switch the hotkeys/chyron drive — but never over a
            // live/pending session (a popup must not yank an attached desktop;
            // the drained event is deliberately dropped in that case).
            self.nav.expanded = true;
            self.nav.surface = Surface::Desktop;
        }

        // The Chat surface — the ONE notification interface (folded alerts +
        // clipboard clips + human chat) — tails its `state/chat/*` read-model
        // whenever the shell is expanded: a cheap incremental read that keeps the
        // roster + conversations live so data is ready the instant the operator
        // switches to it, and drives the chrome unread indicator (no cold-start
        // wait). This subsumes the retired Notifications + Clipboard polls
        // (NOTIFY-CHAT-6).
        if self.nav.expanded {
            self.chat.poll(ctx);
        }

        // The Storage surface tails the `state/storage/*` mirrors + the selected
        // peer's progress lane while it's in view — a cheap local scan so a UDisks2
        // change on any peer surfaces without operator input (E12-21).
        if self.nav.expanded && self.nav.surface == Surface::Storage {
            self.storage.poll(ctx);
        }

        // The seat snapshot feeds BOTH the System surface and the always-visible
        // chrome status icons, so poll it every frame (self-gating on the shared
        // cadence) — the chrome's Bluetooth/Volume icons stay live even while the
        // System surface isn't the one in view.
        self.system.poll(ctx);

        // The thin persistent chrome bar (48px = SP_XL + SP_M).
        let unread = self.chat.total_unread();
        egui::TopBottomPanel::top("mcnf-chrome")
            .exact_height(Style::SP_XL + Style::SP_M)
            .show(ctx, |ui| {
                let outcome = chrome::show(
                    ui,
                    &mut self.chrome,
                    self.system.snapshot(),
                    self.nav.expanded,
                    unread,
                );
                if outcome.toggled {
                    self.nav.toggle_expand();
                }
                // The unread indicator opens the unified Chat surface through the
                // ONE `shell/goto/chat` nav grammar (the same resolver the KIRON
                // chyron uses) — no second navigation path in the chrome.
                if outcome.open_chat {
                    if let Some(nav) = toast_bridge::resolve_action("shell/goto/chat") {
                        self.apply_nav(nav);
                    }
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

        // E12-19 — hotkey dispatch (lock 8), driven each frame before the OSD paint
        // so a volume/brightness flash lands this same frame. Drain the seat's
        // forwarded host keys (XF86 media + the Super leader; empty on the windowed
        // fallback, so the wiring self-gates to the real DRM seat) and this frame's
        // egui key presses, route them through the fixed table, and apply each typed
        // action to the seat / nav.
        // SURFACE-9 (lock 9): republish the seat's debounced formfactor transitions to
        // the mesh Bus each frame (a no-op unless the seat reported a Tablet↔Laptop
        // flip). Empty on the windowed fallback, so it self-gates to the real seat.
        // SURFACE-10 (lock 14): the same flip feeds the OSK so it auto-raises on Tablet.
        if let Some(formfactor) = self.formfactor.pump() {
            self.keyboard.set_formfactor(formfactor);
            // SURFACE-11 (lock 16): the same flip re-installs the interaction density —
            // Tablet grows hit targets + spacing (touch), Laptop reverts to the compact
            // pointer metrics. Keyed off the real SURFACE-9 signal, mesh-wide.
            Style::install_with_density(ctx, Density::for_formfactor(formfactor));
        }

        // SURFACE-11 (lock 16): a swipe from the left/bottom edge reveals the shell body
        // (the dock / tablet bar). Drained from the seat's gesture side channel; empty
        // on the windowed fallback, so the reveal self-gates to the real DRM seat.
        for edge in mde_egui::drain_edge_swipes() {
            if matches!(edge, mde_egui::Edge::Left | mde_egui::Edge::Bottom) {
                self.nav.expanded = true;
            }
        }

        let host_keys = mde_egui::hostkeys::drain_host_keys();
        let presses = ctx.input(|i| hotkeys::egui_key_presses(&i.events));
        for action in self.hotkeys.dispatch(&host_keys, &presses) {
            self.apply_hotkey(action);
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
            self.apply_nav(nav);
        }

        // SURFACE-10 (lock 14): the on-screen keyboard overlay — drawn last (Foreground)
        // so it floats above the chrome, the active surface, and any fullscreen guest.
        // It reads the live focus + the cached formfactor and self-manages its raise /
        // dismiss; on a Laptop (or the windowed fallback) it stays inert.
        self.keyboard.show(ctx);
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
