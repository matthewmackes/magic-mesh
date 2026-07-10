//! `mde-shell-egui` — the single MCNF E12 "Quasar" egui shell (E12-3).
//!
//! One eframe app on the `mde-egui` harness. The shell has **ONE chrome** — the
//! left **vertical dock** (VDOCK, `dock::dock`: the slide-in, auto-hide app picker
//! + bottom status/system quads; the old horizontal taskbar and the top chrome
//! strip are both retired). Beside it, the central view is either:
//!
//! * the **session `EmptyState`** (collapsed) — a real session is a fullscreen VM
//!   texture from `mde-vdi`; or
//! * the active **surface** (expanded) — Workbench / Mesh Map / the app
//!   surfaces, selected on the dock (the dock IS the nav; any dock click
//!   surfaces the body).
//!
//! The session↔body transition eases through the shared `Motion` table and the
//! whole surface renders through the shared `Style` (governance §4/§5/§7). This is
//! the skeleton the panels (Workbench/Files/Music/Voice) and the VM session-view
//! plug into.

mod about;
mod auth;
mod backdrop;
mod bt_pairing;
mod chat;
mod chooser;
mod chrome;
mod console;
mod controller;
mod curtain;
mod datacenter;
mod device_manager;
mod discovery;
mod dock;
mod explorer;
mod formfactor;
mod host_mirror;
mod hotkeys;
mod iac;
mod keyboard;
mod lock_signal;
mod mesh_view;
mod network;
mod pam_auth;
mod phones_hub;
mod power_honor;
mod power_settings;
mod provisioning;
// WIN7-SHOT-1 — a headless CPU screenshot capture, test-only tooling (see the
// module doc): never compiled into the production binary, so it is gated here
// rather than declared like every real surface module above/below it.
#[cfg(test)]
mod screenshot;
mod services_flow;
mod session;
mod session_rail;
mod spawn_lighthouse_flow;
mod splash;
mod start_menu;
mod status;
mod storage;
mod surface_card;
mod system;
mod thisnode;
mod timers;
mod toast_bridge;
mod vdi;
mod web;
mod workbench;

use mde_egui::{eframe, egui, run_client, Density, Motion, Style};

use mde_seat::hotkeys::HotkeyAction;
use mde_seat::{Probe, SeatSnapshot};

use mde_bookmarks_egui::{
    bookmarks_panel, real_manager, BookmarksBus, Manager as BookmarksManager,
};
use mde_editor_egui::{editor_panel, real_editor, EditorSurface};
use mde_files::editor_open::EditorLaunchWatch;
use mde_files_egui::{files_panel, FileBrowser};
use mde_media_egui::{
    media_header, media_panel, media_pump, real_media, MediaSurface, VideoTextureCache,
};
use mde_music_egui::{music_header, music_panel, music_pump, MusicApp};
use mde_term_egui::{real_terminal, terminal_panel, terminal_pump, TerminalSurface};
use mde_voice_egui::{voice_menubar, voice_panel, voice_pump, VoiceApp};

use dock::Surface;
// CURTAIN-3 — the logind lock-signal receive seam, so `render` can poll the
// listener source for `loginctl lock-session` (the trait's `poll`).
use lock_signal::LockSignals;
use workbench::Plane;

/// The shell's pure navigation state: whether the shell body (the active
/// surface) is showing over the session view, and which plane the Workbench has
/// selected. Kept separate from the surface apps (which need an eframe
/// `CreationContext` to build) so the nav invariants stay unit-testable without
/// a GPU. The old chrome Expand/Collapse toggle is retired (the dock is the nav):
/// any dock click, hotkey, chyron action, or edge swipe surfaces the body.
#[derive(Default)]
struct Nav {
    /// `true` while the shell body (the active surface) fills the central view.
    expanded: bool,
    /// Which surface fills the shell body (Workbench by default).
    surface: Surface,
    /// The Workbench plane shown when the Workbench surface is active.
    plane: Plane,
}

/// The whole shell: the nav state, the live chrome/Fleet Bus state, and the three
/// embedded mesh-control surfaces it owns and drives per frame (E12-3b EMBED).
struct Shell {
    /// Body-vs-session state + the active surface + the selected Workbench plane.
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
    /// The Spawn Lighthouse flow (OW-7) — the Provisioning plane's promote-to-
    /// lighthouse action: pick a cloud target, optionally an HA pair, preview the
    /// daemon's plan (dry-run), spawn over the Bus, and render the
    /// `spawn_lighthouse_onboard` worker's typed
    /// answer (plan summary / CA-migration steps / LAN-only retry hint / typed
    /// gated error).
    spawn_lighthouse: spawn_lighthouse_flow::SpawnLighthouseFlowState,
    /// The live mesh-status fold — peers + mesh health folded from the
    /// world-readable snapshot, polled on the shared cadence (self-gating in
    /// `render`). The dock grade band still reads this ONE poll's product.
    chrome: chrome::ChromeState,
    /// NOTIF-3 — daemon-owned notification segment rollups for the compact dock
    /// status strip. Missing daemon state renders as dim, not green.
    notify_status: status::StatusState,
    /// NOTIF-6 — ambient own-seat critical edge cue. Driven from the same daemon
    /// segment snapshot as the dock pips, with no text toast.
    critical_edge: status::CriticalEdgeCue,
    /// Local hostname used to decide whether a critical belongs to this seat.
    local_host: String,
    /// VDOCK-1/2/3/4 — the left vertical dock's state: the auto-hide half (the
    /// Super-tap reveal latch + the pin), the app picker's `active` surface, the
    /// bottom status-strip inputs, and the system-quad power menu + pending request.
    /// `dock::dock` reads + drives it each frame; the Super-tap reveal toggles it on
    /// the hotkey path (`hotkeys::HotkeyRouter::take_dock_toggle`); and
    /// `mount_dock_chrome` mirrors `nav.surface` in/out + drains its lock/power request.
    vdock: dock::DockState,
    /// WIN7-2 — the Start Menu's cross-frame state (the fixed-size overlay
    /// panel's own open latch + click-away guard, `docs/design/
    /// win7-desktop-survey.md` lock #13). The dock's Start cell AND a clean
    /// Super tap both latch its toggle (see `start_menu`'s module doc for how
    /// the Super tap fans out to this alongside VDOCK-1's own dock toggle);
    /// the shell drains it, mirrored open-state into [`Self::console`], and
    /// mounts the panel each frame (`start_menu::start_menu_panel`).
    start_menu: start_menu::StartMenuState,
    /// CONSOLE-1 — the Console front door (`docs/design/console-frontdoor.md`):
    /// the Win10-style taxonomy of operational entries. Pre-WIN7-2 this was
    /// its own standalone panel the dock's Start cell toggled directly; it is
    /// now embedded as the Start Menu's right pane ([`Self::start_menu`],
    /// lock #10) — `console::console_content` renders it there, and this
    /// field's own `open` bit is a mirror of the Start Menu's, not a second
    /// independent latch. The shell still drives its typed
    /// [`console::ConsoleRequest`]s the same way (a live surface-link routes
    /// the nav; command launches stay honest-gated on the CONSOLE-2 spawn-tab
    /// seam, §7).
    console: console::ConsoleState,
    /// The Music surface, owned + built once (its worker thread wakes the shell's
    /// egui context on every update). Rendered via `mde_music_egui::music_panel`.
    music: MusicApp,
    /// The Media surface (MEDIA-18) — the production `MediaController` over the real
    /// `mde_media_core` backend (Player / Library / Playlist), built once by
    /// `mde_media_egui::real_media()`. Driven per-frame (pump + header + panel) the
    /// same way Music/Files/Voice are, so the whole media player (Sources / Library /
    /// Player / Queue) is reachable as an in-shell surface — no demo data (§7).
    media: MediaSurface,
    /// The Media surface's MEDIA-2 phase-1 frame-sink texture cache
    /// (`docs/gpu_encoder.md`) — owned alongside `media` so the Player tab's
    /// video stage's `TextureHandle` persists across frames instead of
    /// re-uploading a GPU texture every call. Only the real mpv engine
    /// (`--features media-mpv`) ever populates it; `FakeMpv` (the default)
    /// leaves it empty and the stage paints its placeholder, exactly as before.
    media_video: VideoTextureCache,
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
    /// NAVBAR-U3 — local projection of the broker VDI session log for the bottom
    /// rail. Falls back to the pending `VdiState` request until the broker log has
    /// a matching session for this seat.
    session_rail: session_rail::SessionRailState,
    /// The Infra as Code (`IaC`) surface — the `OpenStack` `IaaS` control plane
    /// (IAC-2). Consumes the Keystone service catalog + per-service API health off
    /// the Bus read verb `action/cloud/get-catalog` (no shell→mackesd dep, §6) and renders
    /// the Overview: the API status band + the merged service directory. Honest
    /// "not configured / unreachable" when the cloud/verb is absent (§7).
    infra_code: iac::InfraCodeState,
    /// The Chat surface (NOTIFY-CHAT-3) — the ICQ roster + conversation panes over
    /// the chat worker's `state/chat/roster` + `state/chat/conversation/<key>`
    /// read-model. A pure renderer; sends via `action/chat/send`.
    chat: chat::ChatState,
    /// The Phones hub surface (KDC-MESH-9) — the desktop-side management surface for
    /// the mesh's paired phone(s). A thin client of the `kdc_host` worker: it renders
    /// the live device roster (`action/connect/devices`) + the mesh service directory
    /// (the replicated `kdc-services/*.json`, KDC-MESH-7) and drives the operator
    /// verbs (unpair / ring / clipboard / sftp / browse). Polled while in view.
    phones_hub: phones_hub::PhonesHubState,
    /// The System surface — this seat's host controls, folded from the ONE
    /// `mde-seat` `Seat` (lock 1): mixer / Bluetooth / displays / power & battery /
    /// backlight / hotkeys. Its cached snapshot feeds the System surface and
    /// remains available to dock status/panel work. Absent backends
    /// render honestly (§7).
    system: system::SystemState,
    /// The Storage surface — GParted-authentic disk/partition management (E12-21).
    /// Folds `state/storage/<node>` mirrors (UDisks2 topology + backend availability)
    /// per peer, renders segment bars + partition tables + a typed-armed pending-op
    /// queue, and drives `action/storage/<node>` back onto the Bus. The `mackesd`
    /// storage worker owns the hard walls + the executor (live apply is E12-23).
    storage: storage::StorageState,
    /// The About surface — the Device-Manager hardware inspector (DEVMGR-2). Reads
    /// THIS node's published `device-inventory/<host>.json` (the §6 JSON contract
    /// the `hardware_probe` worker publishes, DEVMGR-1) on a cadence + on Scan, and
    /// renders the faithful Windows-Device-Manager by-type tree + rich header card
    /// + menu/toolbar chrome in `mde_egui` dark tokens, with the brand shrunk to a
    /// title strip + an ⓘ dialog. A pure consumer — it drives no worker.
    device_manager: device_manager::DeviceManagerState,
    /// The KIRON alert/OSD bridge (KIRON-2) — the shell's one `ToastHost` plus its
    /// `event/toast/show` Bus subscription, suppression posture, and the single
    /// notification-sound seam. Driven every frame; only its centered OSD pill
    /// floats above whatever surface (or fullscreen guest) is in view; notification
    /// visuals live in Chat.
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
    /// The Bookmarks manager surface (BOOKMARKS-4), mounted in-shell so Browser
    /// users can reach folders/tags/search/dead-link workflows without leaving the
    /// platform chrome. Persistence and mesh sync remain owned by the bookmarks
    /// worker; this is the existing egui manager over the CRDT model.
    bookmarks: BookmarksManager,
    bookmarks_bus: BookmarksBus,
    /// The Terminal surface (TERM-16) — the production `TerminalSurface` (the
    /// TERM-4/5/8 `TabbedTerminal`: tabs / splits / broadcast / a shell on any mesh
    /// peer) over a real local PTY, built once by `mde_term_egui::real_terminal()`.
    /// Driven per-frame (pump + panel) the same way Media is, so the whole
    /// Terminator-class terminal is reachable as an in-shell surface — no demo data
    /// (§7). This is the RESCUE: before it, `mde-term-egui` was mounted nowhere.
    terminal: TerminalSurface,
    /// The Editor surface (EDITOR-1) — the native Zed-style code editor
    /// (`mde-editor-egui`), mounted exactly like Files/Terminal: the shell holds
    /// its `EditorSurface` (built by `real_editor()`) and renders it per-frame with
    /// `editor_panel`. EDITOR-1 is the mountable scaffold — the editor chrome + the
    /// honest "No file open" empty state (§7); the rope buffer + text widget +
    /// tree-sitter highlighting land in EDITOR-2 onward, filling this surface.
    editor: EditorSurface,
    /// EDITOR-9 — the Files "Send-to-Editor" drain: tails `action/editor/open` and
    /// opens the requested path in the Editor surface (`EditorSurface::open_path`),
    /// bringing it to the front. The receive half of the same persist-first verb
    /// pattern the Send-To actions use; a dark Bus is an honest no-op.
    editor_launch: EditorLaunchWatch,
    /// The Mesh Map surface (OW-10) — the live `mde-mesh-view` canvas, fed a
    /// `MeshState` folded from the same world-readable mesh-status snapshot the
    /// Workbench planes read. Polled while in view; opens the honest "waiting for
    /// mesh" EmptyState until a snapshot lands.
    mesh_view: mesh_view::MeshViewState,
    /// The Discovery hero-card surface (EXPLORER-3) — the cinematic one-unit-at-a-
    /// time view over every discovered unit (mesh peers · LAN hosts · `OpenStack`
    /// objects), folded from the aggregator's `state/units/*` mirrors (EXPLORER-1).
    /// A thin renderer (§6): it reads the Bus, never scans. Mounted as the Mesh
    /// Map surface's **Explorer** lens (the [`explorer::LENS_KEY`] toggle) pending
    /// the dedicated dock entry; polled only while that lens is visible (#24).
    explorer: explorer::ExplorerState,
    /// The onboard self-test watch (OW-10) — observes the `event/onboard/self-test`
    /// verdict lane and raises a one-shot edge the instant a node goes all-green, so
    /// the shell auto-opens the Mesh Map. The receive half of a flow whose publish
    /// half is integration-gated, exactly like the VDI / Browser transports.
    self_test: mesh_view::SelfTestWatch,
    /// The Timers & Alarms store (VDOCK-5) — countdown timers + daily alarms,
    /// owned by the SHELL (not the panel) and ticked once per frame, so a due
    /// timer/alarm fires its `event/notify/timer` notification even while the
    /// surface is closed (the clock's replacement, design lock #16/#20). The
    /// dock's clock-glyph strip opens `Surface::Timers` to edit it.
    timers: timers::TimersState,
    /// The POWER-5 idle + lid honorer — the compositorless DRM shell's own
    /// swayidle/logind-lid replacement. Ticked once per frame; enforces the
    /// operator's idle-suspend timeout + lid-close action (read from the System
    /// state's persisted config) against the ONE seat. Safe by default (idle off).
    power_honor: power_honor::PowerHonor,
    /// The CURTAIN-1 lock curtain — the full-screen lock layer (the pure
    /// state machine + the slide/settle-bounce motion + the giant clock face).
    /// While engaged it consumes ALL input (lock 10): the pointer through its
    /// whole-screen Foreground layer, the keyboard through its per-frame focus
    /// steal plus the hotkey / edge-swipe / central-view gates in `render`.
    /// Super+L drops it; unlocking runs the CURTAIN-2 PAM seam
    /// ([`curtain::Curtain::pam`] / [`pam_auth::PamVerifier`]) — the seat user's
    /// real system password, verified off the render thread (§7).
    curtain: curtain::Curtain,
    /// CURTAIN-3 — the logind session `Lock`/`Unlock` listener: a background thread
    /// forwards `loginctl lock-session` (and any session-manager lock) so `render`
    /// drops the same in-process [`curtain`](Self::curtain). Inert when there is no
    /// system bus / logind (headless CI, the windowed fallback) — honest, never a
    /// faked signal. The idle/lid Lock actions + the boot-gate feed the SAME curtain.
    lock_signal: lock_signal::LogindLockSource,
}

