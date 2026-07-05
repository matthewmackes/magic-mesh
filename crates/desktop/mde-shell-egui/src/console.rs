//! CONSOLE — the **Terminal's Start-Menu front door** (design
//! `docs/design/console-frontdoor.md`, CONSOLE-1).
//!
//! A Carbon-styled Win10 Start menu whose entries are **operational terminal
//! ops**: the dock's Start cell (the Terminal glyph — it IS the terminal's front
//! door, lock #2) toggles this panel, a two-pane footprint that slides up beside
//! the dock (~200ms Motion, lock #44). The **left rail** is the category
//! jump-index (clicking a category jump-scrolls the list, lock #49) over the
//! `user@host · version` footer (lock #43); the **right pane** is the pinned
//! Terminal + Monitor pair (lock #31) above the grouped entry list — each row an
//! icon + label + one-line description + a subtle Fedora/Quasar provenance tag
//! (locks #33/#38). Full arrow-key nav with the EXPLORER-18 focus-ring posture
//! (locks #40/#48); Esc / click-away / pressing Start again dismiss (lock #4).
//!
//! **Vertical-dock reconciliation:** the design's "far-left Start button" came
//! from the retired horizontal taskbar. The dock is now the left VERTICAL column
//! (VDOCK), so Start maps to the dock's **topmost cell** (the vertical analog of
//! far-left, still "before Workbench", lock #2) while the panel keeps its Win10
//! **bottom-left footprint** — anchored beside the dock column at the screen
//! bottom, rising with the locked slide-up Motion (lock #3/#44).
//!
//! **Honest gates (§7):** an entry's real launch — "open a named terminal tab
//! running this command" — needs the CONSOLE-2 `spawn_tab` seam on
//! `mde-term-egui`, which has not landed (`TabbedTerminal::new_tab()` takes no
//! command today). Activating a command entry therefore raises a **typed
//! `NotWired` notice** in the panel, never a faked launch. Surface-link entries
//! (the pinned Terminal, the Containers&VMs "Cloud plane" link, lock #41) route
//! for real through the shell nav. A command whose underlying tool is absent
//! from `$PATH` renders greyed and reports the missing tool by name (the design's
//! "no dead entries" rule).
//!
//! Like the dock, this module is pure chrome + state: it records a typed
//! [`ConsoleRequest`] the shell drains after the frame (`main.rs`), and never
//! reaches the nav / curtain / seat itself (§6, the VDOCK deferred-wire idiom).

use mde_egui::egui;
use mde_egui::{Motion, Style};
use mde_theme::brand::icons::IconId;

use crate::dock::{icon_texture, Surface, DOCK_W};

// ── geometry (all §4 token math, the dock's 8px grid) ───────────────────────

/// The stable id of the console's floating [`egui::Area`] layer, so the shell
/// (and the layer tests) can name its `LayerId`.
const CONSOLE_AREA: &str = "console-frontdoor";

/// The egui memory key for the panel's slide animation (the Motion latch that
/// eases the rise 0↔1 — the dock's `DOCK_SLIDE_KEY` idiom).
const SLIDE_KEY: &str = "console-slide";

/// The left rail's width (categories + footer) — `SP_XL · 5` (160pt).
const RAIL_W: f32 = Style::SP_XL * 5.0;

/// The right entry-list pane's width — `SP_XL · 11` (352pt), wide enough for a
/// label + one-line description + the provenance tag on the 8px grid.
const LIST_W: f32 = Style::SP_XL * 11.0;

/// The whole panel's width — rail + list (the Win10 two-pane footprint).
const PANEL_W: f32 = RAIL_W + LIST_W;

/// The panel's height — `SP_XL · 18` (576pt), clamped to the screen at mount.
const PANEL_H: f32 = Style::SP_XL * 18.0;

/// One entry row's height — two text lines (label + description) on the grid.
const ROW_H: f32 = Style::SP_XL + Style::SP_S;

/// A group heading row's height (`SP_L`).
const HEADING_H: f32 = Style::SP_L;

/// One rail category row's height (`SP_L`).
const RAIL_ROW_H: f32 = Style::SP_L;

/// The rail's title block height ("Console" + the "Operations" subtitle).
const TITLE_H: f32 = Style::SP_XL + Style::SP_M;

/// The rail footer's height — the `user@host` + version lines (lock #43).
const FOOTER_H: f32 = Style::SP_XL + Style::SP_S;

/// The honest-gate notice strip reserved beneath the entry list (§7) — always
/// reserved so a raised notice never shifts the scrolled list.
const NOTICE_H: f32 = Style::SP_XL;

/// The keyboard focus ring's stroke — the EXPLORER-18 posture (design O11: the
/// selection is always legible), mirrored here because the explorer's const is
/// module-private.
const FOCUS_RING_W: f32 = 2.5;

/// An entry row's glyph edge (`SP_M`, 16pt) — smaller than the dock's 24px app
/// glyph, the row-scale icon.
const ENTRY_ICON: f32 = Style::SP_M;

/// A 1px hairline rule (the dock's `HAIRLINE_W` restated — module-private there).
const HAIRLINE_W: f32 = 1.0;

// ── the entry model (design "Entry model": a const table, no dead entries) ──

/// The subtle per-entry provenance tag (lock #38): whether the op is stock
/// Fedora tooling or the Quasar mesh platform's own layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provenance {
    /// Stock Fedora / systemd tooling.
    Fedora,
    /// The mesh platform's layer (meshctl / mackesd / the Bus / the channel).
    Quasar,
}

