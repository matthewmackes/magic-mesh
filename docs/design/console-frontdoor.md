# CONSOLE — the Terminal's Start-Menu front door

> **HISTORICAL / SUPERSEDED (2026-07-22):** the Start-menu/bottom-rail design is
> retired by `docs/design/platform-interfaces.md`; its OpenStack command entries
> are also retired by the zero-OpenStack cutover.

Operator-locked 2026-07-04 (50-Q `/plan` survey). A Carbon-styled **Start Menu that is
the Front Door for the Terminal**: the bottom rail's far-left Start/Advanced icon opens a
Win10-style panel of **operational entries**, each of which launches a TUI/CLI op as a
**new tab in the terminal emulator** (`mde-term-egui`, which already has tabs). The menu
title is **"Console" / "Operations."**

Grounded in a live evaluation of Eagle (a real install): present tools are **btop**
(not htop), systemctl/journalctl/systemd-cgtop, **nmcli/ip/ss/mtr** (no nmtui),
dnf/dnf5/rpm/flatpak, lsblk/df/smartctl (no ncdu), podman/virsh/cockpit-bridge, the full
mesh stack (mackesd/meshctl/nebula/syncthing/etcdctl), tmux/nano. The platform adds btop,
mtr, tmux, ncdu (to bundle), and the entire mesh layer beyond stock Fedora Server.

## Locked decisions (50)

