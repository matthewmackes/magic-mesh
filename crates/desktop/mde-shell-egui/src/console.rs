//! CONSOLE — the **Terminal's operational front door** (design
//! `docs/design/console-frontdoor.md`, CONSOLE-1).
//!
//! A Carbon-styled taxonomy of **operational terminal ops**: the **left rail**
//! leads with the `user@host · version` identity block (lock #43), then the
//! category jump-index (clicking a category jump-scrolls the list, lock #49),
//! then the Power section anchored at the rail's true bottom edge (lock #11 —
//! see the WIN7-5 update below for why identity moved to the top); the
//! **right pane** is the pinned Terminal + Monitor pair (lock #31) above the
//! grouped entry list — each row an icon + label + one-line description + a
//! subtle Fedora/Quasar provenance tag (locks #33/#38). Full arrow-key nav
//! with the EXPLORER-18 focus-ring posture (locks #40/#48).
//!
//! **WIN7-2 update:** this module used to mount its own floating `egui::Area`
//! (slide/Esc/click-away, toggled straight off the dock's Start cell). The
//! win7-desktop-survey (lock #10) migrates that whole front door into the new
//! **Start Menu**'s right pane (`crate::start_menu`) — the Start Menu now owns
//! the outer panel (Area, slide, Esc, click-away, the bottom-left footprint) and
//! embeds this module's content at its own right-pane rect via
//! [`console_content`]. `ConsoleState::open` is no longer a self-toggled latch;
//! it is mirrored in from the Start Menu each frame ([`ConsoleState::set_open`],
//! the `DockState::set_active` idiom) so the focus ring / `handle_keys` still
//! read a meaningful "am I showing" bit. This is a presentation-layer
//! extraction only — WIN7-5 is what actually **redesigns** this content for its
//! new home (lock #10); today it renders unchanged, just embedded rather than
//! self-mounted. Every action that used to close the whole front door (a routed
//! link, a spawned tab, a fired power verb, Esc) still calls [`ConsoleState::close`]
//! exactly as before; `start_menu` detects that self-closure and dismisses the
//! whole Start Menu with it (see `start_menu`'s module doc).
//!
//! **WIN7-5 update:** this content's PRESENTATION is now genuinely redesigned
//! for its Start Menu home (lock #10) — WIN7-2's straight embed above is
//! superseded; `ConsoleState`'s data/actions and the CONSOLE-2 activation
//! seam are completely UNCHANGED (same 7 groups, same Power semantics/typed-
//! arming, same Custom persistence) — only HOW this content draws changed.
//! Two real problems this unit found (not assumed) in WIN7-2's straight
//! embed: (1) the rail positioned its Power section by subtracting from the
//! bottom (`footer_top - POWER_H`) with nothing accounting for the space
//! ABOVE it, leaving an unaccounted ~168pt dead gap between the jump-index
//! and Power — closed to one deliberate, named, tested [`RAIL_SECTION_GAP`];
//! (2) the identity block (`user@host` + version) sat in a bottom FOOTER
//! underneath Power, backwards from the authentic Win7/Win10 Start Menu
//! shape (the signed-in user leads the rail; Power alone anchors the TRUE
//! bottom, lock #11) — relocated to the top, and the old "Console" /
//! "Operations" title block it displaces is dropped outright rather than
//! relocated, since the tile pane beside it (`start_menu.rs`) carries no
//! equivalent self-title either (a screen reader still gets this pane's
//! identity from `start_menu.rs`'s own "Console" `Role::Group` landmark).
//! The jump-index rows (lock #49) are now icon+label+"N entries" mini-rows
//! at [`JUMP_ROW_H`] — deliberately the SAME height as `start_menu::TILE_H`,
//! so the rail's nav rows line up with the tile grid beside it — reusing
//! each group's own first entry's icon as a representative glyph (no new
//! data: `ConsoleGroup` gained no field) in the SAME icon+label shape
//! [`entry_row`] already uses, just condensed, so the rail reads as a
//! smaller sibling of the list it jump-scrolls rather than a bare text menu
//! bolted beside it. The Custom row wears the SAME `Provenance::Quasar`
//! accent every operator-owned entry already tags itself with, at rest,
//! flagging "this one's yours" the way its own entries already do. The list
//! pane's own group headings ([`heading`]) were investigated and found to
//! ALREADY match `start_menu.rs`'s `tile_group_heading` byte-for-byte (both
//! the uppercased-SMALL/TEXT_DIM/`SP_XS`-left-inset recipe) — left
//! deliberately untouched, already coherent with the tile pane. Considered
//! and REJECTED: `dock.rs`'s own app-picker group-heading treatment
//! (per-group categorical accent colour, centred, non-uppercased) — its six
//! named hues (Comms/Workloads/Terminals/Mesh/System/Media) are already
//! claimed by a DIFFERENT taxonomy than this module's seven operational
//! groups (System/Network/Packages/Storage/Mesh/Containers&VMs/Shells);
//! reusing them here would blur what a categorical accent means everywhere
//! else it appears, and minting seven new hues would violate this shell's
//! one-categorical-palette convention (`Style`'s own test coverage). Also
//! considered and rejected: threading `nav.surface` in so a jump/entry row
//! could show dock.rs's "currently active surface" fill+bar+tint ladder —
//! new cross-module plumbing this module doesn't have today, well beyond a
//! presentation redesign of what's already there. Accesskit (lock #14):
//! every raw-painted interactive row this unit touches now exports its own
//! `Button` node (see the "accesskit" section near the bottom of this file)
//! — WIN7-2 shipped this whole module's embedding with only the Start
//! Menu's PANEL-level landmarks covering it; individual rows were
//! explicitly flagged as not-yet-covered. A new `Live::Polite` region also
//! announces the honest-gate notice (§7) when it fires — previously
//! visual-only, so a screen-reader user pressing a gated command heard
//! nothing explaining why. Left deliberately untouched: `RAIL_W`/`LIST_W`/
//! `PANEL_W`/`PANEL_H` (so `start_menu.rs`'s already-tested overall
//! footprint is unaffected), the Power section's own internal sizing/arming
//! logic (safety-critical, left alone beyond adding accesskit), and the
//! list pane's entry-row visual language (already dense/coherent — this
//! unit's redesign budget went to the rail, which genuinely needed it).
//! WIN7-7 remains the later crate-wide full accesskit sweep; this unit does
//! not claim to close every gap, only the rows it rewrote the rendering of.
//! **WIN7-7 update:** audited — every raw-painted `ui.interact` call site in
//! this file (jump rows, static/Custom entry rows, the Custom remove cross,
//! the Custom add-form's Add row, the 4 Power rows, the arming stage's
//! Confirm/Cancel) already had its own `install_row_accessibility` call from
//! this unit; the crate-wide sweep found no residual gap HERE and changed
//! nothing in this file. The real gaps it closed were in `dock.rs` (the
//! bottom taskbar had NO accesskit at all before WIN7-7) and `start_menu.rs`
//! (the panel's own open/close transition had no live announcement).
//!
//! **The launch (CONSOLE-5, §6/§7):** activating a command entry opens a
//! **named terminal tab running it** through the CONSOLE-2 `spawn_tab` seam on
//! `mde-term-egui` — the panel records a typed [`ConsoleRequest::SpawnTab`] the
//! shell drains (`main.rs`), switching to `Surface::Terminal` and driving
//! `TerminalSurface::spawn_tab`. The line rides a login shell (`bash -lc …`) so
//! its shell syntax is honored, and a **root op** (a leading `sudo`, lock #29)
//! runs that shell under [`sudo_argv`] so sudo prompts interactively in the
//! tab's PTY. Surface-link entries (the pinned Terminal, the Containers&VMs
//! "Cloud plane" link, lock #41) route for real through the shell nav. A command
//! whose underlying tool is absent from `$PATH` renders greyed and reports the
//! missing tool by name — the one honest gate that remains (the design's "no
//! dead entries" rule, §7).
//!
//! **CONSOLE-4** adds the rail's **Power section** (lock #28: Lock → the shell
//! curtain, Suspend at once, Reboot / Shut Down behind the VDOCK-4 typed-arming
//! echo — every verb drives the REAL seam via [`ConsoleRequest`]: the curtain /
//! `system.honor_power`, never a raw `systemctl`) and the **Custom group**
//! (lock #35): operator-registered named command entries, added in-UI and
//! persisted to `console-custom.json` under the client data dir (atomic
//! temp + rename, the timers idiom). A custom entry's launch rides the same
//! spawn-tab seam, opening its own named tab (CONSOLE-5).
//!
//! **CONSOLE-3** is the front door's CONTENT: the const [`ConsoleEntry`] table
//! across every operational group (System / Network / Packages / Storage / Mesh
//! / Containers & VMs / Shells), each row a real tool honest-gated on `$PATH`
//! (§7) and carrying its own **domain glyph** (lock #33) — System / Storage /
//! Mesh / Cloud / Signal / … — so the menu scans by domain rather than a
//! wall of identical terminal icons. The Containers & VMs group's Cloud-plane
//! row is the Workbench Cloud-plane link (the combined-group decision, Q41/Q50).
//!
//! Like the dock, this module is pure chrome + state: it records a typed
//! [`ConsoleRequest`] the shell drains after the frame (`main.rs`), and never
//! reaches the nav / curtain / seat itself (§6, the VDOCK deferred-wire idiom).

use std::fs;
use std::path::{Path, PathBuf};

use mde_egui::egui;
use mde_egui::Style;
use mde_seat::PowerVerb;
use mde_term_egui::sudo_argv;
use mde_theme::brand::icons::IconId;
use serde::{Deserialize, Serialize};

use crate::dock::{icon_texture, Surface};
use crate::workbench::Plane;

// ── geometry (all §4 token math, the dock's 8px grid) ───────────────────────

/// The left rail's width (identity + categories + Power, WIN7-5's top-to-
/// bottom order — see the module doc) — `SP_XL · 5` (160pt).
const RAIL_W: f32 = Style::SP_XL * 5.0;

/// The right entry-list pane's width — `SP_XL · 11` (352pt), wide enough for a
/// label + one-line description + the provenance tag on the 8px grid.
const LIST_W: f32 = Style::SP_XL * 11.0;

/// The whole content's width — rail + list (the Win10 two-pane footprint).
/// `pub(crate)` — WIN7-2's `start_menu` reads it to size its own right pane
/// (the rect it hands to [`console_content`]) exactly to this content's width.
pub(crate) const PANEL_W: f32 = RAIL_W + LIST_W;

/// The content's height — `SP_XL · 18` (576pt). `pub(crate)` — `start_menu`
/// reuses this AS its own overall panel height (already satisfying the
/// win7-desktop-survey's lock #2 "roughly half-height", clamped to the screen
/// at mount) rather than inventing a second height for the embedding panel.
pub(crate) const PANEL_H: f32 = Style::SP_XL * 18.0;

/// One entry row's height — two text lines (label + description) on the grid.
const ROW_H: f32 = Style::SP_XL + Style::SP_S;

/// A group heading row's height (`SP_L`).
const HEADING_H: f32 = Style::SP_L;

/// One Power-section action row's height (`SP_L`). WIN7-5 — previously also
/// the jump-index row height; the jump-index now uses its own
/// [`JUMP_ROW_H`], so this constant's scope narrowed to `power_section`'s
/// four action rows (and the arming stage sharing the same box).
const RAIL_ROW_H: f32 = Style::SP_L;

