# WL-FUNC-032 leftover honesty — Seat 15 live hotkeys probe — r1

Date: 2026-08-24  
Classification: leftover-honesty / live-seat read-only probe; **not**
live-surface Ctrl+J / Ctrl+N keystroke proof, **not** keystroke injection,
**not** `production_admitted`  
Source revision (control tree): `7fe8fad6ccc8`  
Installed identity: `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
Seat: Seat 15 `172.20.0.15` (`Basement-Test-Workstation`)  
Observed: `2026-08-24T15:47:15Z`–`2026-08-24T15:48:21Z`  
`production_admitted: false`

Read-only SSH from the control host (`mm@`,
`/root/.ssh/mackes_mesh_ed25519`). `sudo -n` used only to list who holds
`/dev/dri/card1`. No package install, no enroll, no `systemctl` mutate, no
dest invented, no `seat-remote-input` / uinput injection, no Sunshine start.

In-tree `crates/desktop/mde-seat/src/hotkeys.rs` and
`crates/desktop/mde-shell-egui/src/hotkeys.rs` already catalog Ctrl+J /
Ctrl+N and refuse Documents / Terminal / Desktop / Browser text-or-guest
focus. `git diff 7e3474ee HEAD --` those two files is empty, so the
installed RPM revision carries the same table. Prior Dell catalog:
`WL-FUNC-032-2026-08-23-installed-hotkeys-catalog-r1.md`.

## Observed (Seat 15)

| Field | Value |
|---|---|
| hostname | `Basement-Test-Workstation` |
| RPM | `magic-mesh-13.0.0-35.x86_64` |
| `mackesd --version` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` |
| `/usr/bin/mde-shell-egui` | 60819776 bytes, mtime `2026-08-22 21:56:45` (local) |
| sha256 | `faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14` |
| `rpm -qf` | `magic-mesh-13.0.0-35.x86_64` |
| `rpm -V` `mde-shell-egui` | unmodified (no verify line) |
| `mde-shell-egui.service` | `active` / `running` since `2026-08-23 10:27:50 EDT` |
| pid | `2353` as `root`, elapsed ~1d 1h at probe |

Packed catalog literals in the installed shell binary (byte-neighborhood
extract, not `strings` line-split):

```text
…XF86Bluetooth Super+Tab Super+grave Super+Escape Super+l Super+s Super+Space Ctrl+J Ctrl+N…
…Open System panel Open omnibox Open Transfers New transfer…
```

Exact byte counts in `/usr/bin/mde-shell-egui`:

| Literal | Count | Role |
|---|---|---|
| `Ctrl+J` | 2 | catalog + apply-site curtain list |
| `Ctrl+N` | 5 | catalog + curtain + bookmark/Transfers chrome copy |
| `Open Transfers` | 1 | `HotkeyAction::OpenTransfers` label |
| `New transfer` | 3 | action label + Transfers editor chrome |
| `Ctrl+K` | 0 | no invented extra Transfers chord |

Same counts as the Dell catalog record. Construct holds the DRM seat:
`lsof /dev/dri/card1` shows `mde-shell` pid `2353` with five open fds on
`card1`. `loginctl` session `1` is class `manager` with empty `Seat` /
`Display`; seat0 `CanGraphical=yes` but empty `ActiveSession` / `Sessions`.
That is the MCNF DRM-service model, not an absent GUI.

Physical input present: Keychron K6 (`/dev/input/event4`, `event5`) and a
PixArt Dell MS116 mouse. Operator dock actions today (journal
`mde_shell_egui::nav_bar`, 2026-08-24 07:24 EDT) opened Remote Sessions,
Infra as Code, Mesh Teams (`Surface::Communications`), and Maps & Location.
Last docked surface: Maps & Location, expanded, `2026-08-24T11:24:27Z`.
No curtain/lock journal lines. Current surface eight hours later is not
readable from outside the process.

`journalctl` for the last 7 days has no `OpenTransfers` / `Ctrl+J` /
hotkey-transfer lines. Dock `open_surface` / `open_editor` / `home` are
audited; `apply_hotkey(OpenTransfers)` is not. Mesh Teams was opened from
the dock, not from Ctrl+J. That is not a refuse of the binding.

Capture dests on this seat:

| Path | State |
|---|---|
| `/usr/bin/sunshine` | present; `sunshine.service` not-found / inactive; no process |
| grim | absent |
| Moonlight | absent |
| `/usr/bin/ffmpeg` | present |
| `/usr/libexec/mackesd/seat-remote-input` | present; `--help` only |
| `/dev/uinput` | present (`root:input`) |

Starting Sunshine or injecting Ctrl+J would invent a dest. The helper was
not invoked. Root Construct fds are not readable as `mm`.

## Non-claims

- Ctrl+J / Ctrl+N were not pressed on a live Construct surface.
- Transfers mode was not observed opening from Files, Workers, Music, Maps,
  Mesh Teams, or any other surface.
- In-mode New Transfer was not observed.
- Documents / Terminal / Desktop / Browser refuse was not observed live.
- `production_admitted` was not flipped.
- No dest was invented.

## Leftover / blocker

Seat 15 **does** list Ctrl+J / Ctrl+N in the compiled hotkeys catalog on a
used DRM Construct (`13.0.0-35`, same `hotkeys.rs` as HEAD). Catalog
presence and dock-journal are not live-surface keystroke proof.

The leftover is still a real Construct key press: Ctrl+J opening
Communications Transfers from every surface, in-mode Ctrl+N starting a new
transfer, and the catalog refuse holding on Documents / Terminal / Desktop /
Browser text-or-guest focus. Closing that leftover needs a capture dest or
an operator keystroke that observably lands Transfers. Injecting uinput
without a frame record would mutate the seat and still not close S1.