| # | Area | Lock |
|---|------|------|
| 1 | Trigger | Start button **only** (Super stays the shell leader key) |
| 2 | Button | Far-left of the bottom rail, before Desktop; **icon = the Start/Advanced tray glyph** |
| 3 | Shape | Bottom-left panel, rises from the button (Win10 footprint) |
| 4 | Dismiss | Click-away + Esc + pressing the button again |
| — | Launch model | Each entry opens a **new tab in the terminal emulator** (`mde-term-egui::new_tab`) |
| 5 | Layout | Win10 two-pane: left rail (categories + power/session), right = pinned + full list |
| 6 | Groups | By domain: System / Network / Packages / Storage / Mesh / Containers&VMs / Shells / Power |
| 7 | On launch | Switch to the Terminal surface + focus the new tab |
| 8 | Tab life | Named tab, **stays open on exit** (shows output + a prompt; close manually) |
| 9 | Monitor | **btop** (the rich present TUI) |
| 10 | Services | `systemctl list-units` interactive (start/stop/restart from there) |
| 11 | Logs | `journalctl -f` live follow |
| 12 | Sysinfo | A **live dashboard** (refreshing status), not a one-shot |
| 13 | Net status | A **mesh-aware summary** (ip + route + DNS + Nebula overlay + mesh reachability) |
| 14 | Sockets | `ss -tulpn` listening + established |
| 15 | Path test | **mtr** to a prompted/default target |
| 16 | Net manage | Interactive **nmcli** (nmtui absent) |
| 17 | Update | **Two entries** — Check (`dnf check-update`) then Apply (`dnf upgrade`) |
| 18 | Installed | `dnf list installed`, paged/searchable |
| 19 | Platform update | Yes — mesh-aware `dnf upgrade magic-mesh` from the signed channel (distinct entry) |
| 20 | Flatpak | A `flatpak update` + `flatpak list` entry |
| 21 | Disk use | df + lsblk overview + **bundle ncdu** (interactive explorer) |
| 22 | Disk health | Yes — `smartctl -H` SMART summary |
| 23 | Mesh storage | Yes — Syncthing/share (`/mnt/mesh-storage`) mount + sync status |
| 24 | Mesh status | `meshctl status` roll-up (overlay/peers/lighthouse/role) |
| 25 | Peers | Peer directory roll-up (name/role/overlay/online/last-seen) |
| 26 | Cloud | Yes — the openstack `state/openstack/<node>` mirror status |
| 27 | Etcd | Yes — `etcdctl endpoint health` + members |
| 28 | Power | Yes — Lock / Suspend / Reboot / Shutdown (real systemctl/loginctl; destructive armed) |
| 29 | Elevation | **sudo in the tab**, password prompt (non-root ops run as the seat user) |
| 30 | Search | **No search box** (the grouped list is short enough) |
| 31 | Pinned | Just a plain **Terminal + Monitor** pinned |
| 32 | Recents | **No** recent/frequent tracking |
| 33 | Entry row | Icon + label + a **one-line description** of what it runs |
| 34 | Local notify | **Yes — local system events → the Chat/notify area** (fixes the empty-Chat bug) |
| 35 | Custom | Yes — operator can add their own command entries (a Custom group) |
| 36 | Confirm | **Typed arming** on destructive ops (reboot/shutdown/service-stop) |
| 37 | Shells | **Multiple**: user shell, root shell (`sudo -i`), tmux session |
| 38 | Provenance | A subtle per-entry **Fedora vs Construct** tag |
| 39 | Name | Title **"Console" / "Operations"**; the button is icon-only (terminal glyph) |
| 40 | Keyboard | Full arrow-key nav + Enter; Esc closes |
| 41 | Containers/VMs | Yes — a combined group (per Q50): **podman + virsh + OpenStack** ops together, **with a link to the correct GUI surface** (Instances/Cloud plane) |
| 42 | Firewall | Yes — `firewall-cmd --list-all` status/zones |
| 43 | Footer | user@host · platform version · Power button (Win10 corner) |
| 44 | Motion | Slide up from the button (~200ms Carbon Motion) |
| 45 | MVP | **Everything at once** — all groups + custom + the local-notify producer in the first cut |
| 46 | Notify events | peer join/leave · updates available · service failed/degraded · disk-low/SMART · **journal WARN-or-above** |
| 47 | Notify history | **Yes — a scrollable, timestamped feed in Chat** (badge counts unread) |
| 48 | Accessibility | Large-type + focus ring (reuse EXPLORER-18), honor text-scale |
| 49 | Rail nav | Clicking a category **jump-scrolls** to that group (the right pane shows all) |
| 50 | Entry set | Approved; Containers/VMs/OpenStack combined with a surface link (see #41) |

## The entry set (proposed, grounded in real tools)

- **System:** Resource Monitor (btop) · Services (systemctl) · Live Logs (journalctl -f) · System Dashboard (live)
- **Network:** Network Status (mesh-aware) · Connections/Ports (ss) · Path Test (mtr) · Manage Connections (nmcli) · Firewall (firewall-cmd)
- **Packages:** Check Updates · Apply Updates (dnf) · Installed (dnf list) · Platform Update (magic-mesh channel) · Flatpak
- **Storage:** Disk Usage (df/lsblk/ncdu) · Disk Health (smartctl) · Mesh Storage (Syncthing)
- **Mesh:** Mesh Status (meshctl) · Peers · Cloud Status (openstack mirror) · Cluster/etcd
- **Containers & VMs:** podman ps · virsh list · OpenStack ops — **+ a "Open in Cloud plane" surface link**
- **Shells:** User shell · Root shell (sudo -i) · tmux
- **Power:** Lock · Suspend · Reboot(armed) · Shut Down(armed)
- **Custom:** operator-defined command entries

## Architecture

- **Shell (`mde-shell-egui`):** a new `console.rs` (the Start-Menu panel) + a Start button
  in `dock.rs` (far-left bottom rail, Start/Advanced tray glyph) that toggles it. The panel is a Win10 two-pane
  `Area` rising from the button (slide-up Motion); left rail = category jump-index +
  power/session + footer (user@host·version); right = pinned (Terminal+Monitor) + the
  grouped entry list; each entry = icon + label + one-line desc + a Fedora/Construct tag;
  full arrow-key nav + focus ring (EXPLORER-18 posture). Selecting an entry closes the
  menu, switches to `Surface::Terminal`, and calls the terminal's `new_tab()` seam with
  the entry's command (named tab, stays-open-on-exit); root ops run under `sudo` in the
  tab; destructive power ops use the platform typed-arming confirm; the Containers&VMs
  surface-link routes to `Surface::Instances`/the Cloud plane.
- **Entry model:** a const table `ConsoleEntry { group, label, desc, provenance, command,
  needs_root, kind: TerminalTab | PowerAction | SurfaceLink }` + an operator custom-entry
  config. No dead entries — every command is a real tool present on the node (or bundled:
  ncdu/nmtui if locked); a missing tool → the entry is honestly hidden/greyed (§7).
- **Terminal seam:** `mde-term-egui` already exposes tabs (`TabbedTerminal`/`new_tab()`);
  Console calls it to open a named tab running the command. If a "run this command in a
  new named tab" entrypoint isn't directly reachable from the shell, add a thin one on the
  terminal surface (a `spawn_tab(name, argv)` seam) — glue, not reimplementation (§6).
- **Local-notification producer (`mackesd`):** a new `notify` worker that watches the
  event sources — mesh peer directory (join/leave), dnf/platform update availability,
  `systemctl --failed` + mesh/cloud health, disk usage + SMART, and `journalctl` at
  WARN+ — and emits typed notifications onto the bus; the Chat surface renders them as a
  scrollable timestamped feed (newest first) + the tray badge counts unread. **This also
  fixes the empty-Chat bug** — see the two chat fixes below.

## The Chat bug (found live on Eagle, this session)

Two defects, both real:
1. **Chat worker never runs** — `bin/mackesd.rs` spawns the ChatWorker only
   `if worker_role::runs("chat", role_rank)`, but **"chat" is not in the `WORKER_TIERS`
   census table**, so `runs()` returns the unknown-worker default and the worker never
   starts (the BUG-STORAGE-1 class). Fix: add `("chat", <rank>)` to `WORKER_TIERS`
   (universal/rank-0 — chat should run on every node).
2. **No local-notification producer** — even with the chat worker running, the surface is
   empty absent peer messages; the operator expects the *node itself* to talk to them. The
   `notify` worker (above) fills this.

## Acceptance (runtime-observable)
- The far-left bottom-rail Start/Advanced icon opens the Console panel (slide-up); Esc/click-away/re-click close it.
- Every group renders its entries (icon + desc + provenance tag); arrow keys navigate with a visible focus ring.
- Selecting an entry switches to the Terminal and opens a **named tab running the real command** (btop actually launches, dnf actually runs, mtr actually probes); root ops sudo-prompt in the tab; the tab stays open on exit.
- Destructive power ops require typed arming; the Containers&VMs surface link opens the Cloud plane.
- Local system events (peer join/leave, updates, failed service, disk/SMART, journal WARN+) appear as a timestamped feed in Chat + bump the tray badge — Chat is no longer empty.
- All Carbon tokens (§4); the chat worker runs on Eagle (census fix).

## Risks
- **Terminal `spawn_tab` seam** — confirm the shell can drive `mde-term-egui` to open a named tab with a given command; add the thin seam if absent.
- **Notify worker event sources** — several are poll-based (dnf check, df, journal); bound them (don't hammer). journal WARN+ can be noisy → rate-limit/coalesce.
- **Elevation UX** — sudo-in-tab needs the tab to be interactive for the password; ensure the PTY is wired before the command runs.
- **`sudo` availability + policy** — the seat user must be a sudoer; document.

## Out of scope
- A full graphical package manager / service manager (the entries are terminal ops).
- Remote-node ops from the menu (this is the local node's front door).

## Tasks → `docs/WORKLIST.md` CONSOLE-1..N + CHAT-FIX-1/2.