/// WIN7-5 — the rail's identity block height: the `user@host` + platform
/// version lines (lock #43). The same two-line recipe this rail always used
/// for that pair (previously painted as a bottom FOOTER, underneath Power);
/// this unit relocates the block to the TOP of the rail (see the module
/// doc's WIN7-5 section) so it leads the rail the way a real Win7/Win10
/// Start Menu's own signed-in-user block does, freeing the true bottom edge
/// for Power alone (lock #11). The old "Console" / "Operations" title block
/// this replaced is gone outright, not relocated — see the module doc for
/// why.
const IDENTITY_H: f32 = Style::SP_XL + Style::SP_S;

/// WIN7-5 — one jump-index row's height: deliberately the SAME value as
/// `start_menu::TILE_H` (restated here, the established per-module idiom —
/// `console.rs` sits lower in this crate's module graph than
/// `start_menu.rs`, which embeds this module, so it cannot import that
/// constant without a cycle). The rail's nav rows now line up in height
/// with the left pane's own tiles: one visual rhythm across the whole
/// Start Menu, not two unrelated panels that happen to share a border.
/// `pub(crate)` (not private) so `start_menu.rs`'s own test suite can pin
/// this cross-module "same value" claim as a real regression check instead
/// of trusting two independently-edited constants to stay in lockstep by
/// eye (the `PANEL_W`/`PANEL_H` cross-module-reuse idiom, restated here for
/// a test-only read rather than a render-path one).
pub(crate) const JUMP_ROW_H: f32 = Style::SP_XL + Style::SP_M;

/// WIN7-5 — the deliberate breathing room between the jump-index and the
/// Power section (lock #11's "anchored bottom," made a real, intentional
/// gap rather than the ~168pt UNaccounted void the WIN7-2 straight-embed
/// migration left here — `IDENTITY_H` + 8×`JUMP_ROW_H` + `POWER_H` leaves
/// exactly one `SP_XL` of the rail's `PANEL_H` unclaimed; this constant is
/// that leftover given a name, a place (`rail`'s layout), and a test
/// (below), instead of being an accident). `#[cfg(test)]`: nothing in the
/// render path reads this value back — `rail`/`power_section` position
/// Power by bottom-relative math (`rail.bottom() - POWER_H`, the robust
/// anchor, never "wherever the jump-index above it happens to end") — so
/// this is verification-only data (the `start_menu.rs`
/// `TILE_GRID_CONTENT_H` `#[cfg(test)]`-on-a-top-level-item idiom).
#[cfg(test)]
const RAIL_SECTION_GAP: f32 = Style::SP_XL;

/// The Custom group's fixed rail-jump-row / list-heading label — named once
/// so the rail's jump row (WIN7-5) and the list's own heading (CONSOLE-4)
/// can never drift into two different strings for the same group.
const CUSTOM_GROUP_LABEL: &str = "Custom";

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

/// CONSOLE-4 — the rail Power section's height (lock #28): a heading + the four
/// action rows; the typed-arming stage renders within the same box, so the rail
/// never reflows while arming.
const POWER_H: f32 = Style::SP_L * 5.0;

/// CONSOLE-4 — one Custom add-form field/button row's height (lock #35).
const FIELD_H: f32 = Style::SP_L;

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
    /// model, design lock "Launch model") through the CONSOLE-2 `spawn_tab`
    /// seam. Root ops embed a leading `sudo` (lock #29), which [`launch_argv`]
    /// routes through [`sudo_argv`] for an interactive PTY prompt. An absent
    /// tool stays honestly greyed — the [`GateReason::ToolMissing`] gate (§7).
    Tab(&'static str),
    /// Route to a shell surface (lock #41's "open the correct GUI surface") —
    /// live NOW through [`ConsoleRequest::Goto`].
    Link(Surface),
    /// Route to a Workbench plane, used for the QUASAR-CLOUD replacement plane.
    Plane(Plane),
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
    /// The row's **domain glyph** (lock #33's "icon"): a distinct brand glyph
    /// per operational domain (System / Network / Storage / Mesh / …), so the
    /// front door reads as a real Start Menu rather than a wall of identical
    /// terminal glyphs. A surface-link entry carries its surface's OWN glyph, so
    /// the iconography stays 1:1 with the surface identity (the entry-table
    /// test pins that invariant).
    icon: IconId,
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
        icon: IconId::Terminal,
        kind: EntryKind::Link(Surface::Terminal),
    },
    ConsoleEntry {
        label: "Monitor",
        desc: "Live per-process CPU / memory / IO (btop)",
        tool: "btop",
        provenance: Provenance::Fedora,
        icon: IconId::System,
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
                icon: IconId::System,
                kind: EntryKind::Tab("btop"),
            },
            ConsoleEntry {
                label: "Services",
                desc: "Unit list — start / stop / restart from it (systemctl)",
                tool: "systemctl",
                provenance: Provenance::Fedora,
                icon: IconId::Settings,
                kind: EntryKind::Tab("systemctl list-units"),
            },
            ConsoleEntry {
                label: "Live Logs",
                desc: "Follow the system journal live (journalctl -f)",
                tool: "journalctl",
                provenance: Provenance::Fedora,
                icon: IconId::Editor,
                kind: EntryKind::Tab("journalctl -f"),
            },
            ConsoleEntry {
                label: "System Dashboard",
                desc: "Live control-group CPU / memory / IO (systemd-cgtop)",
                tool: "systemd-cgtop",
                provenance: Provenance::Fedora,
                icon: IconId::System,
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
                icon: IconId::Signal,
                kind: EntryKind::Tab(
                    "bash -lc 'ip -br addr; echo; ip route; echo; meshctl status'",
                ),
            },
            ConsoleEntry {
                label: "Connections & Ports",
                desc: "Listening + established sockets (ss -tulpn)",
                tool: "ss",
                provenance: Provenance::Fedora,
                icon: IconId::Signal,
                kind: EntryKind::Tab("ss -tulpn"),
            },
            ConsoleEntry {
                label: "Path Test",
                desc: "ICMP path quality to the lighthouse overlay (mtr)",
                tool: "mtr",
                provenance: Provenance::Fedora,
                icon: IconId::Signal,
                kind: EntryKind::Tab("mtr 10.42.0.1"),
            },
            ConsoleEntry {
                label: "Manage Connections",
                desc: "NetworkManager device + connection overview (nmcli)",
                tool: "nmcli",
                provenance: Provenance::Fedora,
                icon: IconId::Settings,
                kind: EntryKind::Tab("nmcli"),
            },
            ConsoleEntry {
                label: "Firewall",
                desc: "Active zone: services, ports, rules (firewall-cmd)",
                tool: "firewall-cmd",
                provenance: Provenance::Fedora,
                icon: IconId::Settings,
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
                icon: IconId::Files,
                kind: EntryKind::Tab("dnf check-update"),
            },
            ConsoleEntry {
                label: "Apply Updates",
                desc: "Upgrade the whole system (sudo dnf upgrade)",
                tool: "dnf",
                provenance: Provenance::Fedora,
                icon: IconId::Files,
                kind: EntryKind::Tab("sudo dnf upgrade"),
            },
            ConsoleEntry {
                label: "Installed Packages",
                desc: "Everything installed, searchable (dnf list)",
                tool: "dnf",
                provenance: Provenance::Fedora,
                icon: IconId::Files,
                kind: EntryKind::Tab("dnf list --installed"),
            },
            ConsoleEntry {
                label: "Platform Update",
                desc: "Update the mesh platform from the signed channel",
                tool: "dnf",
                provenance: Provenance::Quasar,
                icon: IconId::MeshView,
                kind: EntryKind::Tab("sudo dnf upgrade magic-mesh"),
            },
            ConsoleEntry {
                label: "Flatpak",
                desc: "List + update the installed Flatpaks",
                tool: "flatpak",
                provenance: Provenance::Fedora,
                icon: IconId::Files,
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
                icon: IconId::Storage,
                kind: EntryKind::Tab("bash -lc 'df -h; echo; lsblk'"),
            },
            ConsoleEntry {
                label: "Disk Explorer",
                desc: "Interactive disk-usage explorer (ncdu)",
                tool: "ncdu",
                provenance: Provenance::Fedora,
                icon: IconId::Storage,
                kind: EntryKind::Tab("ncdu /"),
            },
            ConsoleEntry {
                label: "Disk Health",
                desc: "SMART health verdict for each disk (smartctl -H)",
                tool: "smartctl",
                provenance: Provenance::Fedora,
                icon: IconId::Storage,
                kind: EntryKind::Tab(
                    "bash -lc 'for d in /dev/sd? /dev/nvme?n1; do [ -e \"$d\" ] && sudo smartctl -H \"$d\"; done'",
                ),
            },
            ConsoleEntry {
                label: "Mesh Storage",
                desc: "The mesh share mount + Syncthing sync status",
                tool: "findmnt",
                provenance: Provenance::Quasar,
                icon: IconId::Storage,
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
                icon: IconId::MeshView,
                kind: EntryKind::Tab("meshctl status"),
            },
            ConsoleEntry {
                label: "Peers",
                desc: "Fleet-wide peer directory (meshctl fleet status)",
                tool: "meshctl",
                provenance: Provenance::Quasar,
                icon: IconId::Node,
                kind: EntryKind::Tab("meshctl fleet status"),
            },
            ConsoleEntry {
                label: "Cloud Status",
                desc: "The state/openstack mirror on the Bus spool",
                tool: "",
                provenance: Provenance::Quasar,
                icon: IconId::MeshView,
                kind: EntryKind::Tab(
                    "bash -lc 'ls -l \"${MDE_BUS_ROOT:-/run/mde-bus}/state/openstack\" 2>/dev/null || echo \"no cloud mirror published on this node\"'",
                ),
            },
            ConsoleEntry {
                label: "Cluster (etcd)",
                desc: "Endpoint health + members (etcdctl)",
                tool: "etcdctl",
                provenance: Provenance::Quasar,
                icon: IconId::Server,
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
                icon: IconId::Instances,
                kind: EntryKind::Tab("podman ps --all"),
            },
            ConsoleEntry {
                label: "Virtual Machines",
                desc: "Every libvirt domain, running or not (virsh)",
                tool: "virsh",
                provenance: Provenance::Fedora,
                icon: IconId::Instances,
                kind: EntryKind::Tab("virsh list --all"),
            },
            ConsoleEntry {
                label: "OpenStack Servers",
                desc: "The cloud's server roster (openstack server list)",
                tool: "openstack",
                provenance: Provenance::Quasar,
                icon: IconId::Server,
                kind: EntryKind::Tab("openstack server list"),
            },
            ConsoleEntry {
                label: "Cloud Plane (GUI)",
                desc: "Open the Workbench Cloud plane — the VM lifecycle GUI",
                tool: "",
                provenance: Provenance::Quasar,
                icon: IconId::Server,
                kind: EntryKind::Plane(Plane::Cloud),
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
                icon: IconId::Terminal,
                kind: EntryKind::Tab("bash -l"),
            },
            ConsoleEntry {
                label: "Root Shell",
                desc: "A root login shell (sudo -i)",
                tool: "sudo",
                provenance: Provenance::Fedora,
                icon: IconId::Terminal,
                kind: EntryKind::Tab("sudo -i"),
            },
            ConsoleEntry {
                label: "tmux",
                desc: "Attach or create the console tmux session",
                tool: "tmux",
                provenance: Provenance::Fedora,
                icon: IconId::Terminal,
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

/// Why an activated command entry could not run — the one honest gate that
/// remains once the launch seam is wired (§7): its tool is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateReason {
    /// The entry's underlying tool is not on this node's `$PATH` — the row greys
    /// and names it, never a dead or a faked entry.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleRequest {
    /// Route the shell body to a surface (a live surface-link entry).
    Goto(Surface),
    /// Route the shell body to a Workbench plane.
    Plane(Plane),
    /// CONSOLE-5 — open a **named terminal tab** running `argv`: the shell
    /// switches to `Surface::Terminal` (lock #7) and drives
    /// `TerminalSurface::spawn_tab` (§6, the deferred-wire idiom — the panel
    /// records this, never reaching the surface itself). `argv` is the typed
    /// program+args [`launch_argv`] built (§9), root ops already `sudo`-wrapped.
    SpawnTab {
        /// The tab's name — the activated entry's label.
        name: String,
        /// The typed program+args to run on the tab's fresh PTY.
        argv: Vec<String>,
    },
    /// CONSOLE-4 — drop the shell curtain (the Power section's Lock; the
    /// in-process lock, exactly like Super+L — NOT logind's session Lock).
    Lock,
    /// CONSOLE-4 — drive a real host power verb (Suspend / Reboot / `PowerOff`)
    /// through `system.honor_power` (§6 — never a raw `systemctl`); the
    /// host-down verbs arrive here only past the typed-arming echo (lock #36).
    Power(PowerVerb),
}