impl Shell {
    /// Build the shell + its embedded surfaces once over a bare egui
    /// [`egui::Context`] (the surfaces' workers clone it so their off-thread
    /// updates repaint the one shell) — the single "built once" mount point of
    /// E12-3b. Called mid-boot by [`Boot::frame`] (the QBRAND-4 `Surfaces`
    /// milestone), on the DRM seat and the windowed fallback alike.
    fn new_for_ctx(ctx: &egui::Context) -> Self {
        let mut shell = Self {
            nav: Nav::default(),
            datacenter: datacenter::DatacenterState::default(),
            thisnode: thisnode::ThisNodeState::default(),
            surface_card: surface_card::SurfaceCardState::default(),
            network: network::NetworkState::default(),
            controller: controller::ControllerState::default(),
            provisioning: provisioning::ProvisioningState::default(),
            services: services_flow::ServicesFlowState::default(),
            spawn_lighthouse: spawn_lighthouse_flow::SpawnLighthouseFlowState::default(),
            chrome: chrome::ChromeState::default(),
            notify_status: status::StatusState::default(),
            critical_edge: status::CriticalEdgeCue::default(),
            local_host: local_hostname(),
            vdock: dock::DockState::default(),
            start_menu: start_menu::StartMenuState::default(),
            console: console::ConsoleState::default(),
            music: MusicApp::new_with_ctx(ctx),
            media: real_media(),
            media_video: VideoTextureCache::default(),
            files: mde_files_egui::real_browser(),
            voice: VoiceApp::new_with_ctx(ctx),
            vdi: vdi::VdiState::default(),
            chooser: chooser::ChooserState::default(),
            session_rail: session_rail::SessionRailState::new(),
            infra_code: iac::InfraCodeState::default(),
            chat: chat::ChatState::default(),
            phones_hub: phones_hub::PhonesHubState::default(),
            system: system::SystemState::default(),
            storage: storage::StorageState::default(),
            device_manager: device_manager::DeviceManagerState::default(),
            toasts: toast_bridge::ToastBridge::default(),
            hotkeys: hotkeys::HotkeyRouter::default(),
            formfactor: formfactor::FormfactorPublisher::default(),
            keyboard: keyboard::Keyboard::default(),
            web: web::WebState::default(),
            bookmarks: real_manager(),
            bookmarks_bus: BookmarksBus::default(),
            terminal: real_terminal(),
            editor: real_editor(),
            editor_launch: EditorLaunchWatch::from_env(),
            mesh_view: mesh_view::MeshViewState::default(),
            explorer: explorer::ExplorerState::default(),
            self_test: mesh_view::SelfTestWatch::default(),
            timers: timers::TimersState::default(),
            power_honor: power_honor::PowerHonor::default(),
            curtain: curtain::Curtain::pam(),
            lock_signal: lock_signal::LogindLockSource::new(ctx),
        };

        // CURTAIN-3 boot-gate (design lock 2): when the persisted policy requires a
        // login at boot (the shipped default), start the shell **Locked** — drop the
        // curtain now, before the first surface renders, so the desktop is never shown
        // or interactable until the seat user's real password passes PAM. This does
        // NOT change the `.13`-style autostart: the service still starts the shell; the
        // shell just starts behind the curtain. `lock()` is idempotent, and the config
        // read folds an absent file to require-login (fail-secure).
        if power_honor::should_lock_at_boot(shell.system.power_honor_config()) {
            shell.curtain.lock();
        }
        shell
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
            HotkeyAction::Lock => {
                // CURTAIN-1 — Super+L drops the lock curtain (design lock 2).
                // The DM-less DRM shell IS this seat's locker, so Lock acts here
                // in the shell rather than routing to logind through the System
                // state (whose `PowerVerb::Lock` leg remains for external callers).
                self.curtain.lock();
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
    /// a `shell/goto/<surface>` / `shell/plane/<plane>` verb executes (the KIRON
    /// chyron action + the OW-10 self-test edge). Any target expands the shell
    /// (a navigation is never a no-op behind the session).
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

    /// NAVBAR-6 — Win10-style `Super`+`1`…`9`/`0` jumps into the dock's canonical
    /// visible launcher order. `Super+0` is the tenth slot; out-of-range slots are
    /// ignored honestly instead of wrapping to a different surface.
    fn apply_nav_slot(&mut self, slot: hotkeys::NavSlot) {
        if let Some(surface) = Surface::ALL.get(slot.index()).copied() {
            self.nav.expanded = true;
            self.nav.surface = surface;
        }
    }

    /// Poll the Mesh Map surface and — when its EXPLORER-3 **Explorer** lens is the
    /// one showing — the Discovery hero card, which tails `state/units/*` ONLY while
    /// that lens is visible: the honest reachable half of the #24 scan-active gate
    /// (the aggregator's in-process scan flag has no Bus seam yet, so nothing is
    /// published here — §7). The mesh fold is the same cheap local scan the
    /// Workbench planes poll (it self-gates).
    fn poll_mesh_map(&mut self, ctx: &egui::Context) {
        self.mesh_view.poll(ctx);
        let explorer_lens = ctx.data(|d| {
            d.get_temp::<bool>(egui::Id::new(explorer::LENS_KEY))
                .unwrap_or(false)
        });
        if explorer_lens {
            self.explorer.poll(ctx);
        }
    }

    /// The Mesh Map surface (OW-10) with its EXPLORER-3 sibling **Explorer** lens
    /// (the Discovery hero card). A slim segmented header toggles between the two
    /// topology lenses — the map (nodes + links) and the one-unit-at-a-time hero
    /// shelf over every discovered unit. The lens persists in egui memory under
    /// [`explorer::LENS_KEY`] so [`Self::poll_mesh_map`] reads the same choice; it
    /// defaults to the map, so OW-10's all-green auto-open still lands on the map.
    /// (Mounted here pending the dedicated dock picker entry — a clean seam.)
    fn show_mesh_map(&mut self, ui: &mut egui::Ui) {
        let mesh_view = &mut self.mesh_view;
        let explorer = &mut self.explorer;
        ui.push_id("shell-mesh-view", |ui| {
            let lens_id = egui::Id::new(explorer::LENS_KEY);
            let mut show_explorer = ui.data(|d| d.get_temp::<bool>(lens_id).unwrap_or(false));
            ui.horizontal(|ui| {
                ui.add_space(Style::SP_S);
                if ui.selectable_label(!show_explorer, "Mesh Map").clicked() {
                    show_explorer = false;
                }
                if ui.selectable_label(show_explorer, "Explorer").clicked() {
                    show_explorer = true;
                }
            });
            ui.data_mut(|d| d.insert_temp(lens_id, show_explorer));
            ui.separator();
            if show_explorer {
                explorer.show(ui);
            } else {
                mesh_view.show(ui);
            }
        });
    }

    /// The expanded shell body: the one active surface. (The dock chrome is NOT
    /// mounted here — `render` mounts the floating dock before the central view,
    /// session and body alike.)
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
                    &mut self.spawn_lighthouse,
                );
            }
            Surface::MeshView => self.show_mesh_map(ui),
            Surface::Desktop => {
                // MENUBAR-ALL — the shared top bar (DESKTOP), mounted above whichever
                // face renders below (the Chooser or the brokered desktop). Its two
                // menus are gated to the face that owns the seam: Session → Return to
                // Mesh Control (the Esc-chord twin, live while a connect is pending)
                // and View → Refresh Sources (live on the Chooser). Rendered first so
                // a picked action applies this frame; the `take_return_to_chrome`
                // drain below still catches a menu-raised return like an Esc.
                let pending = self.vdi.requested_summary();
                let sources = self.chooser.source_count();
                if let Some(action) = vdi::desktop_menubar(ui, pending, sources) {
                    match action {
                        vdi::DesktopMenuAction::ReturnToChrome => {
                            self.vdi.request_return_to_chrome();
                        }
                        vdi::DesktopMenuAction::RefreshSources => self.chooser.refresh_now(),
                    }
                }
                ui.separator();
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
            Surface::InfraCode => {
                // The OpenStack IaaS control plane (IAC-2) — the Overview tab: the
                // API status band + the merged service directory, consumed off the
                // Bus (`action/cloud/get-catalog`). Scoped under its own `push_id`
                // like every mounted surface so its egui ids can't collide in the
                // shell's one `Context`.
                let infra_code = &mut self.infra_code;
                ui.push_id("shell-infra-code", |ui| {
                    iac::infra_code_panel(ui, infra_code);
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
            Surface::Media => {
                // The full media player (MEDIA-18) over the real `mde_media_core`
                // backend — Sources / Library / Player / Queue. Mounted exactly like
                // Music/Voice: drive its per-frame pump, then its header + central
                // panel, scoped under its own `push_id` so its egui ids can't collide
                // in the shell's one `Context`.
                media_pump(&mut self.media);
                let media = &mut self.media;
                let media_video = &mut self.media_video;
                ui.push_id("shell-media", |ui| {
                    media_header(ui, media);
                    ui.separator();
                    media_panel(ui, media, media_video);
                });
                // Keep the frame loop ticking while playing so the core's live clock
                // advances (the standalone MediaApp requests the same in its update).
                if self.media.is_playing() {
                    ui.ctx().request_repaint();
                }
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
                    voice_menubar(ui, voice);
                    ui.separator();
                    voice_panel(ui, voice);
                });
            }
            Surface::Browser => {
                // The sandboxed Servo browser (BOOKMARKS-6) — the `mde-web-preview`
                // helper driven over IPC and displayed by uploading its shm frames
                // to an egui texture. Scoped under its own `push_id` like every
                // mounted surface. The panel polls + drives its own tabs.
                //
                // `live-helper`: on first open, spawn the sandboxed helper as a live
                // tab, honest-gated to a usable seat (a real seat has been probed) +
                // an installed helper binary — else a NAMED honest notice, never a
                // fake page (§7). The default build keeps no live tab and shows the
                // gated EmptyState.
                #[cfg(feature = "live-helper")]
                {
                    let seat_present = self.system.snapshot().is_some();
                    self.web.ensure_live_tab(seat_present);
                }
                let web = &mut self.web;
                ui.push_id("shell-web", |ui| {
                    web::web_panel(ui, web);
                });
                if self.web.take_bookmarks_manager_request() {
                    self.nav.surface = Surface::Bookmarks;
                }
                // First-class tabs: the Browser panel owns the visible `+` button;
                // the live-helper shell arm owns the real helper spawn.
                #[cfg(feature = "live-helper")]
                {
                    let seat_present = self.system.snapshot().is_some();
                    self.web.drain_live_tab_requests(seat_present);
                }
                // Respawn-on-reload: a crashed tab's Reload asked to restart. Under
                // `live-helper` the shell swaps in a fresh live session; the default
                // build drains the flag honestly (no live tab exists, so it is inert
                // — never a faked page, §7).
                let restart_requested = self.web.take_respawn_request();
                #[cfg(feature = "live-helper")]
                if restart_requested {
                    self.web.respawn_live();
                }
                #[cfg(not(feature = "live-helper"))]
                let _ = restart_requested;
            }
            Surface::Bookmarks => {
                let bookmarks = &mut self.bookmarks;
                self.bookmarks_bus.pump(bookmarks);
                ui.push_id("shell-bookmarks", |ui| {
                    bookmarks_panel(ui, bookmarks);
                });
            }
            Surface::Terminal => {
                // The Terminator-class terminal (TERM-16) over a real local PTY —
                // tabs / splits / broadcast / a shell on any mesh peer. Mounted
                // exactly like Media: drive its per-frame pump (which lands the
                // bundled ligature face + drains the chord keymap BEFORE the panes
                // read input), then its panel, scoped under its own `push_id` so its
                // egui ids can't collide in the shell's one `Context`. The pane
                // widget heartbeats its own repaints while live, so the shell adds
                // none.
                terminal_pump(&mut self.terminal, ui.ctx());
                let terminal = &mut self.terminal;
                ui.push_id("shell-terminal", |ui| {
                    terminal_panel(ui, terminal);
                });
            }
            Surface::Editor => {
                // The native Zed-style code editor (EDITOR-1). Mounted exactly
                // like Files: the shell renders its `EditorSurface` through
                // `editor_panel`, scoped under its own `push_id` so its egui ids
                // can't collide in the shell's one `Context`. EDITOR-1 is the
                // scaffold — the editor chrome + the honest "No file open" empty
                // state (§7); the rope buffer + text widget land in EDITOR-2/3 and
                // render here without re-wiring this mount.
                let editor = &mut self.editor;
                ui.push_id("shell-editor", |ui| {
                    editor_panel(ui, editor);
                });
            }
            Surface::Chat => {
                let chat = &mut self.chat;
                ui.push_id("shell-chat", |ui| {
                    chat.show(ui);
                });
            }
            Surface::Phones => {
                // The Phones hub (KDC-MESH-9) — the desktop-side management surface
                // for the mesh's paired phone(s). A thin client of the `kdc_host`
                // worker (renders its published state + drives its Bus verbs, §6); its
                // poll is driven in `render` while in view. Scoped under its own
                // `push_id` like every mounted surface.
                let phones = &mut self.phones_hub;
                ui.push_id("shell-phones", |ui| phones.show(ui));
            }
            Surface::System => {
                // This seat's host controls, folded from the one `mde-seat` Seat
                // (E12-15). Under SETTINGS-1 the surface is a master-detail shell —
                // `system.show` draws the left domain-group rail + the wide detail
                // pane, routing to each existing section body verbatim (§6) and
                // persisting the rail selection itself. Scoped under its own
                // `push_id` like every mounted surface so its egui ids can't collide
                // in the shell's one `Context`. The snapshot is refreshed in
                // `render` (it also feeds dock status), so the panel
                // only renders here. The System panel drives Displays + Power live
                // (E12-18).
                let system = &mut self.system;
                ui.push_id("shell-system", |ui| {
                    system.show(ui);
                });
            }
            Surface::Storage => {
                // GParted disk/partition management (E12-21) — scoped under its own
                // `push_id` like every surface; the storage worker owns the walls.
                let storage = &mut self.storage;
                ui.push_id("shell-storage", |ui| storage.show(ui));
            }
            Surface::About => {
                // The About surface body is the Device-Manager hardware inspector
                // (DEVMGR-2, design docs/design/about-device-manager.md): a compact
                // brand title strip + an ⓘ dialog (the platform-identity screen,
                // QBRAND-6) over the faithful by-type device tree + rich header card
                // + menu/toolbar chrome, read from THIS node's published inventory.
                // Scoped under its own `push_id` like every mounted surface so its
                // egui ids can't collide in the shell's one `Context`.
                let dm = &mut self.device_manager;
                ui.push_id("shell-about", |ui| dm.show(ui));
            }
            Surface::Timers => {
                // Timers & Alarms (VDOCK-5) — a pure renderer over the
                // shell-owned store `render` ticks every frame, so a countdown
                // never depends on this panel being open (the design's "Timers
                // reliability" lock). Opened by the dock's clock-glyph strip
                // (lock #20); scoped under its own `push_id` like every mounted
                // surface.
                let timers = &mut self.timers;
                ui.push_id("shell-timers", |ui| {
                    timers::timers_panel(ui, timers);
                });
            }
        }
    }
}

