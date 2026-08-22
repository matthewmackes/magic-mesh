# Live leftover park — FUNC-024/025/026/027/031/032 — r1

Date: 2026-08-22  
Classification: leftover-honesty / park; **not** live media, Files, co-edit,
or hotkey production proof  
Source revision: `6d835d990`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

In-tree compile leftovers for these epics are gone. Remaining demand is
live-seat production evidence. Read-only 2026-08-22 probes on Dell
(`172.20.146.225`), Seat 15 (`172.20.0.15`), and Surface (`172.20.146.79`)
show every seat still on `magic-mesh-12.1.6-35` (`mackesd 12.1.6`,
2026-08-15). Details: `WL-FUNC-028-2026-08-22-installed-cli-gap-r1.md`.

Also absent on all three seats (paths only; files unread):

- `files-folder-prefs.json` and `files-bookmarks.json` under
  `~/.config/mcnf/`, `~/.local/share/mcnf/`, and `/var/lib/mde/`
- `gateway.toml` under `/etc/mackesd/`, `~/.config/mcnf/`, and
  `/var/lib/mackesd/`

No unpublished signed three-RPM candidate exists (`WL-REL-002` Blocked).
Operator 2026-08-22: seats may be mutated only when that candidate exists,
with red `AI-GENERATED-ALERT` + 5s.

## Why each epic is parked

| Epic | In-tree leftover | Why parked |
|---|---|---|
| FUNC-024 | live media / SFU / PSTN | Needs current-revision media plane plus FUNC-030 (now Blocked) |
| FUNC-025 | live mesh-tree / archive-queue | Needs current-revision Files on a used seat |
| FUNC-026 | live restart of folder prefs | Persist files absent; needs operator use then restart |
| FUNC-027 | live restart of bookmarks | Same as FUNC-026 |
| FUNC-031 | live two-seat co-edit | Needs two current-revision seats and operator share |
| FUNC-032 | live-surface hotkey proof | Needs current-revision Construct on a used seat |

Unblock: unpublished signed candidate (`WL-REL-002`) installed under the
seat-mutation lock, then live evidence on Dell / Seat 15 / Surface.
Do not treat farm-green crate tests as production for these leftovers.