// ── CONSOLE-4: the Power section (lock #28, the VDOCK-4 arming idiom) ───────

/// One rail Power action (lock #28). `Lock` drops the curtain; the rest drive
/// their real [`PowerVerb`]. Reboot + Shut Down are typed-armed (lock #36);
/// Lock + Suspend act on a single click. (The VDOCK-4 `PowerItem` idiom
/// restated — that enum is dock-private and its menu closes with the dock,
/// while these rows live in the Console rail.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerAction {
    /// Drop the shell curtain (the in-process lock).
    Lock,
    /// Suspend-to-RAM — reversible, so no typed arming.
    Suspend,
    /// Reboot the host — typed-armed (lock #36).
    Reboot,
    /// Power the host off — typed-armed (lock #36); the design's "Shut Down".
    ShutDown,
}

/// The Power section's four rows in render order (lock #28).
const POWER_ACTIONS: [PowerAction; 4] = [
    PowerAction::Lock,
    PowerAction::Suspend,
    PowerAction::Reboot,
    PowerAction::ShutDown,
];

impl PowerAction {
    /// The operator-facing label — the typed-arming echo must match it exactly
    /// (case-insensitive).
    const fn label(self) -> &'static str {
        match self {
            Self::Lock => "Lock",
            Self::Suspend => "Suspend",
            Self::Reboot => "Reboot",
            Self::ShutDown => "Shut Down",
        }
    }

    /// Whether this verb demands the typed-arming echo before it fires — the
    /// host-down pair (lock #36); Lock + Suspend act at once.
    const fn typed_armed(self) -> bool {
        matches!(self, Self::Reboot | Self::ShutDown)
    }

    /// The shell request this action fires — every verb the REAL seam: Lock →
    /// the curtain, the rest their logind verb via `system.honor_power` (§6).
    const fn request(self) -> ConsoleRequest {
        match self {
            Self::Lock => ConsoleRequest::Lock,
            Self::Suspend => ConsoleRequest::Power(PowerVerb::Suspend),
            Self::Reboot => ConsoleRequest::Power(PowerVerb::Reboot),
            Self::ShutDown => ConsoleRequest::Power(PowerVerb::PowerOff),
        }
    }
}

/// A host-down verb mid typed-arming: the action + the echo the operator types
/// to arm it (the storage / VDOCK-4 arming-echo idiom).
#[derive(Debug)]
struct Arming {
    /// The action this stage fires once its echo matches.
    action: PowerAction,
    /// The operator-typed echo — must equal [`PowerAction::label`]
    /// (case-insensitive, trimmed) for [`ConsoleState::armed`] to hold.
    echo: String,
}

// ── CONSOLE-4: the Custom group's persisted config (lock #35) ───────────────

/// The Custom config's file name under the client data dir.
const CUSTOM_FILE: &str = "console-custom.json";

/// One operator-registered Custom entry (lock #35): a name + the command line
/// its terminal tab will run once the CONSOLE-2 seam lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomEntry {
    /// The operator's entry name (the row label + the tab's name-to-be).
    pub name: String,
    /// The command line to run.
    pub command: String,
}

/// The persisted Custom store — one JSON file under the client data dir
/// (atomic temp + rename, the timers idiom).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct CustomFile {
    /// The operator's entries, in registration order.
    #[serde(default)]
    entries: Vec<CustomEntry>,
}

impl CustomFile {
    /// Load from `path`, honestly folding a missing / half-written / malformed
    /// file to the empty store (never a fatal, never a fabricated entry).
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write to `path` (atomic temp + rename, like the timers / prefs records).
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

// ── state ────────────────────────────────────────────────────────────────────

/// The Console content's cross-frame state: whether it's showing (mirrored in
/// from the WIN7-2 Start Menu, [`Self::set_open`]), the keyboard focus ring,
/// the rail's pending jump-scroll, the honest gate notice, and the pending
/// shell request. Pure (no egui handles), so the open/close + nav + gate
/// invariants are unit-tested without a GPU.
pub struct ConsoleState {
    /// Whether the content is showing — mirrored in from `start_menu`'s own
    /// open state each frame ([`Self::set_open`]); also still flippable
    /// directly via [`Self::toggle`] for this module's own standalone tests.
    open: bool,
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
    /// The rail identity block's `user@host` (lock #43 — WIN7-5 relocated
    /// the block from a bottom footer to the rail's top; see the module
    /// doc), resolved once.
    identity: String,
    /// The rail identity block's platform version line (lock #43), baked
    /// once.
    version: String,
    /// CONSOLE-4 — a host-down verb mid typed-arming (lock #36); `None` while
    /// the Power section shows its plain rows.
    arming: Option<Arming>,
    /// CONSOLE-4 — the operator's Custom entries (lock #35), loaded from and
    /// persisted to [`Self::store`].
    custom: CustomFile,
    /// The Custom config path (`<client-data-dir>/console-custom.json`);
    /// `None` headless — persistence is then an honest no-op (§7).
    store: Option<PathBuf>,
    /// The Custom add-form's draft name field.
    draft_name: String,
    /// The Custom add-form's draft command field.
    draft_command: String,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self::with_store(mde_bus::client_data_dir().map(|d| d.join(CUSTOM_FILE)))
    }
}

impl ConsoleState {
    /// Build over an explicit Custom store path (the testable constructor
    /// `Default` folds to — the timers `with_roots` idiom): load the operator's
    /// Custom entries, everything else cold. `pub(crate)` — `start_menu`'s own
    /// tests use `with_store(None)` too, for the same deterministic-headless
    /// reason this module's own tests do (never touching a real client data
    /// dir).
    pub(crate) fn with_store(store: Option<PathBuf>) -> Self {
        let custom = store
            .as_deref()
            .map_or_else(CustomFile::default, CustomFile::load_from);
        Self {
            open: false,
            focus: 0,
            focus_moved: false,
            jump: None,
            gate: None,
            pending: None,
            present: Vec::new(),
            identity: identity_line(),
            version: mde_theme::brand::build::version_line(),
            arming: None,
            custom,
            store,
            draft_name: String::new(),
            draft_command: String::new(),
        }
    }

    /// Whether the content is showing.
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// Flip [`Self::open`]. Opening resets the focus ring + notice and
    /// refreshes the `$PATH` presence table. Kept for this module's own
    /// standalone tests (a one-line "open it" primer); production code drives
    /// [`Self::set_open`] instead, mirroring the Start Menu's own open state in.
    pub(crate) fn toggle(&mut self) {
        self.set_open(!self.open);
    }

    /// Mirror an externally-owned open state in (the `DockState::set_active`
    /// idiom) — WIN7-2's Start Menu is the single source of truth for whether
    /// this content shows; a no-op unless the value actually changes, so a
    /// steady "still open" mirror each frame doesn't re-reset the focus ring.
    /// A closed→open edge resets the focus ring + notice and refreshes the
    /// `$PATH` presence table, exactly like the old self-`toggle` did.
    pub(crate) fn set_open(&mut self, open: bool) {
        if open == self.open {
            return;
        }
        self.open = open;
        if self.open {
            self.focus = 0;
            self.focus_moved = false;
            self.jump = None;
            self.gate = None;
            self.arming = None;
            self.refresh_presence();
        }
    }