/// The boot driver both runners share (QBRAND-4): the branded splash owns the
/// screen while the shell's real init milestones land, then the built shell
/// renders every frame. One driver, so the DRM seat and the windowed fallback
/// boot identically — splash, milestones, first dock frame.
#[derive(Default)]
struct Boot {
    /// The boot-splash: the official artwork + the banked init milestones.
    splash: splash::Splash,
    /// The shell, built once mid-boot (the `Surfaces` milestone).
    shell: Option<Shell>,
}

impl Boot {
    /// Drive one frame. While the splash owns the screen it paints FIRST — so
    /// the frame on display while a slow init step runs shows the progress
    /// already banked — then exactly one real milestone advances; the next
    /// frame's bar shows it land. Once the splash dismisses (init complete +
    /// the eased bar settled), the shell renders — the first dock frame
    /// replaces the splash.
    fn frame(&mut self, ctx: &egui::Context) {
        if !self.splash.dismissed() {
            self.splash.show(ctx);
            if !self.splash.is_complete(splash::Milestone::Seat) {
                // This callback running at all proves the seat came up — the
                // runner (DRM/KMS + wgpu, or the windowed client) finishes
                // that init before it can call back.
                self.splash.complete(splash::Milestone::Seat);
            } else if self.shell.is_none() {
                // Surface construction — every backend the shell owns (music
                // worker, media core, files browser, voice SIP agent, the
                // terminal's real PTY, …) built once.
                self.shell = Some(Shell::new_for_ctx(ctx));
                self.splash.complete(splash::Milestone::Surfaces);
            } else if !self.splash.is_complete(splash::Milestone::MeshSnapshot) {
                // The shell's FIRST mesh-status snapshot poll — the same
                // world-readable fold the dock grade/status chrome renders on its
                // cadence, so the first dock frame opens with live status dots
                // instead of cold dim ones whenever a snapshot exists.
                if let Some(shell) = self.shell.as_mut() {
                    shell.chrome.poll(ctx);
                }
                self.splash.complete(splash::Milestone::MeshSnapshot);
            }
            // Keep boot frames flowing while the eased bar plays out.
            ctx.request_repaint();
            return;
        }
        self.shell
            .get_or_insert_with(|| Shell::new_for_ctx(ctx))
            .render(ctx);
    }
}