impl Provenance {
    /// The tag's label text.
    const fn label(self) -> &'static str {
        match self {
            Self::Fedora => "Fedora",
            Self::Quasar => "Quasar",
        }
    }

    /// The tag's tint — a subtle two-tone (§4 tokens): the platform's own ops
    /// read in the interactive accent, stock tooling sits dim.
    const fn color(self) -> egui::Color32 {
        match self {
            Self::Fedora => Style::TEXT_DIM,
            Self::Quasar => Style::ACCENT,
        }
    }
}

/// What activating an entry does (the design's `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    /// Open a **named terminal tab** running this command line (the launch
    /// model, design lock "Launch model"). Root ops embed `sudo` (lock #29).
    /// GATED until the CONSOLE-2 `spawn_tab` seam lands — activation raises the
    /// typed [`GateReason::SpawnTabNotWired`] notice, never a faked launch (§7).
    Tab(&'static str),
    /// Route to a shell surface (lock #41's "open the correct GUI surface") —
    /// live NOW through [`ConsoleRequest::Goto`].
    Link(Surface),
}

/// One Console entry: icon + label + one-line description + provenance
/// (lock #33), the `$PATH` tool its presence is honestly checked against, and
/// what activating it does.
struct ConsoleEntry {
    /// The row label.
    label: &'static str,
    /// The one-line description of what it runs (lock #33).
    desc: &'static str,
    /// The `$PATH` binary the entry needs (`""` = always available — surface
    /// links and shell built-ins). Absent → the row greys + reports it (§7).
    tool: &'static str,
    /// The Fedora/Quasar provenance tag (lock #38).
    provenance: Provenance,
    /// What activation does.
    kind: EntryKind,
}

/// One labelled group of the right pane (lock #6's domain taxonomy).
struct ConsoleGroup {
    /// The group heading + the rail jump-index label.
    label: &'static str,
    /// The group's entries, in locked order (lock #50).
    entries: &'static [ConsoleEntry],
}

/// The pinned pair (lock #31): a plain Terminal + the Monitor. The Terminal is
/// a LIVE surface link (the Terminal surface holds a real PTY shell today); the
/// Monitor is a command entry, gated like the rest until CONSOLE-2.
const PINNED: [ConsoleEntry; 2] = [
    ConsoleEntry {
        label: "Terminal",
        desc: "Open the Terminal surface — tabs, splits, mesh peers",
        tool: "",
        provenance: Provenance::Quasar,
        kind: EntryKind::Link(Surface::Terminal),
    },
    ConsoleEntry {
        label: "Monitor",
        desc: "Live per-process CPU / memory / IO (btop)",
        tool: "btop",
        provenance: Provenance::Fedora,
        kind: EntryKind::Tab("btop"),
    },
];

