# WL-FUNC-028 leftover honesty — installed sync-pair CLI — r1

Date: 2026-08-23  
Classification: leftover-honesty / installed-CLI presence; **not** live
next-run / last-result on a saved pair, **not** GUI Transfers proof  
Source revision: `4d7ce44080a3`  
Control host: `rocky9-kvm2`  
Observed: `2026-08-23T15:20:39Z`  
`production_admitted: false`

Read-only SSH from the control host. No `sync-pair add`, no enroll, no
package install, no `systemctl` mutate.

The 2026-08-22 CLI gap (`WL-FUNC-028-2026-08-22-installed-cli-gap-r1.md`)
was `magic-mesh-12.1.6-35` without `mackesd transfer sync-pair`. That
gap is closed on the three `13.0.0` acceptance seats. The leftover is
now a saved pair with worker next-run / last-result.

## Observed (all three acceptance seats)

| Seat | Address | hostname | RPM | `mackesd --version` | `mackesd transfer sync-pair` |
|---|---|---|---|---|---|
| Dell | `172.20.146.225` | `DELL-LAPTOP` | `magic-mesh-13.0.0-35` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` | `add`, `remove`, `list` present |
| Seat 15 | `172.20.0.15` | `Basement-Test-Workstation` | same RPM | same CLI family | same subcommands |
| Surface | `172.20.146.79` | `SURFACE` | same RPM | same CLI family | same subcommands |

Dell `mackesd transfer sync-pair list` printed `no sync pairs saved`.
`--json` printed `[]`.

Also observed on all three seats (paths only; files unread):

- `files-folder-prefs.json` and `files-bookmarks.json` absent under
  `~/.config/mcnf/`, `~/.local/share/mcnf/`, and `/var/lib/mde/`
- no `gateway.toml` at `/etc/mackesd/`, `~/.config/mcnf/`, or
  `/var/lib/mackesd/`

## Leftover

FUNC-028 leftover is still live Bus / operator-visible next-run and
last-result on a real pair. Empty `list` is not production pair proof.

A Dell mutation attempt after `seat-update-warning.sh` (red
`AI-GENERATED-ALERT` + 5s) refused:

```text
Error: writing save-sync-pair verb under /var/lib/mde/transfers
Caused by:
    Permission denied (os error 13)
```

`/var/lib/mde/transfers` is `root:root` `0755`. Seat user `mm` can list
pairs but cannot enqueue `save-sync-pair`. That is the next producer
gap; do not treat a `sudo` write as the operator CLI path.