impl eframe::App for Boot {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
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
            // The Spawn Lighthouse flow (OW-7) self-gates on `pending` too — free
            // unless a spawn request is awaiting the worker's answer.
            self.spawn_lighthouse.poll(ctx);
        }

        // OW-10 — the onboard self-test watch: an all-green verdict landing on the
        // mesh Bus auto-opens the live Mesh Map, through the SAME
        // `shell/goto/<surface>` nav grammar the KIRON chyron uses (no second
        // navigation path). The watch self-gates on the shared cadence; the Mesh
        // Map is independently reachable from the dock besides.
        self.self_test.poll(ctx);
        if self.self_test.take_all_green() {
            if let Some(nav) = toast_bridge::resolve_action("shell/goto/mesh-map") {
                self.apply_nav(nav);
            }
        }

        // The Mesh Map surface (+ its EXPLORER-3 Explorer lens) refolds while in view.
        if self.nav.expanded && self.nav.surface == Surface::MeshView {
            self.poll_mesh_map(ctx);
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
        // switches to it, and drives the dock Chat quad's unread badge (no
        // cold-start wait). This subsumes the retired Notifications + Clipboard
        // polls (NOTIFY-CHAT-6).
        if self.nav.expanded {
            self.chat.poll(ctx);
            // EDITOR-9 — pick up a Files "Send-to-Editor" (or any node's
            // `action/editor/open`): open the requested path in the Editor surface
            // and bring it to the front. `take` self-throttles + edge-triggers, so a
            // per-frame call is cheap; a read failure (a vanished file) is dropped
            // (the editor keeps its current state — never a faked open, §7).
            if let Some(path) = self.editor_launch.take() {
                if self.editor.open_path(&path).is_ok() {
                    self.nav.surface = Surface::Editor;
                }
            }
        }

        // The Storage surface tails the `state/storage/*` mirrors + the selected
        // peer's progress lane while it's in view — a cheap local scan so a UDisks2
        // change on any peer surfaces without operator input (E12-21).
        if self.nav.expanded && self.nav.surface == Surface::Storage {
            self.storage.poll(ctx);
        }

        // The Infra as Code surface polls the OpenStack catalog off the Bus
        // (`action/cloud/get-catalog`) while it's in view — a non-blocking
        // request/reply on its ~15 s cadence; the request is published sync and its
        // reply read on a later tick, so the frame never stalls (IAC-2).
        if self.nav.expanded && self.nav.surface == Surface::InfraCode {
            self.infra_code.poll(ctx);
        }

        // The About → Device-Manager surface re-reads THIS node's published
        // hardware inventory on its cadence while in view (DEVMGR-2) — a cheap
        // local read of the replicated `device-inventory/<host>.json`, honest
        // pre-poll dim until the `hardware_probe` worker's file lands (§7).
        if self.nav.expanded && self.nav.surface == Surface::About {
            self.device_manager.poll(ctx);
        }

        // The Phones hub tails the live KDE Connect roster (`action/connect/devices`)
        // + the mesh service directory (the replicated `kdc-services/*.json`) while
        // in view (KDC-MESH-9) — a non-blocking Bus RPC + a cheap local scan on the
        // shared cadence; the verb replies land on a later tick (§7).
        if self.nav.expanded && self.nav.surface == Surface::Phones {
            self.phones_hub.poll(ctx);
        }

        // The seat snapshot feeds BOTH the System surface and the dock's status
        // quads' always-visible status cells, so poll it every frame (self-gating on
        // the shared cadence) — the quads' BT / Volume / Battery cells stay live even
        // while the System surface isn't the one in view.
        self.system.poll(ctx);

        // POWER-5 — the DRM-native idle + lid honorer: one tick per frame folds this
        // frame's input + the seat's lid reading into the idle / lid-close decision and
        // drives it through the ONE seat (a self-contained block so an EDITOR-9 merge
        // stays trivial). Safe by default — idle action is off until the operator arms
        // a timeout in the Power section.
        // CURTAIN-3: an idle/lid action of **Lock** is reported back (not routed to
        // logind) so it drops the in-process curtain here, exactly like Super+L.
        if self.power_honor.tick(ctx, &self.system) {
            self.curtain.lock();
        }

        // VDOCK-5 — the Timers & Alarms tick (the power_honor idiom): one call
        // per frame evaluates the shell-owned countdown timers + daily alarms
        // and fires each due one onto the CHAT-FIX-2 `event/notify/timer` lane —
        // surface open or closed. It self-schedules the next wakeup, so a due
        // alarm rings without input even on the idle DRM seat.
        self.timers.tick(ctx);

        // CURTAIN-3 — logind session Lock/Unlock: drain the listener's forwarded
        // signals and route them (`loginctl lock-session` drops the curtain; an
        // Unlock is received but never bypasses PAM — design lock 1). Empty when no
        // system bus / logind, so this self-gates to a real seat, honestly.
        let lock_signals = self.lock_signal.poll();
        lock_signal::apply_lock_signals(&lock_signals, &mut self.curtain);

        // E12-17 — the BlueZ pairing agent is live only while the System surface is
        // in view: register on entry (once an adapter is present), drop
        // (unregister) on leave. So a pairing PIN/passkey prompt is answered by the
        // panel's modal, and no default agent lingers on the system bus otherwise.
        self.system
            .sync_pairing_agent(self.nav.expanded && self.nav.surface == Surface::System);

        // The top chrome strip is retired; its snapshot poll survives as the dock
        // dock grade/status mesh fold. ONE self-gating poll per frame (it also keeps the
        // repaint heartbeat alive for the quad status dots) — the quads read the
        // product, no second poll.
        self.chrome.poll(ctx);

        // The shell's dock chrome (VDOCK), mounted BEFORE the central view so it
        // frames the session + shell body: the left vertical dock, the sole chrome.
        // Extracted to a helper so `render` stays within the line budget.
        self.mount_dock_chrome(ctx);

        // WIN7-2 — the Start Menu: the panel + its toggle/request drains, split
        // to a helper (the mount_dock_chrome idiom) so `render` stays within
        // the line budget. Its Super-tap trigger drains later, alongside
        // VDOCK-1's own dock toggle (see below).
        self.mount_start_menu(ctx);

        // The central view: the session↔body cross-fade — or nothing at all
        // while the settled curtain fully covers the seat (CURTAIN-1, lock 10).
        self.central_view(ctx);

        // QC-13 — Cloud row → Desktop SPICE handoff. The Cloud plane lives inside
        // Workbench and parks its state in egui memory; after Workbench renders, a
        // dialable Nova console descriptor can queue one native VDI attach here.
        if let Some(request) = workbench::cloud_plane::take_console_attach(ctx) {
            self.vdi.request_connect(request);
            self.nav.expanded = true;
            self.nav.surface = Surface::Desktop;
        }

        // Route the System surface's own control-error alerts (a refused / absent
        // Bluetooth write, a pairing-agent registration failure — §7) into the ONE
        // ToastBridge, applying the same suppression + sound policy as a Bus alert.
        // Drained here, after the surface's render borrow has ended.
        for toast in self.system.take_toasts() {
            self.toasts.raise(toast);
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
            let density = Density::for_formfactor(formfactor);
            Style::install_with_density(ctx, density);
            // NAVBAR-8: the bottom rail consumes the same shell density instead of
            // growing its own compact/expanded toggle.
            self.vdock.set_density(density);
        }

        // SURFACE-11 (lock 16): a swipe from the left/bottom edge reveals the shell body
        // (the dock / tablet bar). Drained from the seat's gesture side channel; empty
        // on the windowed fallback, so the reveal self-gates to the real DRM seat.
        for edge in mde_egui::drain_edge_swipes() {
            // CURTAIN-1 (lock 10): the drain always runs (the side channel must
            // not back up), but a swipe acts on the nav only past the curtain.
            if matches!(edge, mde_egui::Edge::Left | mde_egui::Edge::Bottom)
                && !self.curtain.engaged()
            {
                self.nav.expanded = true;
            }
        }

        let host_keys = mde_egui::hostkeys::drain_host_keys();
        let presses = ctx.input(|i| hotkeys::egui_key_presses(&i.events));
        for action in self.hotkeys.dispatch(&host_keys, &presses) {
            // CURTAIN-1 (lock 10): while the curtain is engaged NO chord acts on
            // the seat or the nav. The dispatch itself still runs so the router's
            // leader latch tracks Super press/release across the lock; every
            // matched action is swallowed until the curtain lifts.
            if !self.curtain.engaged() {
                self.apply_hotkey(action);
            }
        }
        if let Some(slot) = self.hotkeys.take_nav_slot() {
            if !self.curtain.engaged() {
                self.apply_nav_slot(slot);
            }
        }
        // VDOCK-1 (lock 13) — a clean Super *tap* (press+release with no leader
        // chord used in between) toggles the vertical dock. Always DRAINED so the
        // router's latch never backs up; but, like every chord above, swallowed
        // while the curtain is engaged (lock 10).
        //
        // WIN7-2 (win7-desktop-survey lock #13) reuses this SAME drain for the
        // Start Menu: the survey never anticipated VDOCK-1's pre-existing claim
        // on Super, and stranding the vertical dock behind an unreachable
        // hotkey would break the shell's only surface launcher until WIN7-3
        // lands real tiles in the Start Menu. So one clean Super tap now
        // reveals BOTH — not a UX collision in practice, since the Start Menu
        // already mounts immediately beside the dock column (`x = DOCK_W`), so
        // the two read as one "reveal the nav chrome" gesture. A flagged
        // judgment call (see `start_menu`'s module doc), not a surveyed answer.
        if self.hotkeys.take_dock_toggle() && !self.curtain.engaged() {
            self.vdock.toggle();
            self.start_menu.toggle();
        }

        // The KIRON alert/OSD bridge (KIRON-2) — driven late so its centered OSD
        // pill floats (Foreground order) above the chrome, the surface, and any
        // fullscreen guest. Refresh the suppression posture (lock 10) first: a
        // fullscreen VDI guest in front is a per-session focus mute, and the seat's
        // audio-mute hushes a non-critical's sound. DND is owned by Chat's
        // notification lane and mutes ambient pushes.
        let focus_mute =
            self.nav.surface == Surface::Desktop && self.vdi.requested_target().is_some();
        let muted = self.system.snapshot().is_some_and(seat_master_muted);
        let dnd = dnd_active();
        self.toasts.set_suppression(dnd, focus_mute, muted);
        if let Some(nav) = self.toasts.drive(ctx) {
            // Legacy action navigation is retained as a safe no-op path while Chat
            // owns visible notification actions. Any target expands the shell.
            // CURTAIN-1 (lock 10): never past the lock — the curtain's layer already
            // blocks the click; this gate is the belt to that suspender.
            if !self.curtain.engaged() {
                self.apply_nav(nav);
            }
        }

        // NAVBAR-W10-6: a click on the backdrop's brand watermark routes to About,
        // guarded by the curtain like every other nav (the backdrop paint latched the
        // request; this drains it — the one-shot `take_*` idiom).
        if let Some(surface) = backdrop::take_nav_request(ctx) {
            if !self.curtain.engaged() {
                self.apply_nav(toast_bridge::Navigate::Surface(surface));
            }
        }

        // SURFACE-10 (lock 14): the on-screen keyboard overlay — drawn last (Foreground)
        // so it floats above the chrome, the active surface, and any fullscreen guest.
        // It reads the live focus + the cached formfactor and self-manages its raise /
        // dismiss; on a Laptop (or the windowed fallback) it stays inert.
        self.keyboard.show(ctx);

        // CURTAIN-1 — the lock curtain, driven absolutely last: its whole-screen
        // Foreground layer (re-raised with `move_to_top`) covers everything above,
        // chyron and OSK floats included. While engaged it consumes ALL input
        // (lock 10) — the pointer through the covering layer, the keyboard through
        // its per-frame focus steal plus the hotkey / edge-swipe / central-view
        // gates above. An early no-op while Unlocked.
        self.curtain.show(
            ctx,
            &mut self.media,
            self.system.snapshot(),
            self.chrome.summary(),
        );

        // NOTIF-6 — no-text critical edge cue. Drawn after the curtain so an
        // own-seat critical can still light the edges with the dock hidden/covered;
        // the cue only acknowledges itself and never routes past the lock.
        self.critical_edge.update(
            self.notify_status.segments(),
            &self.local_host,
            dnd || focus_mute,
        );
        self.critical_edge.show(ctx);

        // WIN7-6 (win7-desktop-survey lock #9) — a Critical firing (or an
        // existing one unmuting into view) auto-closes the Start Menu if it's
        // open, so the cue gets a clear field: a deliberate STRENGTHENING of
        // the cue's own "always wins" posture above, not a weakening of
        // anything WIN7-2 built. Edge-triggered off `take_became_visible` — a
        // one-shot hidden->visible latch, NOT a per-frame "is it visible
        // right now" poll — so this closes an open Start Menu exactly once
        // per real firing and never re-fights an operator who reopens the
        // Start Menu afterward: not while the SAME critical is still up but
        // acknowledged (`visible()` stays false, so no further edge fires),
        // and not while it's still up and un-acknowledged either (no NEW
        // edge without a real change). `StartMenuState::close` already
        // no-ops while closed, so this is safe to call unconditionally, and
        // closing it also closes the embedded Console pane for free — its
        // own `open` bit is a same-frame mirror of the Start Menu's
        // (`start_menu.rs`'s `console.set_open(state.open)`), not a second
        // latch this call would need to touch separately.
        if self.critical_edge.take_became_visible() {
            self.start_menu.close();
        }
    }

    /// Mount the shell's **dock chrome** for this frame (VDOCK) — the left
    /// **vertical dock** (`dock::dock`), the shell's sole chrome: a floating,
    /// slide-in, auto-hide `Area` that reserves NO gutter of its own (the central
    /// view fills the full width AND height; `central_view` insets an empty gutter
    /// in lockstep with the slide so the dock never overlaps the surface). A routed
    /// click surfaces the shell body (the dock IS the nav — a navigation is never a
    /// no-op behind the session). Split out of `render` so each stays within the
    /// line budget.
    fn mount_dock_chrome(&mut self, ctx: &egui::Context) {
        // The dock owns its own picker `active`; the shell keeps `nav.surface` as
        // the ONE source of truth every other nav path (hotkeys, chyron, self-test,
        // chooser) writes. So MIRROR the live surface INTO the dock before `dock()`
        // (the picker then highlights whatever is showing), feed the bottom status
        // status strip its live inputs, then read the picker's selection
        // straight back OUT so a picker-cell click routes the body.
        self.vdock.set_active(self.nav.surface);
        self.vdock
            .set_transfer_active_count(self.files.transfers_counts().active);
        // WIN7-2 — mirror the Start Menu's open state in first, so the Start
        // cell's active tint follows the real panel (the set_active idiom).
        self.vdock.set_start_menu_open(self.start_menu.is_open());
        self.notify_status.poll(ctx);
        let mut rail_sessions = self.session_rail.entries(&self.local_host);
        let has_visible_desktop_session = !rail_sessions.is_empty();
        if rail_sessions.is_empty() {
            rail_sessions = self
                .vdi
                .requested_summary()
                .map(|(name, protocol)| vec![dock::SessionRailEntry::new(name, protocol)])
                .unwrap_or_default();
        }
        self.vdock.set_status_inputs(
            self.chrome.summary().clone(),
            self.system.snapshot().cloned(),
            self.chat.total_unread(),
            self.vdi.requested_target().is_some(),
            rail_sessions,
            self.chrome.grades().clone(),
            self.notify_status.segments().clone(),
        );
        let desktop_sources = self.chooser.rail_sources();
        let bar_clicked = dock::dock(ctx, &mut self.vdock)
            | dock::notification_rail_with_sources(ctx, &mut self.vdock, &desktop_sources);
        self.nav.surface = self.vdock.active();
        if let Some(id) = self.vdock.take_desktop_source_pick() {
            if let Some(request) = self.chooser.connect_source_id(&id) {
                self.vdi.request_connect(request);
            }
            self.nav.surface = Surface::Desktop;
        }
        if let Some(id) = self.vdock.take_desktop_session_focus() {
            let _ = self.session_rail.focus_session(&id);
            self.nav.surface = Surface::Desktop;
        }
        if self.vdock.take_desktop_reconnect() {
            if desktop_reconnect_should_query_recents(has_visible_desktop_session) {
                if let Some(request) = self.chooser.connect_last_recent() {
                    self.vdi.request_connect(request);
                }
            }
            self.nav.surface = Surface::Desktop;
        }
        // VDOCK-4 — drain the system-quad's pending request: Lock drops the
        // in-process curtain (exactly like Super+L), a Power verb drives the seat
        // honorer (its typed-armed consent is the operator's; a refusal is an
        // honest no-op, §7).
        match self.vdock.take_request() {
            Some(dock::DockRequest::Lock) => self.curtain.lock(),
            Some(dock::DockRequest::Power(verb)) => {
                let _ = self.system.honor_power(verb);
            }
            None => {}
        }
        // NODE-GRADE-2 (#7) — a tapped grade row asks to open that node's Explorer
        // hero. The dock can't reach the Explorer/nav (§6), so drain its request
        // here: route to the Mesh Map, flip on its Explorer lens, and focus the peer
        // (the reused EXPLORER jump path). The tap itself expanded the shell below.
        if let Some(host) = self.vdock.take_node_focus() {
            self.nav.surface = Surface::MeshView;
            ctx.data_mut(|d| d.insert_temp(egui::Id::new(explorer::LENS_KEY), true));
            self.explorer.focus_node(&host);
        }
        if bar_clicked {
            self.nav.expanded = true;
        }
    }

    /// WIN7-2 — mount the **Start Menu** for this frame: drain the dock Start
    /// cell's toggle latch (ALWAYS drained so it never backs up — the
    /// Super-tap idiom), mount the panel (the fixed-size floating slide-up
    /// beside the dock, `start_menu::start_menu_panel`, which embeds the
    /// Console front door as its right pane), and drive Console's typed
    /// requests exactly as before (the panel is a presentation change, lock
    /// #10 says the underlying requests/data don't move). Everything but the
    /// drain sits behind the curtain gate (CURTAIN-1 lock 10): the panel reads
    /// raw Esc/arrow/Enter presses, which must never act past the lock — the
    /// hotkey-gate posture. Split out of `render` (the `mount_dock_chrome`
    /// idiom) so each stays within the line budget.
    ///
    /// WIN7-3 — also drains a live-tile click (the left pane) after Console's
    /// own requests: the SAME "route + expand the body" outcome as a Console
    /// `Goto`, just raised from `StartMenuState::take_tile_activation`
    /// instead.
    fn mount_start_menu(&mut self, ctx: &egui::Context) {
        let toggled = self.vdock.take_start_menu_toggle();
        if self.curtain.engaged() {
            return;
        }
        if toggled {
            self.start_menu.toggle();
        }
        // WIN7-4 — refresh the live-tile fact inputs before the panel renders
        // (the `set_status_inputs`/`mount_dock_chrome` idiom): every field
        // below reads the SAME already-published accessor an existing dock
        // pip / the surface's own status chip already reads (§7 — see
        // `TileFactInputs`'s own field docs for each exact source), cloned/
        // copied out now so `start_menu::start_menu_panel` needs no extra
        // parameters of its own.
        let media_loaded = self.media.player().media().is_some();
        self.start_menu.set_tile_inputs(start_menu::TileFactInputs {
            chat_unread: self.chat.total_unread(),
            chat_recent_sender: self.chat.most_recent_sender().map(str::to_owned),
            mesh: self.chrome.summary().clone(),
            segments: self.notify_status.segments().clone(),
            media_title: media_loaded
                .then(|| mde_media_egui::model::now_playing_title(self.media.player())),
            media_playing: media_loaded && self.media.is_playing(),
            music_now_playing: self
                .music
                .now_playing()
                .map(|song| (song.title.clone(), song.artist.clone())),
            voice_call_label: {
                let label = self.voice.call_state().label();
                (!label.is_empty()).then_some(label)
            },
            files_active_transfers: self.files.transfers_counts().active,
            storage_local: self.storage.local_summary(),
            bookmarks_total: self.bookmarks.total(),
            phones: self.phones_hub.device_counts(),
            workbench_seen: self.controller.seen(),
            workbench_peer_count: self.controller.peer_count(),
            workbench_leader: self.controller.leader().map(str::to_owned),
            desktop_sources: self.chooser.source_count(),
            desktop_session: self
                .vdi
                .requested_summary()
                .map(|(name, protocol)| (name.to_owned(), protocol)),
            infra_services: self.infra_code.service_summary(),
            browser_tabs: self.web.tab_count(),
            terminal_tabs: self.terminal.tab_count(),
        });
        // WIN7-DESKTOP-1 regression fix — reserve the SAME live taskbar height
        // `mount_dock_chrome` just rendered the rail at, so the Start Menu's
        // Power-anchored bottom sits flush above the taskbar rather than under
        // it (see `start_menu::start_menu_panel`'s own doc comment).
        start_menu::start_menu_panel(
            ctx,
            &mut self.start_menu,
            &mut self.console,
            self.vdock.rail_height(),
        );
        match self.console.take_request() {
            Some(console::ConsoleRequest::Goto(surface)) => {
                // A live surface-link entry (the pinned Terminal, the Cloud-plane
                // link) routes the shell body — a navigation is never a no-op
                // behind the session.
                self.nav.expanded = true;
                self.nav.surface = surface;
            }
            Some(console::ConsoleRequest::Plane(plane)) => {
                self.apply_nav(toast_bridge::Navigate::Plane(plane));
            }
            // CONSOLE-5 — the front door opens a real tab: a command / Custom
            // entry switches the body to the Terminal surface (lock #7) and
            // drives the now-landed spawn-tab seam over the shell's live
            // `TerminalSurface`. Root ops arrive already `sudo`-wrapped (the
            // console's `launch_argv`); a refused spawn is the surface's own
            // honest error chip (§7) — never a fabricated tab.
            Some(console::ConsoleRequest::SpawnTab { name, argv }) => {
                self.nav.expanded = true;
                self.nav.surface = Surface::Terminal;
                let _ = self.terminal.spawn_tab(name, &argv);
            }
            // CONSOLE-4 — the rail Power section: Lock drops the in-process
            // curtain (exactly like Super+L); a Power verb drives the seat
            // honorer (the typed-armed consent is the operator's; a refusal is
            // an honest no-op, §7). The same seams the VDOCK-4 drain drives —
            // never a raw `systemctl` (§6).
            Some(console::ConsoleRequest::Lock) => self.curtain.lock(),
            Some(console::ConsoleRequest::Power(verb)) => {
                let _ = self.system.honor_power(verb);
            }
            None => {}
        }
        // WIN7-3 — a live-tile click (the left pane) ends in the SAME
        // outcome as an embedded Console `Goto` above: route the body and
        // expand it (a navigation raised from the Start Menu is never a
        // no-op behind the session, matching every other nav path here).
        if let Some(surface) = self.start_menu.take_tile_activation() {
            self.nav.expanded = true;
            self.nav.surface = surface;
        }
    }

    /// The central view: the session↔body cross-fade through the expand
    /// transition. While the settled curtain fully covers the seat (CURTAIN-1,
    /// lock 10) it mounts NOTHING — an opaque sheet hides it anyway, and
    /// surfaces beneath must not run their raw input reads (the VDI guest
    /// forward drains `ui.input` directly, past focus and layer hit-tests).
    /// The curtain's drop/lift tweens still render the view beneath the
    /// sliding sheet.
    fn central_view(&mut self, ctx: &egui::Context) {
        // Expand transition: 0.0 = collapsed (session), 1.0 = expanded (the
        // active surface). The floating dock rides outside the fade.
        let t = Motion::animate(ctx, "shell-expand", self.nav.expanded, Motion::BASE);

        // DOCK-OVERLAP — when the dock is shown and we are NOT in a
        // full-screen remote desktop, reserve a left gutter equal to the dock's
        // live eased slide width so the central content is NOT covered by the dock
        // (it insets in lockstep with the slide, no overlap). In a full-screen
        // remote desktop the dock instead floats as an overlay (it reveals OVER the
        // edge-to-edge remote), so NO gutter is reserved; hidden → no gutter (full
        // width). Mounted as an empty left `SidePanel` BEFORE the `CentralPanel`,
        // which the floating dock `Area` paints over. Reuses the EXACT
        // full-screen-remote condition the KIRON focus-mute uses (`render`).
        let full_screen_remote_desktop =
            self.nav.surface == Surface::Desktop && self.vdi.requested_target().is_some();
        let gutter = reserved_dock_gutter(full_screen_remote_desktop, ctx, &self.vdock);
        if gutter > 0.0 {
            egui::SidePanel::left("shell-dock-gutter")
                .exact_width(gutter)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE)
                .show(ctx, |_ui| {});
        }

        let covered = self.curtain.covers_fully();
        egui::CentralPanel::default().show(ctx, |ui| {
            if covered {
                return;
            }
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
    }
}