/// The seven operational groups in locked order (lock #6: System / Network /
/// Packages / Storage / Mesh / Containers & VMs / Shells; Power joins the left
/// rail and Custom the list tail under CONSOLE-4). Every command is a REAL tool
/// grounded in the live Eagle evaluation (btop not htop, nmcli not nmtui,
/// mtr / smartctl / podman / virsh present; ncdu to bundle) — no dead entries.
const GROUPS: [ConsoleGroup; 7] = [
    ConsoleGroup {
        label: "System",
        entries: &[
            ConsoleEntry {
                label: "Resource Monitor",
                desc: "Live per-process CPU / memory / IO (btop)",
                tool: "btop",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("btop"),
            },
            ConsoleEntry {
                label: "Services",
                desc: "Unit list — start / stop / restart from it (systemctl)",
                tool: "systemctl",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("systemctl list-units"),
            },
            ConsoleEntry {
                label: "Live Logs",
                desc: "Follow the system journal live (journalctl -f)",
                tool: "journalctl",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("journalctl -f"),
            },
            ConsoleEntry {
                label: "System Dashboard",
                desc: "Live control-group CPU / memory / IO (systemd-cgtop)",
                tool: "systemd-cgtop",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("systemd-cgtop"),
            },
        ],
    },
    ConsoleGroup {
        label: "Network",
        entries: &[
            ConsoleEntry {
                label: "Network Status",
                desc: "Mesh-aware summary: links, routes, overlay + peers",
                tool: "ip",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab("bash -lc 'ip -br addr; echo; ip route; echo; meshctl status'"),
            },
            ConsoleEntry {
                label: "Connections & Ports",
                desc: "Listening + established sockets (ss -tulpn)",
                tool: "ss",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("ss -tulpn"),
            },
            ConsoleEntry {
                label: "Path Test",
                desc: "ICMP path quality to the lighthouse overlay (mtr)",
                tool: "mtr",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("mtr 10.42.0.1"),
            },
            ConsoleEntry {
                label: "Manage Connections",
                desc: "NetworkManager device + connection overview (nmcli)",
                tool: "nmcli",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("nmcli"),
            },
            ConsoleEntry {
                label: "Firewall",
                desc: "Active zone: services, ports, rules (firewall-cmd)",
                tool: "firewall-cmd",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("sudo firewall-cmd --list-all"),
            },
        ],
    },
    ConsoleGroup {
        label: "Packages",
        entries: &[
            ConsoleEntry {
                label: "Check Updates",
                desc: "What would update, without changing anything (dnf)",
                tool: "dnf",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("dnf check-update"),
            },
            ConsoleEntry {
                label: "Apply Updates",
                desc: "Upgrade the whole system (sudo dnf upgrade)",
                tool: "dnf",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("sudo dnf upgrade"),
            },
            ConsoleEntry {
                label: "Installed Packages",
                desc: "Everything installed, searchable (dnf list)",
                tool: "dnf",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("dnf list --installed"),
            },
            ConsoleEntry {
                label: "Platform Update",
                desc: "Update the mesh platform from the signed channel",
                tool: "dnf",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab("sudo dnf upgrade magic-mesh"),
            },
            ConsoleEntry {
                label: "Flatpak",
                desc: "List + update the installed Flatpaks",
                tool: "flatpak",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("bash -lc 'flatpak list; flatpak update'"),
            },
        ],
    },
    ConsoleGroup {
        label: "Storage",
        entries: &[
            ConsoleEntry {
                label: "Disk Usage",
                desc: "Filesystem fill + block-device tree (df, lsblk)",
                tool: "df",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("bash -lc 'df -h; echo; lsblk'"),
            },
            ConsoleEntry {
                label: "Disk Explorer",
                desc: "Interactive disk-usage explorer (ncdu)",
                tool: "ncdu",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("ncdu /"),
            },
            ConsoleEntry {
                label: "Disk Health",
                desc: "SMART health verdict for each disk (smartctl -H)",
                tool: "smartctl",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab(
                    "bash -lc 'for d in /dev/sd? /dev/nvme?n1; do [ -e \"$d\" ] && sudo smartctl -H \"$d\"; done'",
                ),
            },
            ConsoleEntry {
                label: "Mesh Storage",
                desc: "The mesh share mount + Syncthing sync status",
                tool: "findmnt",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab(
                    "bash -lc 'findmnt /mnt/mesh-storage; echo; systemctl --no-pager status \"syncthing*\"'",
                ),
            },
        ],
    },
    ConsoleGroup {
        label: "Mesh",
        entries: &[
            ConsoleEntry {
                label: "Mesh Status",
                desc: "This node + fleet status roll-up (meshctl status)",
                tool: "meshctl",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab("meshctl status"),
            },
            ConsoleEntry {
                label: "Peers",
                desc: "Fleet-wide peer directory (meshctl fleet status)",
                tool: "meshctl",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab("meshctl fleet status"),
            },
            ConsoleEntry {
                label: "Cloud Status",
                desc: "The state/openstack mirror on the Bus spool",
                tool: "",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab(
                    "bash -lc 'ls -l \"${MDE_BUS_ROOT:-/run/mde-bus}/state/openstack\" 2>/dev/null || echo \"no cloud mirror published on this node\"'",
                ),
            },
            ConsoleEntry {
                label: "Cluster (etcd)",
                desc: "Endpoint health + members (etcdctl)",
                tool: "etcdctl",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab("bash -lc 'etcdctl endpoint health; etcdctl member list'"),
            },
        ],
    },
    ConsoleGroup {
        label: "Containers & VMs",
        entries: &[
            ConsoleEntry {
                label: "Containers",
                desc: "Every podman container, running or not",
                tool: "podman",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("podman ps --all"),
            },
            ConsoleEntry {
                label: "Virtual Machines",
                desc: "Every libvirt domain, running or not (virsh)",
                tool: "virsh",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("virsh list --all"),
            },
            ConsoleEntry {
                label: "OpenStack Servers",
                desc: "The cloud's server roster (openstack server list)",
                tool: "openstack",
                provenance: Provenance::Quasar,
                kind: EntryKind::Tab("openstack server list"),
            },
            ConsoleEntry {
                label: "Cloud Plane (GUI)",
                desc: "Open the Instances surface — the VM lifecycle GUI",
                tool: "",
                provenance: Provenance::Quasar,
                kind: EntryKind::Link(Surface::Instances),
            },
        ],
    },
    ConsoleGroup {
        label: "Shells",
        entries: &[
            ConsoleEntry {
                label: "User Shell",
                desc: "A login shell as the seat user",
                tool: "bash",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("bash -l"),
            },
            ConsoleEntry {
                label: "Root Shell",
                desc: "A root login shell (sudo -i)",
                tool: "sudo",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("sudo -i"),
            },
            ConsoleEntry {
                label: "tmux",
                desc: "Attach or create the console tmux session",
                tool: "tmux",
                provenance: Provenance::Fedora,
                kind: EntryKind::Tab("tmux new-session -A -s console"),
            },
        ],
    },
];

/// The flat activation list — the pinned pair then every group's entries in
/// order (the keyboard-nav ring + the presence table's index space).
fn static_rows() -> impl Iterator<Item = &'static ConsoleEntry> {
    PINNED
        .iter()
        .chain(GROUPS.iter().flat_map(|g| g.entries.iter()))
}

/// How many rows the flat list holds.
fn total_rows() -> usize {
    PINNED.len() + GROUPS.iter().map(|g| g.entries.len()).sum::<usize>()
}

/// The entry at a flat index (indices come from the same `static_rows` order,
/// so this cannot miss for `flat < total_rows()`).
fn entry_at(flat: usize) -> &'static ConsoleEntry {
    static_rows()
        .nth(flat)
        .expect("flat index within total_rows()")
}

// ── the honest gate (§7 — typed, never a fake) ──────────────────────────────

/// Why an activated entry could not run — the typed honest gate (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateReason {
    /// The launch leg is **`NotWired`**: opening a named terminal tab running a
    /// command needs the CONSOLE-2 `spawn_tab` seam on `mde-term-egui`
    /// (`TabbedTerminal::new_tab()` takes no command today).
    SpawnTabNotWired,
    /// The entry's underlying tool is not on this node's `$PATH`.
    ToolMissing(&'static str),
}

/// The notice the panel shows for a gated activation: which entry, and the
/// typed reason it did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateNotice {
    /// The activated entry's label.
    pub(crate) entry: String,
    /// The typed reason.
    pub(crate) reason: GateReason,
}

