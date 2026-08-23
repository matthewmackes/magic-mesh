# WL-FUNC-028 leftover — live Dell sync-pair next-run / last-result — r1

Date: 2026-08-23  
Classification: leftover live CLI pair proof; **not** GUI Transfers editor,
enroll, freeze, or `production_admitted`  
Source revision: this change on `agent/drain-worklist-20260725`  
Control host: `rocky9-kvm2`  
Seat: Dell `172.20.146.225` (`DELL-LAPTOP`)  
`production_admitted: false`

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-028`.
- Operator 2026-08-23: overcome blockers and drain the worklist.
- Red `AI-GENERATED-ALERT` + 5s ran on Dell before the inbox permission
  correction and again before the attempted `mackesd.service` start.

## Blocker

`/var/lib/mde/transfers` was `root:root` `0755`. Seat-user
`mackesd transfer sync-pair add` refused EACCES. `sudo` is not the
operator CLI path.

## Act

`ensure_operator_inbox` now creates `<store>/inbox` mode `1777` when the
transfers worker opens its engine. The CLI `write_verb` does not chmod
(seat user cannot chmod a root-owned inbox).

On Dell, after the warning, root created `/var/lib/mde/transfers/inbox`
mode `1777` (corrected-forward of the missing worker ensure). Then `mm`
(not sudo) ran:

```text
mackesd transfer sync-pair add --id leftover-028 --interval 1h \
  --source /tmp/mcnf-sync-src --destination /tmp/mcnf-sync-dst
```

`ADD_RC=0`. `mackesd.service` does not exist on this seat (`Unit not
found`). `mackesd serve --group data` and `--group integrations` were
already running (`mcnf-wx-data.service`, `mcnf-wx-integrations.service`)
and drained the inbox.

## Observed store (`mackesd transfer sync-pair list --json`)

| Field | Value |
|---|---|
| id | `leftover-028` |
| every_secs | 3600 |
| last_result | `done` |
| peer_reachable | true |
| next_run_ms | `1787520448446` |
| last_fired_ms | `1787516848446` |

Inbox listing after drain: empty. Operator add was not sudo.

## Non-claims

- GUI Transfers editor (S2) was not proven.
- `production_admitted` was not flipped.
- This does not close `WL-FUNC-023` leftover (3).

## Farm

`cargo test -p mackesd operator_inbox_is_sticky_world_writable` queued on
`.196` slot 1 (`/tmp/farm-func028-inbox.log`).