/// DOCK-OVERLAP — the width of the left gutter the shell reserves for the vertical
/// dock this frame so the central content is never covered by it. It is the dock's
/// live eased slide width ([`dock::gutter_width`], `0.0` when hidden + settled) —
/// but reserved ONLY when we are NOT in a full-screen remote desktop. In a
/// full-screen remote desktop the dock floats as an overlay (it reveals OVER the
/// edge-to-edge remote), so nothing is reserved. Split out (and pure but for the
/// dock's slide read) so the gate is unit-testable.
fn reserved_dock_gutter(
    full_screen_remote_desktop: bool,
    ctx: &egui::Context,
    vdock: &dock::DockState,
) -> f32 {
    if full_screen_remote_desktop {
        0.0
    } else {
        dock::gutter_width(ctx, vdock)
    }
}

/// The seat's master-output mute, if the mixer probe answered — gates a
/// non-critical chyron's notification sound (KIRON lock 8). No mixer backend reads
/// as *not* muted (an absent probe never silences an alert).
fn seat_master_muted(snap: &SeatSnapshot) -> bool {
    matches!(&snap.mixer, Probe::Present(status) if status.master.muted)
}

fn dnd_active() -> bool {
    mde_bus::client_data_dir().is_some_and(|root| mde_bus::dnd::load_default(&root).active)
}

