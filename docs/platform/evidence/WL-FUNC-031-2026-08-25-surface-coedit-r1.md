# WL-FUNC-031 leftover honesty — Surface Documents co-edit — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:30Z`–`2026-08-25T10:52:52Z`  
Classification: leftover-honesty / installed-seat live probe; **not**
two-seat co-edit, **not** visible-cursor production proof, **not**
`production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at `4071ed295e18`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Read-only SSH. No share start. No join. No `systemctl` mutate. No dest
invented. Seat 15 and Dell were not SSHed (parent/Health Fix). In-tree
Documents mount (`documents.rs` `live_document_share_session` /
`documents_body` sync) and external-write three-way merge were not
changed this unit. Phase-3c markers are already gone from
`documents.rs`. Farm gates were not re-run.

Same Surface identity and dest-cut as
`WL-FUNC-024-2026-08-25-surface-live-media-r1.md`. Prior park:
`WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.

## In-tree vs installed

`documents_body` still mounts share start/join/follow/close and pumps
`live_document_share_session()`. Construct on this seat packs those
strings (byte counts in `/usr/bin/mde-shell-egui`):

| Literal | Count |
|---|---|
| `Share this document into a space` | 1 |
| `Cannot share: this seat is not a member` | 1 |
| `Joined share session.` | 1 |
| `Share session closed` | 2 |

Packed UI is not a live session.

## Live leftover on Surface

`collab` did not spawn at `mackesd-data` start
(`2026-08-25T10:00:12.526096Z`):

```text
collab worker: governed collaboration identity unavailable; not spawning
error=collaboration identity admission is stale, malformed, or out of scope
```

Admission `source_revision` is still `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`
against installed `4071ed295e18…`. No `/run/mde-bus` document-session /
share / collab topic tree. Construct holds `/dev/dri/card1` (pid
`1290917`) but journal since dest-cut has no Documents / share-session
lines.

Overlay ICMP to Seat 15 `10.42.0.5` and Dell `10.42.0.4` succeeds from
this seat. That is not two-seat co-edit with visible cursors.

## Blocker

Leftover is live two-seat co-edit only. Closing it needs a current-SHA
collaboration identity dest on Surface (FUNC-023; not this write scope)
so `collab` can spawn, plus a second current-revision seat (Seat 15/Dell
Health Fix) and an operator share. Do not invent a dest. Do not flip
`production_admitted`.