    /// Close the panel (Esc / click-away / a routed link / a fired power verb).
    /// Drops any in-flight arming — a reopened Console never resumes a stale
    /// half-typed host-down confirm.
    fn close(&mut self) {
        self.open = false;
        self.gate = None;
        self.arming = None;
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

    /// The whole activation ring's length: the static rows plus the operator's
    /// Custom entries (CONSOLE-4), which sit at the flat tail.
    fn rows_total(&self) -> usize {
        total_rows() + self.custom.entries.len()
    }

    /// Activate the flat row `flat` (a click or Enter): a surface link routes +
    /// closes; a command entry opens its **named terminal tab** (CONSOLE-5 — the
    /// front door launches), unless its tool is absent, when it stays honestly
    /// greyed and names the gap (the [`GateReason::ToolMissing`] gate, §7). A
    /// Custom row (the flat tail, CONSOLE-4) launches its operator-owned command
    /// line the same way — the operator owns it, so no `$PATH` check.
    fn activate(&mut self, flat: usize) {
        if flat >= total_rows() {
            if let Some(entry) = self.custom.entries.get(flat - total_rows()) {
                let (name, command) = (entry.name.clone(), entry.command.clone());
                self.launch(name, &command);
            }
            return;
        }
        let entry = entry_at(flat);
        match entry.kind {
            EntryKind::Link(surface) => {
                self.pending = Some(ConsoleRequest::Goto(surface));
                self.close();
            }
            EntryKind::Plane(plane) => {
                self.pending = Some(ConsoleRequest::Plane(plane));
                self.close();
            }
            EntryKind::Tab(cmd) => {
                if self.present.get(flat).copied().unwrap_or(false) {
                    self.launch(entry.label.to_owned(), cmd);
                } else {
                    self.gate = Some(GateNotice {
                        entry: entry.label.to_owned(),
                        reason: GateReason::ToolMissing(entry.tool),
                    });
                }
            }
        }
    }

    /// Record the spawn-tab request that opens `cmd` in a `name`d Terminal tab,
    /// then close: the shell (`main.rs`) drains it, focuses `Surface::Terminal`
    /// and drives `TerminalSurface::spawn_tab` (§6, the deferred-wire idiom —
    /// the panel never reaches the surface). Root ops ride [`sudo_argv`] inside
    /// [`launch_argv`]; a refused spawn is the surface's own honest chip (§7).
    fn launch(&mut self, name: String, cmd: &str) {
        self.pending = Some(ConsoleRequest::SpawnTab {
            name,
            argv: launch_argv(cmd),
        });
        self.close();
    }

    /// CONSOLE-4 — press a rail Power row (lock #28): Lock / Suspend fire their
    /// request at once and close the panel; a host-down verb only ENTERS the
    /// typed-arming stage (lock #36) — nothing fires until the echo matches.
    fn power_press(&mut self, action: PowerAction) {
        if action.typed_armed() {
            self.arming = Some(Arming {
                action,
                echo: String::new(),
            });
        } else {
            self.pending = Some(action.request());
            self.close();
        }
    }

    /// Whether the in-flight arming's echo matches its action's label — the
    /// gate a Reboot / Shut Down confirm must pass (§7 — a blank / mistyped
    /// echo never fires). The VDOCK-4 `PowerMenu::armed` rule.
    fn armed(&self) -> bool {
        self.arming
            .as_ref()
            .is_some_and(|a| a.echo.trim().eq_ignore_ascii_case(a.action.label()))
    }

    /// CONSOLE-4 — fire the armed host-down verb: records its real request and
    /// closes. Refuses (returns `false`, fires NOTHING) unless [`Self::armed`].
    fn confirm_armed(&mut self) -> bool {
        if !self.armed() {
            return false;
        }
        let action = self.arming.as_ref().expect("armed() checked").action;
        self.pending = Some(action.request());
        self.close();
        true
    }

    /// Cancel the typed-arming stage back to the plain Power rows.
    fn cancel_arming(&mut self) {
        self.arming = None;
    }

    /// CONSOLE-4 — register the drafted Custom entry (lock #35): both fields
    /// trimmed non-empty, appended, persisted (atomic), drafts cleared. A blank
    /// draft is refused (`false`) — the Add affordance disables on it too.
    fn add_custom(&mut self) -> bool {
        let name = self.draft_name.trim().to_owned();
        let command = self.draft_command.trim().to_owned();
        if name.is_empty() || command.is_empty() {
            return false;
        }
        self.custom.entries.push(CustomEntry { name, command });
        self.draft_name.clear();
        self.draft_command.clear();
        self.persist_custom();
        true
    }

    /// CONSOLE-4 — unregister a Custom entry by index (persisted); the focus
    /// ring re-clamps so it never points past the shrunken tail.
    fn remove_custom(&mut self, index: usize) {
        if index < self.custom.entries.len() {
            self.custom.entries.remove(index);
            self.persist_custom();
            self.focus = self.focus.min(self.rows_total().saturating_sub(1));
        }
    }

    /// Persist the Custom store (a silent no-op headless — no data dir, §7;
    /// the timers `persist` idiom).
    fn persist_custom(&self) {
        if let Some(path) = self.store.as_deref() {
            let _ = self.custom.save_to(path);
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
    tool_present_in(tool, std::env::var_os("PATH"))
}

/// The [`tool_present`] core against an explicit `PATH` (`""` = always present).
/// Split out so a fixture `PATH` can prove every declared tool resolves without
/// mutating the process-global environment (which would race the test suite).
fn tool_present_in(tool: &str, path: Option<std::ffi::OsString>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if tool.is_empty() {
        return true;
    }
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(tool);
        candidate
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Turn a Console entry's command **line** into the typed program+args
/// [`ConsoleRequest::SpawnTab`] runs (§9). The line is handed to a login shell
/// (`bash -lc <cmd>`) so its shell syntax — the `;`, quotes, globs and `$…` the
/// entries lean on — is honored exactly as written and the `sbin` admin tools
/// resolve on the login `$PATH`.
///
/// A **root op** carries a leading `sudo ` (design lock #29): its shell runs
/// under [`sudo_argv`] (`sudo -- bash -lc …`) so the whole pipeline is elevated
/// and sudo prompts **interactively in the tab's PTY**. A `sudo` that owns its
/// own flag rather than a command — the Root Shell's `sudo -i` — is left
/// verbatim (wrapping it would feed sudo a bogus program name after `--`).
fn launch_argv(cmd: &str) -> Vec<String> {
    if let Some(rest) = cmd.strip_prefix("sudo ") {
        if !rest.trim_start().starts_with('-') {
            return sudo_argv(&shell_lc(rest));
        }
    }
    shell_lc(cmd)
}

/// The `bash -lc <cmd>` login-shell recipe both launch legs share.
fn shell_lc(cmd: &str) -> Vec<String> {
    vec!["bash".to_owned(), "-lc".to_owned(), cmd.to_owned()]
}

/// The rail identity block's `user@host` (lock #43): `$USER` / `$LOGNAME` →
/// `operator` (the backdrop's identity precedence), at this node's hostname
/// (the shared shell resolution — no second hostname idiom). Unchanged by
/// WIN7-5's relocation of the block itself from a bottom footer to the top of
/// the rail — only where the resulting string is painted moved, not how it's
/// built.
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

/// A flat entry row's stable id. `pub(crate)` (the other stable-id functions
/// below stay module-private) — `start_menu`'s own test reads this one back
/// to click a specific row of the embedded content and prove the WIN7-2
/// embedding is reachable end-to-end, not just architecturally wired.
pub(crate) fn console_entry_id(flat: usize) -> egui::Id {
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

/// A rail Power row's stable id (CONSOLE-4).
fn console_power_id(action: PowerAction) -> egui::Id {
    egui::Id::new(("console-power", action.label()))
}

/// The typed-arming echo field's stable id (the one field the stage owns).
fn console_arming_field_id() -> egui::Id {
    egui::Id::new("console-arming-field")
}

/// The arming stage's Confirm row id (fires only once armed, §7).
fn console_confirm_id() -> egui::Id {
    egui::Id::new("console-arming-confirm")
}

/// The arming stage's Cancel row id.
fn console_cancel_id() -> egui::Id {
    egui::Id::new("console-arming-cancel")
}

/// A Custom row's remove-cross id (CONSOLE-4), by entry index.
fn console_custom_remove_id(index: usize) -> egui::Id {
    egui::Id::new(("console-custom-remove", index))
}

/// The Custom add-form's name field id.
fn console_custom_name_id() -> egui::Id {
    egui::Id::new("console-custom-name")
}

/// The Custom add-form's command field id.
fn console_custom_command_id() -> egui::Id {
    egui::Id::new("console-custom-command")
}

/// The Custom add-form's Add row id (disabled on a blank draft).
fn console_custom_add_id() -> egui::Id {
    egui::Id::new("console-custom-add")
}

// ── render ───────────────────────────────────────────────────────────────────

/// Render the Console content into `rect` — the rail|list divider (§4 tokens),
/// the keyboard layer while showing, and the two panes. `pub(crate)`: WIN7-2's
/// `start_menu` calls this directly at its own right-pane rect, having already
/// mirrored its own open state into `state` via [`ConsoleState::set_open`] and
/// painted the OUTER panel chrome (fill/border) itself — this function only
/// owns what's specific to the content's own internal rail|list split, not a
/// second outer frame (no double border). Before WIN7-2 this lived inside a
/// standalone `console_panel` that also mounted its own floating `egui::Area`
/// (slide/click-away/dismiss); that machinery now lives in `start_menu`
/// instead, so embedding this content is a plain rect-scoped call, not a
/// second nested panel.
pub(crate) fn console_content(ui: &mut egui::Ui, rect: egui::Rect, state: &mut ConsoleState) {
    ui.painter().vline(
        rect.left() + RAIL_W,
        (rect.top() + Style::SP_XS)..=(rect.bottom() - Style::SP_XS),
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
    if state.open {
        handle_keys(ui, state);
    }
    rail(ui, rect, state);
    list_pane(ui, rect, state);
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
    let total = state.rows_total();
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

/// The left rail (lock #5, redesigned WIN7-5): the `user@host` / version
/// identity block leading the rail (lock #43 — relocated from a bottom
/// footer, see the module doc), the category jump-index (lock #49) as
/// icon+label+count mini-rows, and the CONSOLE-4 Power section (lock #28)
/// anchored to the rail's true bottom edge (lock #11).
fn rail(ui: &mut egui::Ui, rect: egui::Rect, state: &mut ConsoleState) {
    let painter = ui.painter().clone();
    let rail = egui::Rect::from_min_size(rect.min, egui::vec2(RAIL_W, rect.height()));

    // The identity block (WIN7-5): user@host over the platform version, now
    // the rail's OPENING line rather than a trailing footnote — the
    // authentic Win7/Win10 Start Menu shape (the signed-in user leads the
    // rail; Power alone anchors the bottom, lock #11). `user@host` reads one
    // rung brighter (TEXT_STRONG) than plain body text since it now leads
    // the rail instead of trailing it.
    painter.text(
        egui::pos2(rail.left() + Style::SP_S, rail.top() + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        &state.identity,
        egui::FontId::proportional(Style::BODY),
        Style::TEXT_STRONG,
    );
    painter.text(
        egui::pos2(
            rail.left() + Style::SP_S,
            rail.top() + Style::SP_XS + Style::SP_M,
        ),
        egui::Align2::LEFT_TOP,
        &state.version,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );

    // The category jump-index (lock #49): one row per domain group plus
    // Custom (CONSOLE-4, jump target GROUPS.len()), each now an
    // icon+label+"N entries" mini-row — deliberately the SAME icon+label
    // shape `entry_row` below already uses (a smaller sibling of the rows
    // it jump-scrolls to, not a bare text menu) at `JUMP_ROW_H`, the SAME
    // height as the tile grid's own tiles (see `JUMP_ROW_H`'s doc). A click
    // still just asks the list to jump-scroll — `state.jump`'s index space
    // is UNCHANGED (0..GROUPS.len() for the real groups, GROUPS.len() for
    // Custom), so `list_pane`'s consumption of it below needed no change at
    // all.
    let mut y = rail.top() + IDENTITY_H;
    for (i, group) in GROUPS
        .iter()
        .map(Some)
        .chain(std::iter::once(None))
        .enumerate()
    {
        let row =
            egui::Rect::from_min_size(egui::pos2(rail.left(), y), egui::vec2(RAIL_W, JUMP_ROW_H));
        let (label, icon, count) = group.map_or(
            (
                CUSTOM_GROUP_LABEL,
                IconId::Terminal,
                state.custom.entries.len(),
            ),
            |g| {
                let icon = g.entries.first().map_or(IconId::Settings, |e| e.icon);
                (g.label, icon, g.entries.len())
            },
        );
        let resp = ui.interact(row, console_rail_id(label), egui::Sense::click());
        let hovered = resp.hovered();
        if hovered {
            painter.rect_filled(row, Style::RADIUS, Style::SURFACE_HI);
        }
        // The Custom row wears the SAME Quasar accent every operator-owned
        // entry already tags itself with (`Provenance::Quasar`), at rest —
        // not just on hover — flagging "this category is yours" at a
        // glance; every domain group stays the neutral TEXT/TEXT_DIM pair
        // the rest of this rail (and the tile grid beside it) already uses.
        // The caption always reads dim regardless, matching `entry_row`'s
        // own desc-line-is-always-TEXT_DIM convention below.
        let is_custom = group.is_none();
        let label_color = if is_custom {
            Provenance::Quasar.color()
        } else if hovered {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        };
        if let Some(tex) = icon_texture(ui.ctx(), icon, ENTRY_ICON, label_color) {
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(row.left() + Style::SP_S + ENTRY_ICON / 2.0, row.center().y),
                egui::vec2(ENTRY_ICON, ENTRY_ICON),
            );
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
        }
        let text_left = row.left() + Style::SP_S + ENTRY_ICON + Style::SP_XS;
        painter.text(
            egui::pos2(text_left, row.top() + Style::SP_XS),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(Style::BODY),
            label_color,
        );
        let caption = jump_caption(count);
        painter.text(
            egui::pos2(text_left, row.bottom() - Style::SP_XS),
            egui::Align2::LEFT_BOTTOM,
            &caption,
            egui::FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
        install_row_accessibility(
            ui.ctx(),
            console_jump_accesskit_id(label),
            label,
            caption,
            row,
        );
        if resp.clicked() {
            state.jump = Some(i);
        }
        y += JUMP_ROW_H;
    }

    // CONSOLE-4 — the Power section (lock #28), anchored to the rail's TRUE
    // bottom edge via bottom-relative math (never "whatever falls out of
    // the jump-index above it" — the WIN7-2-era straight embed left an
    // unaccounted ~168pt dead gap here; see the module doc's WIN7-5
    // section and `RAIL_SECTION_GAP`'s own doc). `power_section`'s own
    // hairline at its top edge, right after the deliberate gap above,
    // marks the boundary as intentional — a visibly separate, more careful
    // zone, reinforcing what the DANGER-tinted host-down rows inside it
    // already say.
    power_section(ui, &rail, rail.bottom() - POWER_H, state);
}

/// The jump-index row's dim caption (WIN7-5): how many entries the category
/// holds right now — `GROUPS`' own const-known count for a domain group,
/// [`ConsoleState::custom`]'s live length for Custom (so it tracks an
/// operator add/remove without a stale reading, never a fixed number baked
/// in at open time). Pure + separately tested (the `start_menu.rs`
/// `tile_display_text` idiom) so the plural-vs-singular wording is verified
/// without a GPU.
fn jump_caption(count: usize) -> String {
    format!("{count} entr{}", if count == 1 { "y" } else { "ies" })
}

/// CONSOLE-4 — the rail's **Power section** (lock #28): a micro-heading over
/// the four action rows — or, while a host-down verb arms, the typed-arming
/// stage in the same box (the VDOCK-4 popup idiom laid into the rail). Lock +
/// Suspend fire at once; Reboot + Shut Down read in DANGER and demand the echo
/// (lock #36). Every verb drives the REAL seam through [`ConsoleRequest`].
#[allow(
    clippy::cast_precision_loss, // the 0..4 row indices are tiny
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
fn power_section(ui: &mut egui::Ui, rail: &egui::Rect, top: f32, state: &mut ConsoleState) {
    let painter = ui.painter().clone();
    painter.hline(
        (rail.left() + Style::SP_XS)..=(rail.right() - Style::SP_XS),
        top,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
    painter.text(
        egui::pos2(rail.left() + Style::SP_S, top + Style::SP_L / 2.0),
        egui::Align2::LEFT_CENTER,
        "POWER",
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
    let rows_top = top + Style::SP_L;

    if state.arming.is_some() {
        power_arming_stage(ui, rail, rows_top, state);
        return;
    }

    let mut pressed: Option<PowerAction> = None;
    for (i, &action) in POWER_ACTIONS.iter().enumerate() {
        let row = egui::Rect::from_min_size(
            egui::pos2(rail.left(), rows_top + i as f32 * RAIL_ROW_H),
            egui::vec2(RAIL_W, RAIL_ROW_H),
        );
        let resp = ui.interact(row, console_power_id(action), egui::Sense::click());
        if resp.hovered() {
            painter.rect_filled(row, Style::RADIUS, Style::SURFACE_HI);
        }
        // The host-down pair reads in DANGER (the dock power_row idiom).
        let color = if action.typed_armed() {
            Style::DANGER
        } else if resp.hovered() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        };
        painter.text(
            egui::pos2(row.left() + Style::SP_S, row.center().y),
            egui::Align2::LEFT_CENTER,
            action.label(),
            egui::FontId::proportional(Style::SMALL),
            color,
        );
        // WIN7-5, lock #14 — a screen-reader user needs to know a Reboot/
        // Shut Down press only ARMS a confirmation rather than firing at
        // once (Lock/Suspend act immediately); the visual-only DANGER tint
        // above carried that distinction for sighted users alone before
        // this unit.
        let value = if action.typed_armed() {
            "Requires a typed confirmation before it fires"
        } else {
            "Fires immediately"
        };
        install_row_accessibility(
            ui.ctx(),
            console_power_accesskit_id(action),
            action.label(),
            value,
            row,
        );
        if resp.clicked() {
            pressed = Some(action);
        }
    }
    if let Some(action) = pressed {
        state.power_press(action);
    }
}

/// The Power section's **typed-arming stage** (lock #36): the echo field, a
/// DANGER Confirm that fires ONLY once the echo matches (§7 — disarmed it is inert,
/// painted dim), and Cancel back to the rows. Addressable rows (stable ids), the
/// dock's explicit-rect idiom.
fn power_arming_stage(ui: &mut egui::Ui, rail: &egui::Rect, top: f32, state: &mut ConsoleState) {
    let Some(action) = state.arming.as_ref().map(|a| a.action) else {
        return;
    };
    let painter = ui.painter().clone();
    let inner_l = rail.left() + Style::SP_S;
    let inner_w = RAIL_W - Style::SP_M;
    // The echo field (scoped so the `&mut` on the buffer ends before `armed`).
    let field = egui::Rect::from_min_size(
        egui::pos2(inner_l, top + Style::SP_XS),
        egui::vec2(inner_w, FIELD_H),
    );
    {
        let echo = &mut state.arming.as_mut().expect("arming set above").echo;
        ui.put(
            field,
            egui::TextEdit::singleline(echo)
                .id(console_arming_field_id())
                .hint_text(action.label()),
        );
    }
    let armed = state.armed();

    // Confirm (left) + Cancel (right) — a disarmed Confirm is inert (§7).
    let buttons_top = top + Style::SP_XS + FIELD_H + Style::SP_XS;
    let confirm = egui::Rect::from_min_size(
        egui::pos2(inner_l, buttons_top),
        egui::vec2(inner_w * 0.62, FIELD_H),
    );
    let cancel = egui::Rect::from_min_size(
        egui::pos2(confirm.right() + Style::SP_XS, buttons_top),
        egui::vec2(inner_w - confirm.width() - Style::SP_XS, FIELD_H),
    );
    let confirm_resp = ui.interact(confirm, console_confirm_id(), egui::Sense::click());
    if armed && confirm_resp.hovered() {
        painter.rect_filled(confirm, Style::RADIUS, Style::SURFACE_HI);
    }
    painter.text(
        egui::pos2(confirm.left() + Style::SP_XS, confirm.center().y),
        egui::Align2::LEFT_CENTER,
        format!("Confirm {}", action.label()),
        egui::FontId::proportional(Style::SMALL),
        if armed {
            Style::DANGER
        } else {
            Style::TEXT_DIM
        },
    );
    let confirm_value = if armed {
        format!("Ready \u{2014} fires {}", action.label())
    } else {
        "Disabled until the typed echo matches the action name".to_owned()
    };
    install_row_accessibility(
        ui.ctx(),
        console_confirm_accesskit_id(),
        format!("Confirm {}", action.label()),
        confirm_value,
        confirm,
    );
    let cancel_resp = ui.interact(cancel, console_cancel_id(), egui::Sense::click());
    if cancel_resp.hovered() {
        painter.rect_filled(cancel, Style::RADIUS, Style::SURFACE_HI);
    }
    painter.text(
        egui::pos2(cancel.left() + Style::SP_XS, cancel.center().y),
        egui::Align2::LEFT_CENTER,
        "Cancel",
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    install_row_accessibility(
        ui.ctx(),
        console_cancel_accesskit_id(),
        "Cancel",
        format!("Cancel the {} confirmation", action.label()),
        cancel,
    );
    if armed && confirm_resp.clicked() {
        let _ = state.confirm_armed();
    } else if cancel_resp.clicked() {
        state.cancel_arming();
    }
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
    let mut remove: Option<usize> = None;
    let mut add = false;
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

            // CONSOLE-4 — the Custom group (lock #35): the operator's own
            // command entries at the flat tail (their launch rides the same
            // CONSOLE-2 seam, so activation honest-gates), then the in-UI add
            // form. The rail's Custom cell jumps here (target GROUPS.len()).
            let head = heading(ui, CUSTOM_GROUP_LABEL);
            if state.jump == Some(GROUPS.len()) {
                ui.scroll_to_rect(head, Some(egui::Align::Min));
                state.jump = None;
            }
            for (ci, entry) in state.custom.entries.iter().enumerate() {
                let (clicked, removed) = custom_row(ui, flat + ci, ci, entry, state);
                if clicked {
                    activated = Some(flat + ci);
                }
                if removed {
                    remove = Some(ci);
                }
            }
            if custom_add_form(ui, state) {
                add = true;
            }
        });
    state.focus_moved = false;
    if add {
        let _ = state.add_custom();
    }
    if let Some(ci) = remove {
        state.remove_custom(ci);
    }
    if let Some(flat) = activated {
        state.activate(flat);
    }

    // The honest-gate notice strip (§7): the typed reason a gated activation
    // did not run — always reserved so the list never shifts under a notice.
    // WIN7-5 — also exports a `Live::Polite` accesskit region (lock #14)
    // while it's showing: before this unit the notice was visual-only, so a
    // screen-reader user pressing a greyed command heard nothing explaining
    // why nothing happened. The `install_tiles_live_summary` honesty
    // convention restated: no node at all while there's nothing to say.
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
        let _ = ui
            .ctx()
            .accesskit_node_builder(console_gate_live_region_id(), |node| {
                node.set_role(egui::accesskit::Role::Status);
                node.set_live(egui::accesskit::Live::Polite);
                node.set_label("Console notice");
                node.set_value(gate.text());
                node.set_bounds(accesskit_rect(strip));
            });
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

/// One entry row (lock #33): the row's declared domain glyph (System / Storage
/// / Mesh / Cloud / Signal / … — a surface link wears its surface's own
/// glyph), the label over the one-line description, and the subtle provenance
/// tag (lock #38). An
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

    // The row's domain glyph (lock #33), through the dock's shared cached
    // loader (§6) — each entry declares its own (a surface link wears its
    // surface's glyph), so the list scans by domain, not one blanket icon.
    let icon_id = entry.icon;
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
    // WIN7-5, lock #14 — the row's own accesskit `Button` node: label is the
    // entry's fixed identity, value is the SAME `desc` string (borrowed here,
    // moved into the paint call right below) already on screen — never a
    // second, independently-worded description that could drift from what's
    // painted. `entry_row`'s rows were explicitly NOT covered by WIN7-2's
    // panel-level-only accesskit pass; this is that coverage.
    install_row_accessibility(
        ui.ctx(),
        console_entry_accesskit_id(flat),
        entry.label,
        desc.as_str(),
        rect,
    );
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

/// CONSOLE-4 — one **Custom** row (lock #35): the operator's name over its
/// command line, the Quasar tag (an operator entry is platform-layer config),
/// the remove cross, and the same focus-ring / activation posture as a static
/// row. Returns `(clicked, remove_clicked)` — the cross is its own hit target,
/// registered after the row so it wins the pointer.
fn custom_row(
    ui: &mut egui::Ui,
    flat: usize,
    index: usize,
    entry: &CustomEntry,
    state: &ConsoleState,
) -> (bool, bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    let resp = ui.interact(rect, console_entry_id(flat), egui::Sense::click());
    let hovered = resp.hovered();
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

    // A command entry wears the Terminal front-door glyph, like a static Tab row.
    let tint = if hovered || focused {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Terminal, ENTRY_ICON, tint) {
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

    let text_left = rect.left() + Style::SP_XL;
    painter.text(
        egui::pos2(text_left, rect.top() + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        &entry.name,
        egui::FontId::proportional(Style::BODY),
        Style::TEXT,
    );
    painter.text(
        egui::pos2(text_left, rect.bottom() - Style::SP_XS),
        egui::Align2::LEFT_BOTTOM,
        &entry.command,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
    painter.text(
        egui::pos2(rect.right() - Style::SP_S, rect.top() + Style::SP_XS),
        egui::Align2::RIGHT_TOP,
        Provenance::Quasar.label(),
        egui::FontId::proportional(Style::SMALL),
        Provenance::Quasar.color(),
    );
    // WIN7-5, lock #14 — reuses the SAME flat-index accesskit id space
    // `entry_row` above does (they already share `console_entry_id` for
    // interaction; the "N of a unified activation ring" identity carries
    // over to accesskit too): label = the operator's own name, value = the
    // command it runs, exactly what's painted above.
    install_row_accessibility(
        ui.ctx(),
        console_entry_accesskit_id(flat),
        entry.name.as_str(),
        entry.command.as_str(),
        rect,
    );

    // The remove cross — its own hit target at the row's lower right.
    let cross = egui::Rect::from_center_size(
        egui::pos2(rect.right() - Style::SP_M, rect.bottom() - Style::SP_M),
        egui::vec2(Style::SP_M, Style::SP_M),
    );
    let cross_resp = ui.interact(cross, console_custom_remove_id(index), egui::Sense::click());
    painter.text(
        cross.center(),
        egui::Align2::CENTER_CENTER,
        "\u{2715}",
        egui::FontId::proportional(Style::SMALL),
        if cross_resp.hovered() {
            Style::DANGER
        } else {
            Style::TEXT_DIM
        },
    );
    install_row_accessibility(
        ui.ctx(),
        console_custom_remove_accesskit_id(index),
        format!("Remove {}", entry.name),
        entry.command.as_str(),
        cross,
    );

    (resp.clicked(), cross_resp.clicked())
}

/// CONSOLE-4 — the Custom group's **in-UI add form** (lock #35): a name field,
/// a command field, and the Add row — disabled (dim, inert) until both drafts
/// are non-blank. Returns `true` when Add was pressed with a valid draft (the
/// caller registers + persists).
fn custom_add_form(ui: &mut egui::Ui, state: &mut ConsoleState) -> bool {
    let form_w = ui.available_width();
    let (name_rect, _) = ui.allocate_exact_size(egui::vec2(form_w, FIELD_H), egui::Sense::hover());
    ui.put(
        name_rect.shrink2(egui::vec2(Style::SP_XS, 1.0)),
        egui::TextEdit::singleline(&mut state.draft_name)
            .id(console_custom_name_id())
            .hint_text("Name"),
    );
    let (cmd_rect, _) = ui.allocate_exact_size(egui::vec2(form_w, FIELD_H), egui::Sense::hover());
    ui.put(
        cmd_rect.shrink2(egui::vec2(Style::SP_XS, 1.0)),
        egui::TextEdit::singleline(&mut state.draft_command)
            .id(console_custom_command_id())
            .hint_text("Command"),
    );
    let (add_rect, _) = ui.allocate_exact_size(egui::vec2(form_w, FIELD_H), egui::Sense::hover());
    let can_add = !state.draft_name.trim().is_empty() && !state.draft_command.trim().is_empty();
    let resp = ui.interact(add_rect, console_custom_add_id(), egui::Sense::click());
    if can_add && resp.hovered() {
        ui.painter()
            .rect_filled(add_rect, Style::RADIUS, Style::SURFACE_HI);
    }
    ui.painter().text(
        egui::pos2(add_rect.left() + Style::SP_XS, add_rect.center().y),
        egui::Align2::LEFT_CENTER,
        "+ Add entry",
        egui::FontId::proportional(Style::SMALL),
        if can_add {
            Style::ACCENT
        } else {
            Style::TEXT_DIM
        },
    );
    let add_value = if can_add {
        "Ready to add this entry"
    } else {
        "Enter a name and a command first"
    };
    install_row_accessibility(
        ui.ctx(),
        console_custom_add_accesskit_id(),
        "Add entry",
        add_value,
        add_rect,
    );
    can_add && resp.clicked()
}

// ── accesskit (lock #14, WIN7-5) ─────────────────────────────────────────────
//
// WIN7-2 shipped this module embedded in the Start Menu with only the OUTER
// panel-level accesskit landmarks covering it (`start_menu.rs`'s
// `install_accessibility` exports a `Role::Group` "Console" landmark for the
// whole pane) — every row inside was explicitly flagged as not-yet-covered.
// This unit's redesign is the point every RAW-PAINTED (`ui.interact` + manual
// `Painter` calls, not a real egui widget) interactive row this file draws
// gains its own node: the rail's jump-index rows, entry/custom rows, the
// Power section's action rows, and the arming stage's Confirm/Cancel. The
// real egui `TextEdit` widgets this module already uses (the arming echo
// field, the Custom add-form's two drafts) get accesskit nodes automatically
// from egui's own widget machinery once the `accesskit` feature is enabled —
// they need no manual call here. WIN7-7 remains the crate-wide full sweep;
// this is only the coverage for the rows this unit rewrote the rendering of.

/// Convert an egui rect to an accesskit one (the `status.rs` / `start_menu.rs`
/// helper, restated module-locally — the established per-module-copy idiom).
fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// Install one raw-painted row's accesskit `Button` node: role + a fixed
/// identity label + the row's CURRENT value + bounds + the `Click` action —
/// the SAME shape `status.rs`'s `install_segment_accessibility` /
/// `start_menu.rs`'s `install_tile_accessibility` already use, restated here.
/// Shared by every interactive row in this module (see this section's own
/// banner comment above) so the role/label/value/bounds/action shape can
/// never drift between the rail, the list, and the Power section.
fn install_row_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    label: impl Into<String>,
    value: impl Into<String>,
    rect: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label.into());
        node.set_value(value.into());
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

/// Stable accesskit id for one jump-index row (WIN7-5). Deliberately distinct
/// from [`console_rail_id`] (the SAME `tile_id`/`tile_accesskit_id` split
/// `start_menu.rs` already establishes for its tiles) — interaction ids and
/// accesskit ids stay separate namespaces even when both key off the same
/// label.
fn console_jump_accesskit_id(label: &str) -> egui::Id {
    egui::Id::new(("console-jump-accesskit", label))
}

/// Stable accesskit id for one entry row — a static [`entry_row`] or a
/// [`custom_row`] — keyed by the SAME flat index [`console_entry_id`]
/// already unifies both kinds of row under.
fn console_entry_accesskit_id(flat: usize) -> egui::Id {
    egui::Id::new(("console-entry-accesskit", flat))
}

/// Stable accesskit id for a Custom row's remove cross (WIN7-5).
fn console_custom_remove_accesskit_id(index: usize) -> egui::Id {
    egui::Id::new(("console-custom-remove-accesskit", index))
}

/// Stable accesskit id for one Power action row (WIN7-5).
fn console_power_accesskit_id(action: PowerAction) -> egui::Id {
    egui::Id::new(("console-power-accesskit", action.label()))
}

/// Stable accesskit id for the arming stage's Confirm row (WIN7-5).
fn console_confirm_accesskit_id() -> egui::Id {
    egui::Id::new("console-arming-confirm-accesskit")
}

/// Stable accesskit id for the arming stage's Cancel row (WIN7-5).
fn console_cancel_accesskit_id() -> egui::Id {
    egui::Id::new("console-arming-cancel-accesskit")
}

/// Stable accesskit id for the Custom add-form's Add row (WIN7-5).
fn console_custom_add_accesskit_id() -> egui::Id {
    egui::Id::new("console-custom-add-accesskit")
}

/// Stable accesskit id for the honest-gate notice's live region (WIN7-5,
/// lock #14 — §7's gate notice was visual-only before this unit; a
/// screen-reader user pressing a greyed-out entry had no way to learn WHY
/// nothing happened).
fn console_gate_live_region_id() -> egui::Id {
    egui::Id::new("console-gate-live-region")
}

#[cfg(test)]
mod tests {
    use super::{
        console_confirm_id, console_content, console_entry_id, console_heading_id,
        console_power_id, console_rail_id, entry_at, identity_line, jump_caption, launch_argv,
        static_rows, tool_present, tool_present_in, total_rows, ConsoleRequest, ConsoleState,
        CustomEntry, EntryKind, GateReason, PowerAction, CUSTOM_GROUP_LABEL, GROUPS, PANEL_H,
        PANEL_W, PINNED, POWER_H, RAIL_SECTION_GAP,
    };
    use crate::dock::Surface;
    use crate::workbench::Plane;
    use mde_egui::egui;
    use mde_egui::Style;
    use mde_seat::PowerVerb;

    /// Drive ONE headless frame of the console content over a stand-in surface
    /// (the dock tests' `drive_vdock` idiom — the same `Context::run` path the
    /// DRM runner drives, minus the GPU). Mounts [`console_content`] at the
    /// SAME rect the old standalone `console_panel` used to settle at
    /// (bottom-left, `PANEL_W` × `PANEL_H`) so every existing coordinate-based
    /// row/rail assertion below still lands on the same pixels — this helper
    /// stands in for `start_menu`'s outer Area now that `console.rs` no longer
    /// mounts one itself (WIN7-2). Renders content only while `state.is_open()`,
    /// the closest local analogue to the old Motion-settled visibility gate.
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
            if state.is_open() {
                egui::Area::new(egui::Id::new("test-console-content-area"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(crate::dock::DOCK_W, size.y - PANEL_H))
                    .show(ctx, |ui| {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(PANEL_W, PANEL_H),
                            egui::Sense::hover(),
                        );
                        console_content(ui, rect, state);
                    });
            }
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
                .any(|e| e.kind == EntryKind::Plane(Plane::Cloud)),
            "the Containers & VMs group links to the Cloud plane"
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
    fn every_entry_wears_its_own_domain_glyph_and_links_match_their_surface() {
        // Lock #33 — every row declares its OWN domain glyph (not one blanket
        // terminal icon), and a surface-link entry wears its surface's own
        // glyph so the iconography stays 1:1 with the surface identity.
        let mut glyphs = std::collections::BTreeSet::new();
        for entry in static_rows() {
            if let EntryKind::Link(surface) = entry.kind {
                assert_eq!(
                    entry.icon,
                    surface.icon_id(),
                    "{} links to {surface:?} but wears a different glyph",
                    entry.label,
                );
            }
            glyphs.insert(entry.icon.name());
        }
        // The table spans several distinct domain glyphs — proof it is NOT the
        // old wall of identical terminal icons.
        assert!(
            glyphs.len() >= 6,
            "the entry table should span several domain glyphs, saw {glyphs:?}"
        );
    }

    #[test]
    fn every_declared_tool_resolves_against_a_fixture_path() {
        // The §7 honest gate's positive proof: stage a stub executable for
        // every tool the table declares, then assert every entry resolves
        // present on that fixture $PATH — every entry maps to a REAL,
        // correctly-named command (a typo'd tool would fail here), while an
        // unstaged name stays absent (the greying's ground truth). No global
        // env mutation — the fixture PATH is passed straight to the core.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path();
        let tools: std::collections::BTreeSet<&str> = static_rows()
            .map(|e| e.tool)
            .filter(|t| !t.is_empty())
            .collect();
        for tool in &tools {
            let path = bin.join(tool);
            std::fs::write(&path, "#!/bin/sh\n").expect("write stub");
            let mut perms = std::fs::metadata(&path).expect("stat stub").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod stub");
        }
        let fixture = bin.as_os_str().to_os_string();
        for entry in static_rows() {
            assert!(
                tool_present_in(entry.tool, Some(fixture.clone())),
                "{} tool {:?} did not resolve on the fixture PATH",
                entry.label,
                entry.tool,
            );
        }
        assert!(
            !tool_present_in("mcnf-definitely-absent-xyzzy", Some(fixture)),
            "an unstaged tool must stay absent (the honest gate's ground truth)"
        );
    }

    #[test]
    fn the_containers_and_vms_plane_link_routes_to_the_cloud_plane() {
        // Q41/Q50 — the combined Containers & VMs group carries the surface
        // link that routes to the Cloud plane (a GUI plane), NOT a terminal tab.
        let flat = static_rows()
            .position(|e| e.kind == EntryKind::Plane(Plane::Cloud))
            .expect("the Cloud-plane surface link exists");
        let mut s = ConsoleState::with_store(None);
        s.toggle();
        s.activate(flat);
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::Plane(Plane::Cloud)),
            "the Containers & VMs plane link routes to the Cloud plane"
        );
        assert!(!s.is_open(), "a routed surface link closes the panel");
    }

    #[test]
    fn the_identity_line_reads_user_at_host() {
        // Named for `identity_line()` itself, not its on-screen position —
        // WIN7-5 relocated the rendered identity block from a bottom footer
        // to the rail's top (see the module doc + `IDENTITY_H`'s own doc),
        // so a "footer" name here would now describe a spot this text no
        // longer occupies. The format this test checks is position-agnostic.
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
    fn esc_closes_the_panel() {
        // The panel-level Area/slide/click-away machinery moved to
        // `start_menu` (WIN7-2) — the "mounts no layer while closed" and
        // "click away dismisses" contracts are now tested there, over the
        // real embedding Area. This module keeps the content-level half of
        // Esc: `handle_keys` still calls `ConsoleState::close` on its own
        // `state.open`, which is what `start_menu` reads to propagate the
        // dismissal to the whole panel (see its module doc).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        assert!(s.is_open());
        drive(&ctx, &mut s, vec![key(egui::Key::Escape)], SZ);
        assert!(!s.is_open(), "Esc dismisses the Console (lock #4)");
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
    fn a_present_command_entry_opens_its_named_tab_and_a_missing_one_greys() {
        // CONSOLE-5 — the front door opens: Enter on a present command entry
        // records the SpawnTab request that opens its NAMED tab and closes the
        // panel; a still-missing tool stays honestly greyed (§7, ToolMissing)
        // and launches nothing. Presence is pinned so the verdict is
        // host-independent.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        s.force_presence(1, true); // the pinned Monitor (btop), "installed"
        drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
        drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::SpawnTab {
                name: "Monitor".to_owned(),
                argv: launch_argv("btop"),
            }),
            "the front door opens the entry's named tab running its command",
        );
        assert!(s.gate.is_none(), "a launched entry raises no gate");
        assert!(!s.is_open(), "launching closes the panel and shows the tab");

        // A fresh state with the tool ABSENT: the row greys + names the missing
        // tool, and routes NOTHING (§7 — never a faked launch).
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        s.force_presence(1, false);
        drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
        drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
        assert_eq!(
            s.gate.clone().expect("a missing tool gates").reason,
            GateReason::ToolMissing("btop")
        );
        assert_eq!(s.take_request(), None, "a missing tool launches nothing");
        assert!(s.is_open(), "the panel stays up so the notice is read");
    }

    #[test]
    fn a_root_op_launches_through_the_documented_sudo_argv_path() {
        // Lock #29 — a leading `sudo ` op runs its login shell UNDER sudo
        // (`sudo -- bash -lc …`, the sudo prompts in the tab's PTY); a plain op
        // is just its login shell; a `sudo` owning its own flag (the Root
        // Shell's `sudo -i`) is left verbatim, never fed to sudo as a program.
        let words = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            launch_argv("sudo dnf upgrade"),
            words(&["sudo", "--", "bash", "-lc", "dnf upgrade"]),
        );
        assert_eq!(
            launch_argv("sudo firewall-cmd --list-all"),
            words(&["sudo", "--", "bash", "-lc", "firewall-cmd --list-all"]),
        );
        assert_eq!(launch_argv("btop"), words(&["bash", "-lc", "btop"]));
        assert_eq!(launch_argv("sudo -i"), words(&["bash", "-lc", "sudo -i"]));
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
    //
    // The original single-group ("Shells") jump-scroll proof that used to live
    // here was folded into WIN7-5's
    // `clicking_any_jump_row_scrolls_that_same_groups_heading_up_the_list`
    // below (── WIN7-5 section), which parametrizes over a representative
    // spread of groups INCLUDING Shells — the exact same case, so the
    // original added no coverage beyond what that generalized test already
    // proves. A cross-unit polish pass removed the now-redundant duplicate
    // rather than leave both.

    // ── CONSOLE-4: the Power section (locks #28/#36 — real seams, typed-armed) ──

    #[test]
    fn power_lock_and_suspend_fire_at_once_through_the_real_seams() {
        // Lock → the shell curtain request (NOT a logind verb); Suspend → its
        // real PowerVerb. Both act on a single press and close the panel.
        let mut s = ConsoleState::with_store(None);
        s.toggle();
        s.power_press(PowerAction::Lock);
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::Lock),
            "Lock drops the curtain, not a logind verb"
        );
        assert!(!s.is_open(), "a fired power action closes the panel");
        assert_eq!(s.take_request(), None, "the request drains exactly once");

        let mut s = ConsoleState::with_store(None);
        s.toggle();
        s.power_press(PowerAction::Suspend);
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::Power(PowerVerb::Suspend)),
            "Suspend drives the real seat verb (no arming — reversible)"
        );
        assert!(!s.is_open());
    }

    #[test]
    fn reboot_and_shut_down_demand_the_typed_echo_before_firing() {
        // Lock #36 — the host-down pair fires ONLY past the typed echo: a
        // blank / mistyped echo never arms, a disarmed confirm refuses (§7).
        let mut s = ConsoleState::with_store(None);
        s.toggle();
        s.power_press(PowerAction::Reboot);
        assert!(s.is_open(), "arming keeps the panel up");
        assert_eq!(s.take_request(), None, "entering arming fires NOTHING");
        assert!(!s.armed(), "an empty echo never arms");
        assert!(!s.confirm_armed(), "a disarmed confirm refuses to fire");
        s.arming.as_mut().expect("arming set").echo = "nope".to_owned();
        assert!(!s.armed(), "a mistyped echo never arms");
        s.arming.as_mut().expect("arming set").echo = "reboot".to_owned();
        assert!(s.armed(), "the exact verb name (any case) arms it");
        assert!(s.confirm_armed());
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::Power(PowerVerb::Reboot)),
            "a confirmed Reboot records the real logind verb"
        );
        assert!(!s.is_open(), "firing closes the panel");
        assert!(s.arming.is_none(), "the stage cleared");

        // Shut Down maps to logind PowerOff behind its own echo ("Shut Down").
        let mut s = ConsoleState::with_store(None);
        s.toggle();
        s.power_press(PowerAction::ShutDown);
        s.arming.as_mut().expect("arming set").echo = "shut down".to_owned();
        assert!(s.confirm_armed());
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::Power(PowerVerb::PowerOff)),
            "Shut Down maps to logind PowerOff"
        );

        // Cancel drops the stage without firing; a close drops it too, so a
        // reopened Console never resumes a stale half-typed confirm.
        let mut s = ConsoleState::with_store(None);
        s.toggle();
        s.power_press(PowerAction::ShutDown);
        s.cancel_arming();
        assert!(s.arming.is_none());
        assert_eq!(s.take_request(), None, "a cancelled arming fired nothing");
        s.power_press(PowerAction::Reboot);
        s.close();
        assert!(s.arming.is_none(), "closing drops the in-flight arming");
    }

    #[test]
    fn the_rail_power_rows_dispatch_and_only_an_armed_confirm_fires() {
        // The pointer path: the rail's Lock row fires its request; the Reboot
        // row only ARMS; the Confirm row is inert until the echo matches.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::with_store(None);
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        drive(&ctx, &mut s, Vec::new(), SZ);
        let lock = ctx
            .read_response(console_power_id(PowerAction::Lock))
            .expect("the Lock power row is registered")
            .rect;
        click(&ctx, &mut s, lock.center(), SZ);
        assert_eq!(s.take_request(), Some(ConsoleRequest::Lock));

        let ctx2 = egui::Context::default();
        Style::install(&ctx2);
        let mut s2 = ConsoleState::with_store(None);
        s2.toggle();
        drive(&ctx2, &mut s2, Vec::new(), SZ);
        drive(&ctx2, &mut s2, Vec::new(), SZ);
        let reboot = ctx2
            .read_response(console_power_id(PowerAction::Reboot))
            .expect("the Reboot power row is registered")
            .rect;
        click(&ctx2, &mut s2, reboot.center(), SZ);
        assert!(s2.arming.is_some(), "the Reboot row enters arming");
        assert_eq!(s2.take_request(), None, "the row itself fires nothing");

        // The arming stage mounted in the same box; its DISARMED Confirm is inert.
        drive(&ctx2, &mut s2, Vec::new(), SZ);
        drive(&ctx2, &mut s2, Vec::new(), SZ);
        let confirm = ctx2
            .read_response(console_confirm_id())
            .expect("the Confirm row is registered")
            .rect;
        click(&ctx2, &mut s2, confirm.center(), SZ);
        assert_eq!(
            s2.take_request(),
            None,
            "a disarmed Confirm never fires (§7)"
        );
        assert!(s2.arming.is_some(), "still arming");

        // Arm the echo (the dock tests' direct-echo idiom) — the Confirm fires.
        s2.arming.as_mut().expect("arming set").echo = "Reboot".to_owned();
        drive(&ctx2, &mut s2, Vec::new(), SZ);
        click(&ctx2, &mut s2, confirm.center(), SZ);
        assert_eq!(
            s2.take_request(),
            Some(ConsoleRequest::Power(PowerVerb::Reboot)),
            "the armed Confirm fires the real verb"
        );
        assert!(!s2.is_open(), "firing closed the panel");
    }

    // ── CONSOLE-4: the Custom group (lock #35 — config round-trip + honest gate) ──

    #[test]
    fn custom_entries_round_trip_the_config_and_survive_a_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("console-custom.json");
        let mut s = ConsoleState::with_store(Some(store.clone()));
        assert!(s.custom.entries.is_empty(), "a fresh store starts empty");

        // A blank draft is refused — nothing registered, nothing written.
        assert!(!s.add_custom(), "a blank draft is refused");
        assert!(!store.exists(), "a refused add persists nothing");

        s.draft_name = "Fleet status".to_owned();
        s.draft_command = "meshctl fleet status".to_owned();
        assert!(s.add_custom(), "a full draft registers");
        assert!(
            s.draft_name.is_empty() && s.draft_command.is_empty(),
            "a registered draft clears its fields"
        );
        assert!(
            store.exists(),
            "the add persisted the config (atomic write)"
        );

        // The round trip: a fresh state over the same store loads it back.
        let reloaded = ConsoleState::with_store(Some(store.clone()));
        assert_eq!(
            reloaded.custom.entries,
            vec![CustomEntry {
                name: "Fleet status".to_owned(),
                command: "meshctl fleet status".to_owned(),
            }]
        );

        // Removal persists too.
        let mut s2 = reloaded;
        s2.remove_custom(0);
        assert!(
            ConsoleState::with_store(Some(store.clone()))
                .custom
                .entries
                .is_empty(),
            "a removal persists"
        );

        // A malformed file folds honestly to the empty store (§7).
        std::fs::write(&store, "{not json").expect("write");
        assert!(
            ConsoleState::with_store(Some(store))
                .custom
                .entries
                .is_empty(),
            "a malformed config folds to empty, never a panic or a fake entry"
        );
    }