const fn desktop_reconnect_should_query_recents(has_visible_desktop_session: bool) -> bool {
    !has_visible_desktop_session
}

fn local_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "local".to_string())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // QBRAND-1 — `--version` prints the single baked build-identity line (version
    // · git hash · date · channel), shared verbatim with `mackesd --version` and
    // the About panel. Handled before standing up the seat so it works headless.
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        println!("{}", mde_theme::brand::build::full());
        return Ok(());
    }

    // E12-3 — the shell OWNS the DRM/KMS seat directly (no compositor, no display
    // manager) when built `--features drm` and a seat is available. It falls back to
    // the windowed eframe client only when there is no DRM master (a dev host, or a
    // compositor already holds the seat) — the exact fallback E12-2 designed in.
    #[cfg(feature = "drm")]
    {
        // QBRAND-4 — the branded boot-splash owns the seat until every real
        // init milestone lands and the artwork's bar fills; the first dock
        // frame then replaces it. `Boot::frame` drives the whole sequence.
        let mut boot = Boot::default();
        match mde_egui::run_drm("org.magicmesh.Shell", |ctx| boot.frame(ctx)) {
            Ok(()) => return Ok(()),
            Err(mde_egui::drm::DrmError::NoDrmMaster(why)) => {
                eprintln!(
                    "mde-shell-egui: no DRM seat ({why}); falling back to the windowed client"
                );
            }
            Err(e) => return Err(Box::new(e)),
        }
    }
    // The windowed fallback boots through the SAME driver — splash, milestones,
    // then the shell (built mid-boot from the window's egui context).
    run_client("org.magicmesh.Shell", "MCNF", |_cc| Boot::default()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{
        chat, console, desktop_reconnect_should_query_recents, dock, editor_panel, files_panel,
        media_header, media_panel, real_editor, real_media, real_terminal, reserved_dock_gutter,
        screenshot, splash, start_menu, status, terminal_panel, Boot, Nav, Plane, Shell, Surface,
        VideoTextureCache,
    };
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;
    use mde_chat::{
        AlertAction, AlertActionKind, Contact, Conversation, Message, MessageKind, NodeRole,
        Roster, Severity,
    };
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;
    use std::path::Path;

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
    fn visible_desktop_sessions_focus_instead_of_reconnecting_recents() {
        assert!(
            !desktop_reconnect_should_query_recents(true),
            "a broker-visible desktop session should be focused by the Desktop cell"
        );
        assert!(
            desktop_reconnect_should_query_recents(false),
            "without a visible session the Desktop cell should fall back to last recent"
        );
    }

    // ── DOCK-OVERLAP: the vertical dock reserves a gutter so it never overlaps ──

    #[test]
    fn the_dock_reserves_a_gutter_except_in_a_full_screen_remote_desktop() {
        // A shown vertical dock, NOT in a full-screen remote desktop → the shell
        // reserves the dock's slide width (DOCK_W once settled) as a left gutter so
        // the dock never overlaps the surface. A fresh context reports the settled
        // slide endpoint on first sight (egui's `animate_bool`), so the reserved
        // width is DOCK_W. Each case uses its own context so the slide latch starts
        // fresh at the right endpoint.
        let ctx = egui::Context::default();
        let mut shown = dock::DockState::default();
        shown.toggle();
        assert!(
            (reserved_dock_gutter(false, &ctx, &shown) - dock::DOCK_W).abs() < f32::EPSILON,
            "a shown dock off a full-screen remote reserves a DOCK_W gutter (no overlap)"
        );

        // A full-screen remote desktop → NO gutter: the dock overlays the
        // edge-to-edge remote on reveal (the remote stays full-screen).
        let ctx2 = egui::Context::default();
        let mut shown2 = dock::DockState::default();
        shown2.toggle();
        assert!(
            reserved_dock_gutter(true, &ctx2, &shown2).abs() < f32::EPSILON,
            "in a full-screen remote desktop the dock overlays — no gutter reserved"
        );

        // A hidden dock → NO gutter (content fills the full width).
        let ctx3 = egui::Context::default();
        let hidden = dock::DockState::default();
        assert!(
            reserved_dock_gutter(false, &ctx3, &hidden).abs() < f32::EPSILON,
            "a hidden dock reserves nothing — the content fills full width"
        );
    }

    #[test]
    fn a_reserved_gutter_insets_the_central_content_by_dock_w() {
        // The reservation MECHANISM (mirrors `central_view`): an empty left
        // SidePanel of the reserved width pushes the CentralPanel's content right by
        // exactly that width, so the floating dock Area over x∈[0,DOCK_W] covers only
        // the empty gutter — never the surface. The CentralPanel's own inner frame
        // margin is constant, so the DOCK_W inset shows as the DELTA between the
        // reserved and the unreserved content left.
        let with = central_left_after_gutter(dock::DOCK_W);
        let without = central_left_after_gutter(0.0);
        assert!(
            (with - without - dock::DOCK_W).abs() < 0.5,
            "a DOCK_W gutter must inset the central content by DOCK_W (with={with}, without={without})"
        );
        assert!(
            with > without,
            "reserving a gutter must push the central content strictly rightward"
        );
    }

    /// Mount an empty left gutter `SidePanel` of `gutter` (0 = none) exactly as
    /// `central_view` does, then a `CentralPanel`, and return the `CentralPanel`
    /// content rect's LEFT — the inset the reserved gutter produces.
    fn central_left_after_gutter(gutter: f32) -> f32 {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let left = std::cell::Cell::new(f32::NAN);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            if gutter > 0.0 {
                egui::SidePanel::left("shell-dock-gutter")
                    .exact_width(gutter)
                    .resizable(false)
                    .show_separator_line(false)
                    .frame(egui::Frame::NONE)
                    .show(ctx, |_ui| {});
            }
            egui::CentralPanel::default().show(ctx, |ui| left.set(ui.max_rect().left()));
        });
        left.get()
    }

    /// One headless boot frame through the SAME `Boot::frame` both runners
    /// drive (QBRAND-4): the splash paints real primitives, the `Seat`
    /// milestone banks (this frame running IS the proof the runner's init
    /// completed), and surface construction is deferred to a later frame — so
    /// the operator sees the splash *before* the heavy build, and the dock
    /// only replaces it after dismissal. (Later frames would build the full
    /// `Shell` — its worker threads (SIP agent, PTY) are the surface tests'
    /// territory, so this test stops at the first frame.)
    #[test]
    fn the_first_boot_frame_paints_the_splash_and_banks_the_seat() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut boot = Boot::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| boot.frame(ctx));
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the boot splash painted no primitives");
        assert!(
            boot.splash.is_complete(splash::Milestone::Seat),
            "the first frame must bank the Seat milestone"
        );
        assert!(
            boot.shell.is_none(),
            "surfaces must build on a later frame, behind the splash"
        );
        assert!(!boot.splash.dismissed(), "dismissed before init completed");
    }

    #[test]
    fn shell_mounts_the_critical_edge_cue_from_own_seat_rollups() {
        // NOTIF-6 integration: the shell owns the cue, feeds it the daemon segment
        // snapshot, and mounts the no-text foreground edge overlay even with the
        // dock hidden. The status module's unit tests cover pulse/ack/mute details;
        // this guards the "implemented but never mounted" failure mode.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut shell = Shell::new_for_ctx(&ctx);
        shell.local_host = "eagle".to_string();
        shell
            .notify_status
            .set_segments_for_test(status::StatusSegments {
                alerts: Some(status::SegmentRollup {
                    segment: "alerts".to_string(),
                    severity: "critical".to_string(),
                    source: "thermal".to_string(),
                    summary: "thermal critical".to_string(),
                    host: "eagle".to_string(),
                    critical_policy: "own-seat-light-show".to_string(),
                    ts_unix_ms: 42,
                }),
                seen: true,
                ..status::StatusSegments::default()
            });
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| shell.render(ctx));

        assert!(
            shell.critical_edge.visible(),
            "the own-seat critical keeps the shell edge cue visible"
        );
        assert!(
            ctx.read_response(egui::Id::new(("notif-critical-edge", 0)))
                .is_some(),
            "the shell mounted the foreground edge-cue hit region"
        );
        assert!(
            ctx.read_response(status::critical_edge_cue_id()).is_some(),
            "the edge-cue Area itself is registered"
        );
    }

    #[test]
    fn notif13_fixture_mounts_status_chat_edge_and_accesskit_together() {
        let tmp = tempfile::tempdir().unwrap();
        let bus_root = tmp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("fixture bus");

        let mut roster = Roster::new("eagle");
        roster.upsert(Contact::new("eagle", NodeRole::Workstation));
        persist
            .write(
                "state/chat/roster",
                Priority::Default,
                None,
                Some(&serde_json::to_string(&roster).unwrap()),
            )
            .unwrap();
        let mut alert_fields = std::collections::BTreeMap::new();
        alert_fields.insert("summary".to_string(), "thermal critical".to_string());
        let alert = Message::new(
            "eagle",
            42,
            MessageKind::Alert {
                severity: Severity::Critical,
                flag: "thermal".to_string(),
                fields: alert_fields,
                action_verb: None,
                actions: vec![
                    AlertAction {
                        id: "ack".to_string(),
                        label: "Ack".to_string(),
                        verb: None,
                        kind: AlertActionKind::Ack,
                    },
                    AlertAction {
                        id: "restart".to_string(),
                        label: "Restart".to_string(),
                        verb: Some("action/systemd/restart".to_string()),
                        kind: AlertActionKind::Safe,
                    },
                ],
            },
        );
        let mut conv = Conversation::new("alert:eagle");
        let alert_id = alert.id.clone();
        conv.insert(alert);
        let msgs: Vec<_> = conv.messages().iter().cloned().collect();
        persist
            .write(
                "state/chat/conversation/alert:eagle",
                Priority::Default,
                None,
                Some(&serde_json::to_string(&msgs).unwrap()),
            )
            .unwrap();

        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut shell = Shell::new_for_ctx(&ctx);
        shell.local_host = "eagle".to_string();
        shell.nav.expanded = true;
        shell.nav.surface = Surface::Chat;
        shell.vdock.toggle();
        shell.vdock.open_status_panel_for_test();
        shell.chat = chat::ChatState::with_bus_root(bus_root);
        shell.chat.select_notifications_for_test();
        shell
            .notify_status
            .set_segments_for_test(status::StatusSegments {
                device: Some(status::SegmentRollup {
                    segment: "device".to_string(),
                    severity: "warning".to_string(),
                    source: "service".to_string(),
                    summary: "sshd.service failed".to_string(),
                    host: "eagle".to_string(),
                    critical_policy: "remote-pip-chat".to_string(),
                    ts_unix_ms: 40,
                }),
                alerts: Some(status::SegmentRollup {
                    segment: "alerts".to_string(),
                    severity: "critical".to_string(),
                    source: "thermal".to_string(),
                    summary: "thermal critical".to_string(),
                    host: "eagle".to_string(),
                    critical_policy: "own-seat-light-show".to_string(),
                    ts_unix_ms: 42,
                }),
                seen: true,
                ..status::StatusSegments::default()
            });

        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| shell.render(ctx));
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "fixture shell frame painted nothing");
        assert!(
            ctx.read_response(status::segment_pip_id(status::StatusSegment::Alerts))
                .is_some(),
            "Alerts pip registered from daemon segment rollup"
        );
        assert!(
            ctx.read_response(status::status_panel_id()).is_some(),
            "status expansion panel mounted"
        );
        assert!(
            ctx.read_response(status::critical_edge_cue_id()).is_some(),
            "own-seat critical edge cue mounted"
        );
        assert!(
            shell.critical_edge.visible(),
            "own-seat critical remains live after the fixture frame"
        );
        assert!(
            shell.chat.notification_count_for_test() > 0,
            "Chat read-model folded the fixture alert"
        );
        assert!(
            ctx.read_response(chat::alert_action_button_id(alert_id.as_str(), "ack"))
                .is_some(),
            "typed Ack action button mounted"
        );
        assert!(
            ctx.read_response(chat::alert_action_button_id(alert_id.as_str(), "restart"))
                .is_some(),
            "typed safe action button mounted"
        );
        assert!(
            ctx.read_response(chat::notification_dnd_toggle_id())
                .is_some(),
            "DND toggle mounted in the Chat Notifications lane"
        );

        let nodes = out
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .iter()
            .map(|(_, node)| node)
            .collect::<Vec<_>>();
        assert!(
            nodes.iter().any(|node| {
                node.label() == Some("Notification status")
                    && node.role() == egui::accesskit::Role::Status
                    && node.live() == Some(egui::accesskit::Live::Polite)
            }),
            "status live region exported"
        );
        assert!(
            nodes.iter().any(|node| {
                node.label() == Some("Critical alert")
                    && node.role() == egui::accesskit::Role::Alert
                    && node.live() == Some(egui::accesskit::Live::Assertive)
            }),
            "critical live region exported"
        );
    }

    /// WIN7-6's own `screen_rect` — a helper so its three tests below each
    /// build a fresh `RawInput` per frame without repeating the literal
    /// (`egui::RawInput` frames are consumed by `ctx.run`, so a multi-frame
    /// test needs a new one each time).
    fn win7_6_test_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        }
    }

    /// WIN7-6's own fixture segments — an own-seat rollup at the given
    /// severity (the `shell_mounts_the_critical_edge_cue_from_own_seat_rollups`
    /// fixture pattern, generalized so the non-Critical test below can reuse
    /// it at "warning" instead of duplicating the whole literal).
    fn win7_6_own_seat_segments(severity: &str, ts_unix_ms: i64) -> status::StatusSegments {
        status::StatusSegments {
            alerts: Some(status::SegmentRollup {
                segment: "alerts".to_string(),
                severity: severity.to_string(),
                source: "thermal".to_string(),
                summary: "thermal reading".to_string(),
                host: "eagle".to_string(),
                critical_policy: "own-seat-light-show".to_string(),
                ts_unix_ms,
            }),
            seen: true,
            ..status::StatusSegments::default()
        }
    }

    #[test]
    fn win7_6_a_critical_firing_closes_an_open_start_menu() {
        // WIN7-6 (win7-desktop-survey lock #9): the NOTIF-6 edge-cue now also
        // auto-closes the Start Menu if it's open the instant a Critical
        // rollup fires, so the cue gets a clear field — a strengthening of
        // the cue's existing "always wins" posture, not a weakening of
        // anything WIN7-2 built.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut shell = Shell::new_for_ctx(&ctx);
        shell.local_host = "eagle".to_string();
        shell.start_menu.toggle();

        // Frame 1: the menu is open, no critical has fired yet.
        let _ = ctx.run(win7_6_test_input(), |ctx| shell.render(ctx));
        assert!(
            shell.start_menu.is_open(),
            "test setup: the Start Menu opens with no critical firing"
        );

        // Frame 2: an own-seat Critical fires.
        shell
            .notify_status
            .set_segments_for_test(win7_6_own_seat_segments("critical", 42));
        let _ = ctx.run(win7_6_test_input(), |ctx| shell.render(ctx));

        assert!(
            shell.critical_edge.visible(),
            "the own-seat critical keeps the shell edge cue visible"
        );
        assert!(
            !shell.start_menu.is_open(),
            "a Critical firing must auto-close an open Start Menu (lock #9)"
        );
    }

    #[test]
    fn win7_6_a_warning_alert_does_not_close_the_start_menu() {
        // The strengthening is specifically for Critical severities
        // (`is_critical_severity`) — a Warning rollup must not touch the
        // Start Menu at all, matching the edge cue's own existing severity
        // gate (it never lights up for anything less than Critical either).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut shell = Shell::new_for_ctx(&ctx);
        shell.local_host = "eagle".to_string();
        shell.start_menu.toggle();

        shell
            .notify_status
            .set_segments_for_test(win7_6_own_seat_segments("warning", 42));
        let _ = ctx.run(win7_6_test_input(), |ctx| shell.render(ctx));

        assert!(
            !shell.critical_edge.visible(),
            "a Warning severity never lights the own-seat edge cue"
        );
        assert!(
            shell.start_menu.is_open(),
            "a non-Critical rollup must not touch the Start Menu's open state"
        );
    }

    #[test]
    fn win7_6_reopening_after_acknowledging_the_critical_is_not_re_closed() {
        // WIN7-6's auto-close is edge-triggered off `take_became_visible`,
        // not a per-frame "is it visible" poll — so once the operator has
        // acknowledged the SAME critical (silencing the cue), reopening the
        // Start Menu afterward must not be immediately fought shut again.
        // This is the precise "strengthening, not an annoying loop" behavior
        // the design lock calls for.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut shell = Shell::new_for_ctx(&ctx);
        shell.local_host = "eagle".to_string();
        shell.start_menu.toggle();

        // Frame 1: menu open, no critical yet.
        let _ = ctx.run(win7_6_test_input(), |ctx| shell.render(ctx));
        assert!(shell.start_menu.is_open(), "test setup: opens clean");

        // Frame 2: the critical fires and auto-closes the menu (the exact
        // behavior `win7_6_a_critical_firing_closes_an_open_start_menu`
        // covers in detail; re-confirmed here as this test's own setup).
        shell
            .notify_status
            .set_segments_for_test(win7_6_own_seat_segments("critical", 42));
        let _ = ctx.run(win7_6_test_input(), |ctx| shell.render(ctx));
        assert!(
            !shell.start_menu.is_open(),
            "test setup: the firing auto-closed it"
        );

        // The operator acknowledges the still-live critical — the edge-click
        // path itself is `status.rs`'s own territory (see
        // `critical_edge_cue_tessellates_as_an_ambient_edge_overlay`); this
        // test drives the same `acknowledge` entry point directly.
        shell.critical_edge.acknowledge();

        // Frame 3: the operator reopens the Start Menu. The SAME critical
        // rollup is still technically active (just acknowledged/silent) —
        // reopening must not be immediately fought shut again.
        shell.start_menu.toggle();
        let _ = ctx.run(win7_6_test_input(), |ctx| shell.render(ctx));

        assert!(
            shell.start_menu.is_open(),
            "reopening after acknowledging the critical must not be re-closed \
             (WIN7-6 must never become an annoying open/slam loop)"
        );
        assert!(
            !shell.critical_edge.visible(),
            "sanity: the cue stays hidden post-ack across this frame too"
        );
    }

    /// Mount the shell's **vertical dock** chrome (VDOCK, the sole chrome) exactly
    /// as `render`'s `mount_dock_chrome` does — the floating dock `Area` mounted
    /// before the central view — so the surface-mount tests below reproduce the live
    /// chrome-then-central order. The dock is revealed so its frame actually paints;
    /// it mirrors `active` in and reads the picker selection back out.
    fn mount_dock(ctx: &egui::Context, active: &mut Surface) {
        let mut vdock = dock::DockState::default();
        vdock.toggle(); // reveal it so the dock frame paints
        vdock.set_active(*active);
        let _ = dock::dock(ctx, &mut vdock);
        *active = vdock.active();
    }

    /// Drive one headless frame that reproduces the shell's **body mount** — the
    /// vertical dock chrome, then a surface scoped under `push_id` in the
    /// shell's `CentralPanel` — then tessellate it on the CPU so any paint-path
    /// fault surfaces as a failure. This is the same `Context::run` → `tessellate`
    /// path the DRM runner drives, minus the GPU (no window, no wgpu).
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
            mount_dock(ctx, &mut active);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-files", |ui| files_panel(ui, &mut files));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted surface produced no draw primitives"
        );
    }

    /// The Media surface (MEDIA-18) mounts through the same `body` path — the dock
    /// chrome plus the media header + `media_panel` scoped under `push_id` — over the
    /// **real** `mde_media_core` backend (`real_media()`, no demo data; with no media
    /// indexed it shows the honest first-run Sources view, still a full paint path).
    /// Tessellating it on the CPU proves the whole media player is runtime-reachable
    /// as an in-shell surface and actually draws — the media analogue of
    /// [`shell_mounts_and_renders_a_surface`]. This is the RESCUE the unit is: before
    /// it, `mde-media-egui` was mounted nowhere.
    #[test]
    fn shell_mounts_and_renders_the_media_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut media = real_media();
        let mut media_video = VideoTextureCache::default();
        let mut active = Surface::Media;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            mount_dock(ctx, &mut active);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-media", |ui| {
                    media_header(ui, &mut media);
                    ui.separator();
                    media_panel(ui, &mut media, &mut media_video);
                });
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted media surface produced no draw primitives"
        );
    }

    /// L0 production-feature detection (BUG-VIDEO-1, `docs/gpu_encoder.md`): a
    /// shell built for the real DRM seat (`--features drm`, the shipped-shell
    /// configuration — see the `drm` feature doc in `Cargo.toml`) must ALSO
    /// enable the real mpv engine (`media-mpv`), or the embedded Media surface
    /// silently ships backed by `FakeMpv` — simulated playback (flips to
    /// Playing, 0:00 frozen, no A/V), the exact live-verified 2026-07-03 Eagle
    /// failure. This assertion compiles into every build so it always runs,
    /// but it is only a real constraint when `drm` is on: a normal portable dev
    /// build (`drm` off) trivially passes regardless of `media-mpv`. Prove the
    /// release combination with
    /// `xcp-build.sh cargo test -p mde-shell-egui --features drm,media-mpv`;
    /// drop `media-mpv` from that command to see this fail.
    #[test]
    #[allow(
        clippy::assertions_on_constants,
        reason = "cfg!(...) is a compile-time constant WITHIN any one build, but this \
                  must stay a runtime #[test] assert, not a `const { assert!() }` block \
                  — the whole point is that dropping media-mpv from a drm build fails \
                  `cargo test`, not that it fails to compile at all (§7 L0 gate: \"a \
                  test that fails if…\", not a hard compile error every drm-only dev \
                  build would trip)"
    )]
    fn release_shell_configuration_enables_the_real_media_engine() {
        assert!(
            !cfg!(feature = "drm") || cfg!(feature = "media-mpv"),
            "mde-shell-egui was built with --features drm (the shipped DRM-seat \
             shell) but without media-mpv — the embedded Media surface would ship \
             backed by FakeMpv (BUG-VIDEO-1, simulated playback, no real A/V). \
             Build with --features drm,media-mpv."
        );
    }

    /// The Terminal surface (TERM-16) mounts through the same `body` path — the
    /// dock chrome plus `terminal_panel` scoped under `push_id` — over a **real**
    /// local PTY (`real_terminal()`, no demo data; a refused first PTY renders the
    /// honest spawn error, still a full paint path). Tessellating it on the CPU
    /// proves the whole Terminator-class terminal is runtime-reachable as an
    /// in-shell surface and actually draws — the terminal analogue of
    /// [`shell_mounts_and_renders_the_media_surface`]. This is the RESCUE the unit
    /// is: before it, `mde-term-egui` was mounted nowhere.
    #[test]
    fn shell_mounts_and_renders_the_terminal_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut terminal = real_terminal();
        let mut active = Surface::Terminal;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            mount_dock(ctx, &mut active);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-terminal", |ui| terminal_panel(ui, &mut terminal));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted terminal surface produced no draw primitives"
        );
    }

    /// The Editor surface (EDITOR-1) mounts through the same `body` path — the dock
    /// chrome plus `editor_panel` scoped under `push_id` — over a fresh
    /// `EditorSurface` (`real_editor()`). EDITOR-1 is the scaffold, so the panel
    /// paints the editor chrome + the honest "No file open" empty state (§7, a real
    /// reachable state, not a `todo!()`). Tessellating it on the CPU proves the
    /// code-editor surface is runtime-reachable as an in-shell surface and actually
    /// draws — the editor analogue of [`shell_mounts_and_renders_the_terminal_surface`].
    #[test]
    fn shell_mounts_and_renders_the_editor_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut editor = real_editor();
        let mut active = Surface::Editor;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            mount_dock(ctx, &mut active);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-editor", |ui| editor_panel(ui, &mut editor));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted editor surface produced no draw primitives"
        );
    }

    // ── WIN7-SHOT-1: real pixels, not just layout rects / accesskit nodes ──────

    /// The screenshot capture PROOF: five WIN7 chrome units (WIN7-1..5) landed
    /// real layout changes verified only by layout-assertion + accesskit tests
    /// (like every test above this one) — every one flagged that nobody, human
    /// or test, had actually SEEN a rendered pixel of the result. This proves
    /// `screenshot::Capture`'s pixel path is real, not just "the file exists":
    /// mirrors how `mde_media_core::VideoFrame::is_blank` proved BUG-VIDEO-1's
    /// own pixel path was real this same session — a wired-but-broken raster
    /// path leaves a UNIFORM canvas; the current shell state (whatever it is —
    /// this fixture doesn't force a surface open, so a fresh boot may render
    /// the CURTAIN-3 lock curtain rather than the desktop, which is itself a
    /// real, non-blank, honest state) never does.
    #[test]
    fn win7_shot_1_screenshot_capture_renders_real_non_blank_pixels() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut shell = Shell::new_for_ctx(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };

        let canvas = screenshot::Capture::new().frame(&ctx, input, |ctx| shell.render(ctx));

        assert_eq!(
            (canvas.width(), canvas.height()),
            (1280, 800),
            "the canvas must be sized to the driven screen_rect (pixels_per_point defaults to 1.0)"
        );
        assert!(
            !canvas.is_blank(),
            "the current shell state must paint real, non-uniform pixels — a blank \
             canvas means the raster path (or the shell itself) painted nothing"
        );

        let tmp = tempfile::tempdir().expect("scratch dir for the capture proof");
        let path = tmp.path().join("current-shell-state.png");
        canvas.write_png(&path).expect("write the proof PNG");
        let written = std::fs::metadata(&path).expect("the PNG must exist on disk");
        assert!(written.len() > 0, "write_png must produce a non-empty file");
    }

    /// Paint the SAME three free functions `Shell::render` composes in
    /// production for this exact slice (`dock::dock`, `dock::
    /// notification_rail_with_sources`, `start_menu::start_menu_panel`) —
    /// bypassing `Shell`/`Curtain` entirely, exactly like `dock.rs`'s own
    /// standalone tests already drive `dock()` without a `Shell`. Going through
    /// the full `Shell` would hit the CURTAIN-3 boot gate (`Shell::new_for_ctx`
    /// starts locked under the shipped `require_login_at_boot` default), which
    /// would hide the whole nav — including the Start Menu — behind the PAM
    /// curtain on any host/CI sandbox with no persisted power-honor config.
    fn paint_taskbar_and_start_menu(
        ctx: &egui::Context,
        vdock: &mut dock::DockState,
        menu: &mut start_menu::StartMenuState,
        console: &mut console::ConsoleState,
    ) {
        let _ = dock::dock(ctx, vdock);
        let _ = dock::notification_rail_with_sources(ctx, vdock, &[]);
        // Mirrors main.rs's real `mount_start_menu` wiring (WIN7-DESKTOP-1
        // regression fix) so this fixture's Start Menu reserves the SAME live
        // taskbar height the rail above just rendered at, exactly like
        // production — load-bearing for this test's own screenshot to show
        // the accumulated chrome correctly, not just each piece in isolation.
        start_menu::start_menu_panel(ctx, menu, console, vdock.rail_height());
    }

    /// WIN7-SHOT-1's actual payoff: the FIRST real look at the accumulated
    /// WIN7-1..5 result (the bottom taskbar + the open two-pane Start Menu) —
    /// every prior WIN7 unit was verified by layout-rect/accesskit assertions
    /// alone. Writes a REAL file at a stable, reportable path (unlike the
    /// proof above's tempdir) — this PNG IS the deliverable a human opens.
    #[test]
    fn win7_shot_2_start_menu_screenshot_shows_the_accumulated_win7_chrome() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut vdock = dock::DockState::default();
        let mut menu = start_menu::StartMenuState::default();
        let mut console = console::ConsoleState::with_store(None);
        // The start_menu.rs idiom throughout this crate: toggle, then settle a
        // quiet frame before the shot (e.g.
        // `the_open_start_menu_does_not_cover_the_rest_of_the_screen`).
        menu.toggle();
        vdock.set_start_menu_open(true);

        let size = vec2(1280.0, 800.0);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), size)),
            ..Default::default()
        };

        let mut cap = screenshot::Capture::new();
        let _settle = cap.frame(&ctx, input(), |ctx| {
            paint_taskbar_and_start_menu(ctx, &mut vdock, &mut menu, &mut console);
        });
        let canvas = cap.frame(&ctx, input(), |ctx| {
            paint_taskbar_and_start_menu(ctx, &mut vdock, &mut menu, &mut console);
        });

        assert!(
            menu.is_open(),
            "the fixture must really have the Start Menu open"
        );
        assert_eq!((canvas.width(), canvas.height()), (1280, 800));
        assert!(
            !canvas.is_blank(),
            "the Start Menu screenshot must not be blank"
        );

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("screenshots")
            .join("win7-start-menu.png");
        canvas
            .write_png(&path)
            .expect("write the WIN7 Start Menu screenshot");
        println!("WIN7 Start Menu screenshot written to {}", path.display());
    }
}
