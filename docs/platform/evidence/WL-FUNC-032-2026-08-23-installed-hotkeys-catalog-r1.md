# WL-FUNC-032 leftover honesty — installed hotkeys catalog lists Ctrl+J — r1

Date: 2026-08-23  
Classification: leftover-honesty / installed catalog presence; **not**
live-surface Ctrl+J proof, **not** keystroke injection, **not**
`production_admitted`  
Source revision (control tree): `f5362d86545a`  
Installed identity: `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
Seat: Dell `172.20.146.225` (`DELL-LAPTOP`)  
Observed: `2026-08-24T00:03:16Z`  
`production_admitted: false`

Read-only SSH from the control host (`mm@`,
`/root/.ssh/mackes_mesh_ed25519`). No package install, no enroll, no
`systemctl` mutate, no dest invented, no keystroke injection.

In-tree `crates/desktop/mde-seat/src/hotkeys.rs` and
`crates/desktop/mde-shell-egui/src/hotkeys.rs` already catalog Ctrl+J /
Ctrl+N and refuse Documents / Terminal / Desktop / Browser text-or-guest
focus. `git diff 7e3474ee HEAD --` those two files is empty, so the
installed RPM revision carries the same table.

## Observed (Dell only)

| Field | Value |
|---|---|
| hostname | `DELL-LAPTOP` |
| RPM | `magic-mesh-13.0.0-35.x86_64` |
| `mackesd --version` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` |
| `/usr/bin/mde-shell-egui` | 60819776 bytes, mtime `2026-08-22 21:56:45 -0400` |
| sha256 | `faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14` |
| `rpm -qf` | `magic-mesh-13.0.0-35.x86_64` |
| `rpm -V` `mde-shell-egui` | unmodified (no verify line) |
| `mde-shell-egui.service` | `active` / `running` since `2026-08-22 22:31:27 EDT` |

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

`journalctl` for the last 7 days has no `OpenTransfers` / `Ctrl+J` /
hotkey-transfer lines (user unit empty; system unit empty; seat-wide
grep empty). That is not a refuse of the binding; the shell does not
audit catalog dispatch to the journal.

## Non-claims

- Ctrl+J was not pressed on a live Construct surface.
- Transfers mode was not observed opening from Files, Workers, Music, or
  any other surface.
- `production_admitted` was not flipped.
- No dest was invented.

## Leftover / blocker

The installed `13.0.0-35` package **does** list Ctrl+J in the compiled
hotkeys catalog. The leftover is still live-surface proof: a real
Construct key press opening Communications Transfers from every surface,
with the catalog refuse holding on Documents / Terminal / Desktop /
Browser text-or-guest focus. Catalog presence is not that proof.