    #[test]
    fn a_custom_entry_opens_its_own_named_tab() {
        // CONSOLE-5 — a Custom entry's launch rides the SAME spawn-tab seam,
        // opening its own named tab running the operator's command line and
        // closing the panel; the keyboard ring includes the custom tail.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("console-custom.json");
        let mut s = ConsoleState::with_store(Some(store));
        s.draft_name = "Farm top".to_owned();
        s.draft_command = "ssh mm@bigboy btop".to_owned();
        assert!(s.add_custom());
        s.toggle();
        assert_eq!(
            s.rows_total(),
            total_rows() + 1,
            "the activation ring includes the custom tail"
        );
        s.activate(total_rows());
        assert_eq!(
            s.take_request(),
            Some(ConsoleRequest::SpawnTab {
                name: "Farm top".to_owned(),
                argv: launch_argv("ssh mm@bigboy btop"),
            }),
            "a custom entry opens its own named tab running the operator's line",
        );
        assert!(s.gate.is_none(), "a launched custom entry raises no gate");
        assert!(!s.is_open(), "launching a custom entry closes the panel");
    }

    // ── WIN7-5: the redesigned right pane (locks #10/#11/#14) ────────────────

    #[test]
    fn jump_caption_reads_singular_for_exactly_one_and_plural_otherwise() {
        assert_eq!(jump_caption(0), "0 entries");
        assert_eq!(jump_caption(1), "1 entry");
        assert_eq!(jump_caption(4), "4 entries");
    }