impl GateNotice {
    /// The operator-facing line (painted in the notice strip).
    fn text(&self) -> String {
        match self.reason {
            GateReason::SpawnTabNotWired => format!(
                "{}: NotWired — the terminal spawn-tab seam (CONSOLE-2) has not landed; \
                 this entry opens a named terminal tab once it does.",
                self.entry
            ),
            GateReason::ToolMissing(tool) => {
                format!(
                    "{}: \u{201c}{tool}\u{201d} is not installed on this node.",
                    self.entry
                )
            }
        }
    }
}

/// A shell-level request the Console records for `main.rs` to drain after the
/// frame — the panel never reaches the nav itself (§6, the `DockRequest` idiom).
/// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form for
/// crate-visible items in a private module — the dock's convention.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleRequest {
    /// Route the shell body to a surface (a live surface-link entry).
    Goto(Surface),
}

// ── state ────────────────────────────────────────────────────────────────────

/// The Console panel's cross-frame state: the open latch the dock's Start cell
/// toggles, the keyboard focus ring, the rail's pending jump-scroll, the honest
/// gate notice, and the pending shell request. Pure (no egui handles), so the
/// open/close + nav + gate invariants are unit-tested without a GPU.
pub struct ConsoleState {
    /// Whether the panel is up (the Start cell's toggle latch).
    open: bool,
    /// Set by [`Self::toggle`] and cleared at the end of the panel frame — the
    /// same-frame click-away guard (the Start-cell click that opened the panel
    /// lands outside it and must not read as a dismissal; the power-menu idiom).
    just_toggled: bool,
    /// The keyboard focus, a flat index into the activation ring (lock #40).
    focus: usize,
    /// Set when an arrow moved the focus this frame, so the focused row scrolls
    /// itself into view once, then cleared.
    focus_moved: bool,
    /// A rail category's pending jump-scroll target (lock #49) — the group
    /// index; drained by the list render.
    jump: Option<usize>,
    /// The honest gate notice for the last gated activation (§7).
    gate: Option<GateNotice>,
    /// The pending shell request, drained by `main.rs` each frame.
    pending: Option<ConsoleRequest>,
    /// Per-row `$PATH` presence, parallel to the flat list — refreshed on each
    /// open (cheap stats), so a just-installed tool ungreys on the next open.
    present: Vec<bool>,
    /// The footer's `user@host` (lock #43), resolved once.
    identity: String,
    /// The footer's platform version line (lock #43), baked once.
    version: String,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            open: false,
            just_toggled: false,
            focus: 0,
            focus_moved: false,
            jump: None,
            gate: None,
            pending: None,
            present: Vec::new(),
            identity: identity_line(),
            version: mde_theme::brand::build::version_line(),
        }
    }
}

impl ConsoleState {
    /// Whether the panel is up — the dock mirrors this into the Start cell's
    /// active tint each frame.
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// Toggle the panel (the Start cell's drained click; pressing Start again
    /// closes, lock #4). Opening resets the focus ring + notice and refreshes
    /// the `$PATH` presence table; either edge arms the same-frame click-away
    /// guard.
    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        self.just_toggled = true;
        if self.open {
            self.focus = 0;
            self.focus_moved = false;
            self.jump = None;
            self.gate = None;
            self.refresh_presence();
        }
    }

    /// Close the panel (Esc / click-away / a routed link).
    fn close(&mut self) {
        self.open = false;
        self.gate = None;
    }

    /// Drain the pending shell request — `main.rs` calls this each frame after
    /// the panel and drives the real nav (§6). `None` (drained once) otherwise.
    pub(crate) const fn take_request(&mut self) -> Option<ConsoleRequest> {
        self.pending.take()
    }

    /// Refresh the per-row `$PATH` presence table (called on open).
    fn refresh_presence(&mut self) {
        self.present = static_rows().map(|e| tool_present(e.tool)).collect();
    }

    /// Activate the flat row `flat` (a click or Enter): a surface link routes +
    /// closes; a command entry raises its typed honest gate — the missing tool
    /// by name, else the CONSOLE-2 `NotWired` seam (§7 — never a faked launch).
    fn activate(&mut self, flat: usize) {
        let entry = entry_at(flat);
        match entry.kind {
            EntryKind::Link(surface) => {
                self.pending = Some(ConsoleRequest::Goto(surface));
                self.close();
            }
            EntryKind::Tab(_) => {
                let reason = if self.present.get(flat).copied().unwrap_or(false) {
                    GateReason::SpawnTabNotWired
                } else {
                    GateReason::ToolMissing(entry.tool)
                };
                self.gate = Some(GateNotice {
                    entry: entry.label.to_owned(),
                    reason,
                });
            }
        }
    }

    /// Pin a row's presence for a deterministic test (the live table reads the
    /// build host's `$PATH`, which a unit test must not depend on).
    #[cfg(test)]
    fn force_presence(&mut self, flat: usize, present: bool) {
        if flat < self.present.len() {
            self.present[flat] = present;
        }
    }
}

