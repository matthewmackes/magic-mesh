# WL-FUNC-028 leftover honesty — installed seats lack sync-pair CLI — r1

Date: 2026-08-22  
Classification: leftover-honesty / installed-revision gap; **not** live
sync-pair proof, **not** RPM handoff, and **not** seat mutation  
Source revision: `0cb08f98c`  
Control host: `rocky9-kvm2`  
Observed: `2026-08-22T23:35:00Z`  
`production_admitted: false`

Read-only SSH from the control host. No package install, no enroll, no
`systemctl` mutate, no `sync-pair add`.

## Observed (all three acceptance seats)

| Seat | Address | hostname | RPM | `mackesd --version` | `mackesd transfer sync-pair` |
|---|---|---|---|---|---|
| Seat 15 | `172.20.0.15` | `Basement-Test-Workstation` | `magic-mesh-12.1.6-35.x86_64` | `12.1.6 "Construct" · non-promotable-unresolved · 2026-08-15 · dev` | `unrecognized subcommand 'sync-pair'` |
| Dell | `172.20.146.225` | `DELL-LAPTOP` | same probe family | same CLI family | same refuse |
| Surface | `172.20.146.79` | `SURFACE` | same probe family | same CLI family | same refuse |

Seat 15 transfer help lists only `submit`, `list`, `destinations`,
`cancel`, `pause`, `resume`. HEAD `mackesd transfer sync-pair
add|remove|list` is therefore not installed.

Also observed on all three seats (paths only; files unread):

- `files-folder-prefs.json` and `files-bookmarks.json` absent under
  `~/.config/mcnf/`, `~/.local/share/mcnf/`, and `/var/lib/mde/`
- no `gateway.toml` at `/etc/mackesd/`, `~/.config/mcnf/`, or
  `/var/lib/mackesd/`

## Unblock

Live FUNC-028 next-run / last-result proof needs the current-revision
`sync-pair` CLI on an acceptance seat. That requires the unpublished
signed three-RPM handoff (`WL-REL-002`), which is Blocked on freeze
(`WL-REL-001`). Seat package mutation stays forbidden until that
candidate exists and a worker runs red `AI-GENERATED-ALERT` + 5s.

This record also supports parking `WL-FUNC-029` (live Vitelity; operator
2026-08-22 allows seat+Vitelity mutation only with that candidate) and
`WL-FUNC-030` (no migrated `gateway.toml` on any acceptance seat).