    #[test]
    fn the_power_section_is_flush_with_the_panes_true_bottom_edge() {
        // Lock #11 — Power anchors the right pane's TRUE bottom, not
        // "wherever the jump-index above it happens to end" (the WIN7-2-era
        // straight-embed bug this unit fixed — see the module doc). The
        // test harness (`drive`, above) mounts `console_content`'s rect
        // flush with the screen bottom, so the pane's real bottom edge is
        // exactly `SZ.y`.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        drive(&ctx, &mut s, Vec::new(), SZ);
        let shutdown = ctx
            .read_response(console_power_id(PowerAction::ShutDown))
            .expect("the Shut Down power row is registered")
            .rect;
        assert!(
            (shutdown.bottom() - SZ.y).abs() < 0.5,
            "the last Power row must sit flush with the pane's true bottom \
             edge: got {} vs {}",
            shutdown.bottom(),
            SZ.y,
        );
    }

    #[test]
    fn the_gap_between_the_jump_index_and_power_is_small_not_the_old_dead_void() {
        // The WIN7-2-era straight embed left an unaccounted ~168pt dead gap
        // between the jump-index and the Power section (a big blank void
        // with nothing marking it as deliberate) — this pins the fix as a
        // real, regression-tested invariant instead of trusting it stays
        // fixed by eye. `RAIL_SECTION_GAP` (32pt) replaces it.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        drive(&ctx, &mut s, Vec::new(), SZ);
        let last_jump_row = ctx
            .read_response(console_rail_id(CUSTOM_GROUP_LABEL))
            .expect("the Custom jump row is registered")
            .rect;
        let power_section_top = SZ.y - POWER_H;
        let gap = power_section_top - last_jump_row.bottom();
        assert!(
            gap >= 0.0,
            "the jump-index must not overlap the Power section: gap {gap}"
        );
        assert!(
            gap <= RAIL_SECTION_GAP + 0.5,
            "the gap between the jump-index and Power must be the one \
             deliberate RAIL_SECTION_GAP, not a large accidental void: got {gap}"
        );
    }