/// Whether `tool` resolves to an executable on `$PATH` (`""` = no tool needed,
/// always present). A real filesystem check — the honest greying's ground truth.
fn tool_present(tool: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if tool.is_empty() {
        return true;
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(tool);
        candidate
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// The footer's `user@host` (lock #43): `$USER` / `$LOGNAME` → `operator` (the
/// backdrop's identity precedence), at this node's hostname (the shared shell
/// resolution — no second hostname idiom).
fn identity_line() -> String {
    let user = ["USER", "LOGNAME"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "operator".to_owned());
    format!("{user}@{}", crate::explorer::local_hostname())
}

// ── stable ids (the dock's addressable-cell idiom, for routing + tests) ─────

/// A flat entry row's stable id.
fn console_entry_id(flat: usize) -> egui::Id {
    egui::Id::new(("console-entry", flat))
}

/// A rail category row's stable id.
fn console_rail_id(label: &str) -> egui::Id {
    egui::Id::new(("console-rail", label))
}

/// A group heading's stable id (display-only; tests read its settled rect to
/// prove the jump-scroll).
fn console_heading_id(label: &str) -> egui::Id {
    egui::Id::new(("console-heading", label))
}

// ── render ───────────────────────────────────────────────────────────────────

/// Mount the Console panel for this frame: the Win10 two-pane over the shared
/// [`Style`] (§4), rising from the screen bottom beside the dock column over the
/// shared [`Motion`] table (lock #44). Fully hidden + settled it mounts **no
/// layer at all** (the dock's passthrough guarantee), so a closed Console steals
/// no input from the surface beneath. Esc, a click away, and the Start cell's
/// re-toggle all dismiss (lock #4).
#[allow(clippy::suboptimal_flops)] // the slide offset reads clearer than mul_add
pub fn console_panel(ctx: &egui::Context, state: &mut ConsoleState) {
    let t = Motion::animate(ctx, SLIDE_KEY, state.open, Motion::BASE);
    if t <= 0.001 {
        state.just_toggled = false;
        return;
    }

    let screen = ctx.screen_rect();
    let panel_h = PANEL_H.min(screen.height() - Style::SP_XL);
    // The slide-up: the panel's top rides from the screen bottom (t=0) to its
    // settled height (t=1), anchored beside the dock column (the reconciliation
    // note in the module doc — the Win10 bottom-left footprint kept).
    let top = screen.bottom() - t * panel_h;

    let area = egui::Area::new(egui::Id::new(CONSOLE_AREA))
        .order(egui::Order::Foreground)
        // It SLIDES (lock #44) — never egui's default fade-in.
        .fade_in(false)
        .constrain(false)
        .fixed_pos(egui::pos2(DOCK_W, top))
        .show(ctx, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(PANEL_W, panel_h), egui::Sense::hover());
            paint_panel_frame(ui, rect);
            if state.open {
                handle_keys(ui, state);
            }
            rail(ui, rect, state);
            list_pane(ui, rect, state);
        });

    // Click-away dismissal (lock #4) — but never on the very frame the Start
    // cell toggled (that click lands outside the panel and would dismiss it
    // in the same frame; the power-menu / overflow-popup guard).
    if state.open && !state.just_toggled && area.response.clicked_elsewhere() {
        state.close();
    }
    state.just_toggled = false;

    // Keep frames flowing while the slide is in flight (the dock's tween idiom).
    if t > 0.001 && t < 0.999 {
        ctx.request_repaint();
    }
}

/// The panel's chrome: the solid SURFACE sheet, the outer hairline, and the
/// rail/list divider (§4 tokens).
fn paint_panel_frame(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.vline(
        rect.left() + RAIL_W,
        (rect.top() + Style::SP_XS)..=(rect.bottom() - Style::SP_XS),
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
}

/// The keyboard layer (locks #40/#48, the EXPLORER-18 posture): Esc closes,
/// ↑/↓ move the focus ring (wrapping), Enter activates the focused row. Inert
/// while a text field owns the keyboard (egui's focus), so typing never navs.
fn handle_keys(ui: &egui::Ui, state: &mut ConsoleState) {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return;
    }
    let (esc, up, down, enter) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
        )
    });
    if esc {
        state.close();
        return;
    }
    let total = total_rows();
    if down {
        state.focus = (state.focus + 1) % total;
        state.focus_moved = true;
    }
    if up {
        state.focus = state.focus.checked_sub(1).unwrap_or(total - 1);
        state.focus_moved = true;
    }
    if enter {
        state.activate(state.focus);
    }
}