    #[test]
    fn clicking_any_jump_row_scrolls_that_same_groups_heading_up_the_list() {
        // Supersedes the original Shells-only jump-scroll proof (folded in
        // here, see the section banner above): the WIN7-5 rewrite touches
        // every jump row's paint path, so this proves the click-to-
        // scroll-target mapping wasn't disturbed for OTHER groups too, not
        // just Shells. A representative spread (first, middle, last group)
        // rather than all seven, to keep the test focused.
        for label in ["Network", "Storage", "Shells"] {
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let mut s = ConsoleState::default();
            s.toggle();
            drive(&ctx, &mut s, Vec::new(), SZ);
            drive(&ctx, &mut s, Vec::new(), SZ);

            let before = ctx
                .read_response(console_heading_id(label))
                .unwrap_or_else(|| panic!("{label} heading is registered"))
                .rect
                .top();
            let rail_row = ctx
                .read_response(console_rail_id(label))
                .unwrap_or_else(|| panic!("{label} rail cell is registered"))
                .rect;
            click(&ctx, &mut s, rail_row.center(), SZ);
            for _ in 0..6 {
                drive(&ctx, &mut s, Vec::new(), SZ);
            }
            let after = ctx
                .read_response(console_heading_id(label))
                .unwrap_or_else(|| panic!("{label} heading is still registered"))
                .rect
                .top();
            assert!(
                after < before - Style::SP_XL,
                "{label}: the jump must scroll ITS OWN group up the pane \
                 (before {before}, after {after})"
            );
        }
    }

    #[test]
    fn every_jump_row_reports_its_real_group_size_and_tracks_custom_live() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        let out = drive(&ctx, &mut s, Vec::new(), SZ);
        let nodes = out
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();

        for group in &GROUPS {
            let node = nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.label() == Some(group.label))
                .unwrap_or_else(|| panic!("{} jump row exports no accesskit node", group.label));
            assert_eq!(node.role(), egui::accesskit::Role::Button);
            let expected = jump_caption(group.entries.len());
            assert_eq!(
                node.value(),
                Some(expected.as_str()),
                "{}'s jump row must report its real entry count",
                group.label
            );
        }

        // Custom starts empty on a fresh store...
        let custom = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(CUSTOM_GROUP_LABEL))
            .expect("the Custom jump row exports an accesskit node");
        assert_eq!(custom.value(), Some("0 entries"));
    }

    #[test]
    fn adding_a_custom_entry_updates_the_customs_jump_row_count_live() {
        // Not a fixed number baked in at open time — `state.custom`'s real,
        // live length, re-read every frame.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("console-custom.json");
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = ConsoleState::with_store(Some(store));
        s.draft_name = "Fleet status".to_owned();
        s.draft_command = "meshctl fleet status".to_owned();
        assert!(s.add_custom());
        s.toggle();
        let out = drive(&ctx, &mut s, Vec::new(), SZ);
        let nodes = out
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();
        let custom = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(CUSTOM_GROUP_LABEL))
            .expect("the Custom jump row exports an accesskit node");
        assert_eq!(custom.value(), Some("1 entry"));
    }

    #[test]
    fn every_row_this_unit_touched_exports_a_clickable_button_accesskit_node() {
        // Lock #14 — WIN7-2 shipped this whole module's embedding with only
        // panel-level accesskit landmarks (`start_menu.rs`'s
        // `install_accessibility`); individual rows were explicitly flagged
        // as not-yet-covered. Proves every raw-painted interactive row this
        // unit rewrote now exports a real Button node: the eight jump rows,
        // the pinned Terminal entry row, and the four Power action rows.
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        let out = drive(&ctx, &mut s, Vec::new(), SZ);
        let nodes = out
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();

        let mut expect_labels: Vec<&str> = GROUPS.iter().map(|g| g.label).collect();
        expect_labels.push(CUSTOM_GROUP_LABEL);
        expect_labels.push("Terminal");
        expect_labels.push(PowerAction::Lock.label());
        expect_labels.push(PowerAction::Suspend.label());
        expect_labels.push(PowerAction::Reboot.label());
        expect_labels.push(PowerAction::ShutDown.label());

        for label in expect_labels {
            let node = nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.label() == Some(label))
                .unwrap_or_else(|| panic!("{label} exports no accesskit node"));
            assert_eq!(node.role(), egui::accesskit::Role::Button, "{label}'s role");
        }
    }

    #[test]
    fn a_custom_rows_accesskit_carries_its_real_name_and_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("console-custom.json");
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = ConsoleState::with_store(Some(store));
        s.draft_name = "Fleet status".to_owned();
        s.draft_command = "meshctl fleet status".to_owned();
        assert!(s.add_custom());
        s.toggle();
        let out = drive(&ctx, &mut s, Vec::new(), SZ);
        let nodes = out
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();

        let row = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Fleet status"))
            .expect("the custom row exports an accesskit node");
        assert_eq!(row.role(), egui::accesskit::Role::Button);
        assert_eq!(row.value(), Some("meshctl fleet status"));

        let remove = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Remove Fleet status"))
            .expect("the remove cross exports an accesskit node");
        assert_eq!(remove.role(), egui::accesskit::Role::Button);
    }

    #[test]
    fn the_gate_notice_exports_a_live_polite_region_only_while_showing() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        let out0 = drive(&ctx, &mut s, Vec::new(), SZ);
        let nodes0 = out0
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();
        assert!(
            !nodes0
                .iter()
                .any(|(_, n)| n.label() == Some("Console notice")),
            "no gate has fired yet — no live region should export"
        );

        // Force the pinned Monitor (btop) ABSENT, activate it, and confirm
        // the resulting gate notice (§7) is now announced — it was
        // visual-only before this unit, so a screen-reader user pressing a
        // greyed row heard nothing explaining why.
        s.force_presence(1, false);
        drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
        let out1 = drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
        assert!(s.gate.is_some(), "the gate should have fired");
        let nodes1 = out1
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();
        let notice = nodes1
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Console notice"))
            .expect("the gate notice must export a live region while showing");
        assert_eq!(notice.role(), egui::accesskit::Role::Status);
        assert_eq!(notice.live(), Some(egui::accesskit::Live::Polite));
        assert!(
            notice.value().unwrap_or_default().contains("Monitor"),
            "the announced text must name the gated entry: {:?}",
            notice.value()
        );
    }

    #[test]
    fn the_confirm_and_cancel_rows_report_their_armed_state_via_accesskit_value() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = ConsoleState::with_store(None);
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        s.power_press(PowerAction::Reboot);
        let out0 = drive(&ctx, &mut s, Vec::new(), SZ);
        let nodes0 = out0
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();
        let confirm0 = nodes0
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Confirm Reboot"))
            .expect("the Confirm row exports an accesskit node while arming");
        assert_eq!(confirm0.role(), egui::accesskit::Role::Button);
        assert!(
            confirm0.value().unwrap_or_default().contains("Disabled"),
            "a disarmed Confirm's value must say so: {:?}",
            confirm0.value()
        );
        let cancel0 = nodes0
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Cancel"))
            .expect("the Cancel row exports an accesskit node");
        assert_eq!(cancel0.role(), egui::accesskit::Role::Button);

        // Arm the echo — the Confirm row's value flips to "ready."
        s.arming.as_mut().expect("arming set").echo = "Reboot".to_owned();
        let out1 = drive(&ctx, &mut s, Vec::new(), SZ);
        let nodes1 = out1
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone();
        let confirm1 = nodes1
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Confirm Reboot"))
            .expect("the Confirm row still exports a node once armed");
        assert!(
            confirm1.value().unwrap_or_default().contains("Ready"),
            "an armed Confirm's value must say so: {:?}",
            confirm1.value()
        );
    }
}