/// The left rail (lock #5): the "Console / Operations" title (lock #39), the
/// category jump-index (lock #49), and the `user@host · version` footer
/// (lock #43). The Power section joins this rail under CONSOLE-4.
fn rail(ui: &egui::Ui, rect: egui::Rect, state: &mut ConsoleState) {
    let painter = ui.painter().clone();
    let rail = egui::Rect::from_min_size(rect.min, egui::vec2(RAIL_W, rect.height()));

    // Title block: the menu is titled "Console" / "Operations" (lock #39).
    painter.text(
        egui::pos2(rail.left() + Style::SP_S, rail.top() + Style::SP_S),
        egui::Align2::LEFT_TOP,
        "Console",
        egui::FontId::proportional(Style::BODY),
        Style::TEXT,
    );
    painter.text(
        egui::pos2(
            rail.left() + Style::SP_S,
            rail.top() + Style::SP_S + Style::SP_L,
        ),
        egui::Align2::LEFT_TOP,
        "Operations",
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );

    // The category jump-index (lock #49): one row per group; a click asks the
    // list to jump-scroll to that group's heading.
    let mut y = rail.top() + TITLE_H;
    for (i, group) in GROUPS.iter().enumerate() {
        let row =
            egui::Rect::from_min_size(egui::pos2(rail.left(), y), egui::vec2(RAIL_W, RAIL_ROW_H));
        let resp = ui.interact(row, console_rail_id(group.label), egui::Sense::click());
        if resp.hovered() {
            painter.rect_filled(row, Style::RADIUS, Style::SURFACE_HI);
        }
        let color = if resp.hovered() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        };
        painter.text(
            egui::pos2(row.left() + Style::SP_S, row.center().y),
            egui::Align2::LEFT_CENTER,
            group.label,
            egui::FontId::proportional(Style::SMALL),
            color,
        );
        if resp.clicked() {
            state.jump = Some(i);
        }
        y += RAIL_ROW_H;
    }

    // The footer (lock #43): user@host over the platform version, in the Win10
    // corner. (The Power button joins it under CONSOLE-4.)
    let footer_top = rail.bottom() - FOOTER_H;
    painter.hline(
        (rail.left() + Style::SP_XS)..=(rail.right() - Style::SP_XS),
        footer_top,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
    painter.text(
        egui::pos2(rail.left() + Style::SP_S, footer_top + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        &state.identity,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    painter.text(
        egui::pos2(
            rail.left() + Style::SP_S,
            footer_top + Style::SP_XS + Style::SP_M,
        ),
        egui::Align2::LEFT_TOP,
        &state.version,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

/// The right pane: the pinned pair + the grouped entry list in one scroll
/// region (lock #5 — the rail's jump targets scroll this), over the reserved
/// honest-gate notice strip (§7).
fn list_pane(ui: &mut egui::Ui, rect: egui::Rect, state: &mut ConsoleState) {
    let list_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + RAIL_W + Style::SP_S, rect.top() + Style::SP_S),
        egui::pos2(rect.right() - Style::SP_S, rect.bottom() - NOTICE_H),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(list_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );

    let mut activated: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("console-list")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            // Pinned (lock #31): a plain Terminal + Monitor lead the pane.
            heading(ui, "Pinned");
            for (flat, entry) in PINNED.iter().enumerate() {
                if entry_row(ui, flat, entry, state) {
                    activated = Some(flat);
                }
            }

            // The grouped list (lock #6), each group under its heading; a rail
            // jump scrolls its heading to the top (lock #49).
            let mut flat = PINNED.len();
            for (gi, group) in GROUPS.iter().enumerate() {
                let head = heading(ui, group.label);
                if state.jump == Some(gi) {
                    ui.scroll_to_rect(head, Some(egui::Align::Min));
                    state.jump = None;
                }
                for entry in group.entries {
                    if entry_row(ui, flat, entry, state) {
                        activated = Some(flat);
                    }
                    flat += 1;
                }
            }
        });
    state.focus_moved = false;
    if let Some(flat) = activated {
        state.activate(flat);
    }

    // The honest-gate notice strip (§7): the typed reason a gated activation
    // did not run — always reserved so the list never shifts under a notice.
    if let Some(gate) = &state.gate {
        let strip = egui::Rect::from_min_max(
            egui::pos2(rect.left() + RAIL_W + Style::SP_S, rect.bottom() - NOTICE_H),
            egui::pos2(rect.right() - Style::SP_S, rect.bottom() - Style::SP_XS),
        );
        let painter = ui.painter().clone();
        painter.rect_filled(strip, Style::RADIUS, Style::SURFACE_HI);
        painter.text(
            egui::pos2(strip.left() + Style::SP_S, strip.center().y),
            egui::Align2::LEFT_CENTER,
            gate.text(),
            egui::FontId::proportional(Style::SMALL),
            Style::WARN,
        );
    }
}

/// One group heading row — the micro-label above its entries (display-only;
/// registered under a stable id so tests read the jump-scroll's effect back).
/// Returns the heading's rect (the rail's jump-scroll target).
fn heading(ui: &mut egui::Ui, label: &str) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADING_H),
        egui::Sense::hover(),
    );
    ui.interact(rect, console_heading_id(label), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_XS, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_uppercase(),
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
    rect
}

/// One entry row (lock #33): the row glyph (a surface link wears its surface's
/// brand glyph; a command entry wears the Terminal front-door glyph), the label
/// over the one-line description, and the subtle provenance tag (lock #38). An
/// absent tool greys the row + names the absence in-line (§7). The focused row
/// wears the EXPLORER-18 focus ring (lock #48) and scrolls itself into view
/// when the ring just moved. Returns `true` on a click (the caller activates).
fn entry_row(ui: &mut egui::Ui, flat: usize, entry: &ConsoleEntry, state: &ConsoleState) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    let resp = ui.interact(rect, console_entry_id(flat), egui::Sense::click());
    let hovered = resp.hovered();
    let present = state.present.get(flat).copied().unwrap_or(true);
    let focused = state.open && state.focus == flat;
    let painter = ui.painter().clone();

    if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    if focused {
        painter.rect_stroke(
            rect,
            Style::RADIUS,
            egui::Stroke::new(FOCUS_RING_W, Style::ACCENT_HI),
            egui::StrokeKind::Inside,
        );
        if state.focus_moved {
            ui.scroll_to_rect(rect, None);
        }
    }

    // The row glyph, through the dock's shared cached loader (§6).
    let icon_id = match entry.kind {
        EntryKind::Link(surface) => surface.icon_id(),
        EntryKind::Tab(_) => IconId::Terminal,
    };
    let tint = if !present {
        Style::TEXT_DIM
    } else if hovered || focused {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    if let Some(tex) = icon_texture(ui.ctx(), icon_id, ENTRY_ICON, tint) {
        let icon = egui::Rect::from_center_size(
            egui::pos2(
                rect.left() + Style::SP_S + ENTRY_ICON / 2.0,
                rect.center().y,
            ),
            egui::vec2(ENTRY_ICON, ENTRY_ICON),
        );
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
    }

    // Label + one-line description (lock #33); an absent tool reads greyed with
    // the absence named in-line (§7 — never a dead entry).
    let text_left = rect.left() + Style::SP_XL;
    let label_color = if present {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    painter.text(
        egui::pos2(text_left, rect.top() + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        entry.label,
        egui::FontId::proportional(Style::BODY),
        label_color,
    );
    let desc = if present {
        entry.desc.to_owned()
    } else {
        format!("{} \u{2014} not installed", entry.desc)
    };
    painter.text(
        egui::pos2(text_left, rect.bottom() - Style::SP_XS),
        egui::Align2::LEFT_BOTTOM,
        desc,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );

    // The subtle provenance tag, right-aligned (lock #38).
    painter.text(
        egui::pos2(rect.right() - Style::SP_S, rect.top() + Style::SP_XS),
        egui::Align2::RIGHT_TOP,
        entry.provenance.label(),
        egui::FontId::proportional(Style::SMALL),
        entry.provenance.color(),
    );

    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::{
        console_entry_id, console_heading_id, console_panel, console_rail_id, entry_at,
        identity_line, static_rows, tool_present, total_rows, ConsoleRequest, ConsoleState,
        EntryKind, GateReason, CONSOLE_AREA, GROUPS, PINNED,
    };
    use crate::dock::Surface;
    use mde_egui::egui;
    use mde_egui::Style;

    /// Drive ONE headless frame of the console over a stand-in surface (the
    /// dock tests' `drive_vdock` idiom — the same `Context::run` path the DRM
    /// runner drives, minus the GPU).
    fn drive(
        ctx: &egui::Context,
        state: &mut ConsoleState,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            console_panel(ctx, state);
        })
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn press_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn release_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// Click `center` — press one frame, release the next (the egui click model
    /// the dock tests use). The caller primes the layout first.
    fn click(ctx: &egui::Context, state: &mut ConsoleState, center: egui::Pos2, size: egui::Vec2) {
        drive(
            ctx,
            state,
            vec![egui::Event::PointerMoved(center), press_at(center)],
            size,
        );
        drive(ctx, state, vec![release_at(center)], size);
    }

    /// The console's floating-Area `LayerId`.
    fn console_layer() -> egui::LayerId {
        egui::LayerId::new(egui::Order::Foreground, egui::Id::new(CONSOLE_AREA))
    }

    const SZ: egui::Vec2 = egui::Vec2::new(1280.0, 800.0);

    // ── the entry table (design "Entry model" — real, locked, no dead rows) ──

    #[test]
    fn the_entry_table_matches_the_locked_taxonomy_and_holds_no_dead_rows() {
        // Lock #6 — the seven operational groups in locked order (Power joins
        // the rail and Custom the tail under CONSOLE-4).
        let labels: Vec<&str> = GROUPS.iter().map(|g| g.label).collect();
        assert_eq!(
            labels,
            [
                "System",
                "Network",
                "Packages",
                "Storage",
                "Mesh",
                "Containers & VMs",
                "Shells"
            ],
            "the locked group taxonomy + order"
        );
        // No dead entries: every group populated, every row fully described,
        // every command entry a real command line.
        for group in &GROUPS {
            assert!(!group.entries.is_empty(), "{} is empty", group.label);
        }
        for entry in static_rows() {
            assert!(!entry.label.is_empty() && !entry.desc.is_empty());
            if let EntryKind::Tab(cmd) = entry.kind {
                assert!(
                    !cmd.trim().is_empty(),
                    "{} has a blank command",
                    entry.label
                );
                assert!(
                    !entry.tool.is_empty() || cmd.starts_with("bash "),
                    "{} declares no presence-check tool",
                    entry.label
                );
            }
        }
        // Lock #31 — pinned is exactly a plain Terminal + Monitor: the Terminal
        // a LIVE surface link, the Monitor the btop command entry.
        assert_eq!(PINNED.len(), 2);
        assert_eq!(PINNED[0].kind, EntryKind::Link(Surface::Terminal));
        assert_eq!(PINNED[1].kind, EntryKind::Tab("btop"));
        // Lock #41 — Containers & VMs carries the Cloud-plane surface link.
        let cvm = GROUPS
            .iter()
            .find(|g| g.label == "Containers & VMs")
            .expect("the combined group exists");
        assert!(
            cvm.entries
                .iter()
                .any(|e| e.kind == EntryKind::Link(Surface::Instances)),
            "the Containers & VMs group links to the Instances surface"
        );
        // The flat index space is coherent.
        assert_eq!(static_rows().count(), total_rows());
        assert_eq!(entry_at(0).label, "Terminal");
    }

    #[test]
    fn tool_presence_is_a_real_path_check() {
        // `sh` exists on any Linux build host; a nonsense binary does not; the
        // empty tool (surface links) is always present.
        assert!(tool_present("sh"), "sh must resolve on $PATH");
        assert!(!tool_present("definitely-not-a-real-tool-xyzzy"));
        assert!(tool_present(""));
    }

    #[test]
    fn the_footer_identity_reads_user_at_host() {
        let line = identity_line();
        assert!(line.contains('@'), "identity must read user@host: {line}");
        assert!(!line.starts_with('@') && !line.ends_with('@'));
    }

    // ── open/close (locks #1/#4) ─────────────────────────────────────────────

    #[test]
    fn the_start_toggle_opens_and_a_second_toggle_closes() {
        // Pressing the Start cell again closes (lock #4) — the dock drains the
        // click into this same toggle either way.
        let mut s = ConsoleState::default();
        assert!(!s.is_open(), "closed by default");
        s.toggle();
        assert!(s.is_open(), "the Start toggle opens the panel");
        s.toggle();
        assert!(!s.is_open(), "pressing Start again closes it");
    }

    #[test]
    fn a_closed_console_mounts_no_layer_and_an_open_one_paints() {
        // The dock's passthrough guarantee: closed + settled → no Area at all,
        // so input over the panel's would-be footprint reaches the surface.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        drive(&ctx, &mut s, Vec::new(), SZ);
        drive(&ctx, &mut s, Vec::new(), SZ);
        let inside = egui::pos2(crate::dock::DOCK_W + 100.0, SZ.y - 100.0);
        assert_ne!(
            ctx.layer_id_at(inside),
            Some(console_layer()),
            "a CLOSED console must not float an intercepting layer"
        );

        // Open on a fresh context (the slide latch settles at the open endpoint
        // on first sight) → the layer mounts and the frame paints primitives.
        let ctx2 = egui::Context::default();
        Style::install(&ctx2);
        let mut s2 = ConsoleState::default();
        s2.toggle();
        drive(&ctx2, &mut s2, Vec::new(), SZ);
        let out = drive(&ctx2, &mut s2, Vec::new(), SZ);
        assert_eq!(
            ctx2.layer_id_at(inside),
            Some(console_layer()),
            "an OPEN console claims its footprint"
        );
        let prims = ctx2.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the open console painted nothing");
    }

    #[test]
    fn esc_closes_the_panel() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        assert!(s.is_open());
        drive(&ctx, &mut s, vec![key(egui::Key::Escape)], SZ);
        assert!(!s.is_open(), "Esc dismisses the Console (lock #4)");
    }

    #[test]
    fn click_away_closes_but_never_on_the_opening_frame() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        // The very frame the Start cell toggled: its click lands outside the
        // panel — the guard must swallow it (the power-menu opened idiom).
        let far = egui::pos2(SZ.x - 40.0, 40.0);
        drive(
            &ctx,
            &mut s,
            vec![egui::Event::PointerMoved(far), release_at(far)],
            SZ,
        );
        assert!(s.is_open(), "the opening click must not self-dismiss");
        // Settle, then a real click away → dismissed (lock #4).
        drive(&ctx, &mut s, Vec::new(), SZ);
        click(&ctx, &mut s, far, SZ);
        assert!(!s.is_open(), "a click away dismisses the Console");
    }

    // ── keyboard nav + activation (locks #40/#48, §7 honest gates) ──────────

    #[test]
    fn arrows_move_the_focus_ring_and_enter_routes_a_live_surface_link() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        assert_eq!(s.focus, 0, "the ring opens on the pinned Terminal");

        drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
        assert_eq!(s.focus, 1, "ArrowDown advances the ring");
        drive(&ctx, &mut s, vec![key(egui::Key::ArrowUp)], SZ);
        assert_eq!(s.focus, 0, "ArrowUp retreats the ring");
        drive(&ctx, &mut s, vec![key(egui::Key::ArrowUp)], SZ);
        assert_eq!(s.focus, total_rows() - 1, "the ring wraps at the top");

        // Enter on the pinned Terminal (a LIVE link): routes + closes.
        let mut s2 = ConsoleState::default();
        s2.toggle();
        drive(&ctx, &mut s2, Vec::new(), SZ);
        drive(&ctx, &mut s2, vec![key(egui::Key::Enter)], SZ);
        assert_eq!(
            s2.take_request(),
            Some(ConsoleRequest::Goto(Surface::Terminal)),
            "the pinned Terminal routes to the Terminal surface"
        );
        assert!(!s2.is_open(), "a routed link closes the panel");
        assert_eq!(s2.take_request(), None, "the request drains exactly once");
    }

    #[test]
    fn a_command_entry_raises_the_typed_notwired_gate_never_a_fake_launch() {
        // §7 — the launch leg needs the CONSOLE-2 spawn-tab seam; until it
        // lands, Enter on a command entry raises the TYPED NotWired notice and
        // routes nothing. Presence is pinned so the verdict is host-independent.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        s.force_presence(1, true); // the pinned Monitor (btop), "installed"
        drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
        drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
        let gate = s.gate.clone().expect("a gated activation raises a notice");
        assert_eq!(gate.reason, GateReason::SpawnTabNotWired);
        assert_eq!(gate.entry, "Monitor");
        assert_eq!(s.take_request(), None, "a gated entry routes NOTHING");
        assert!(s.is_open(), "the panel stays up so the notice is read");

        // The same entry with its tool absent names the missing tool instead.
        s.force_presence(1, false);
        drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
        assert_eq!(
            s.gate.clone().expect("still gated").reason,
            GateReason::ToolMissing("btop")
        );
    }

    #[test]
    fn clicking_an_entry_row_activates_it() {
        // The pointer path matches the keyboard path: a click on the pinned
        // Terminal's row routes to the Terminal surface (through the same
        // activate). Uses the stable per-row id (the dock's addressable idiom).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        drive(&ctx, &mut s, Vec::new(), SZ);
        let row = ctx
            .read_response(console_entry_id(0))
            .expect("the pinned Terminal row is registered")
            .rect;
        click(&ctx, &mut s, row.center(), SZ);
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::Goto(Surface::Terminal)),
            "a row click routes like Enter"
        );
    }

    // ── the rail jump-index (lock #49) ───────────────────────────────────────

    #[test]
    fn a_rail_category_click_jump_scrolls_the_list_to_its_group() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        drive(&ctx, &mut s, Vec::new(), SZ);

        // The Shells heading starts far down the (scrolling) list.
        let before = ctx
            .read_response(console_heading_id("Shells"))
            .expect("the Shells heading is registered")
            .rect
            .top();

        // Click the rail's "Shells" jump cell, then let the scroll settle.
        let rail_row = ctx
            .read_response(console_rail_id("Shells"))
            .expect("the Shells rail cell is registered")
            .rect;
        click(&ctx, &mut s, rail_row.center(), SZ);
        for _ in 0..6 {
            drive(&ctx, &mut s, Vec::new(), SZ);
        }
        let after = ctx
            .read_response(console_heading_id("Shells"))
            .expect("the Shells heading is still registered")
            .rect
            .top();
        assert!(
            after < before - Style::SP_XL,
            "the jump must scroll the Shells group up the pane (before {before}, after {after})"
        );
    }
}
