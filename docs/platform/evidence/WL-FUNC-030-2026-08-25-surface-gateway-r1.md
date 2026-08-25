# WL-FUNC-030 leftover honesty — Surface SIP gateway form / live GET — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:55Z`–`2026-08-25T10:59:30Z`  
Classification: leftover-honesty / live-seat GET without credentials;
**not** set-gateway, **not** clear-gateway, **not** in-place
`gateway.toml` migrate, **not** Activity form paint, **not**
`production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
Seat: Surface `172.20.146.79` (`SURFACE`, overlay `10.42.0.7`)  
`production_admitted: false`

SSH as `root@` with `/root/.ssh/mackes_mesh_ed25519`. No invented SIP
host, username, or password. No `gateway.toml` write. No set/clear.
Did not SSH Seat 15 or Dell.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-030`.
- Prior Seat 15 + Surface path-only probe (Surface actions then down):
  `WL-FUNC-030-2026-08-24-live-gateway-r1.md`.
- Identity dest that later started Surface actions:
  `WL-FUNC-023-2026-08-24-surface-collab-dest-admitted-r1.md`.

## Packed form (installed Construct)

| Literal | Count |
|---|---|
| `action/voip/set-gateway` | 1 |
| `action/voip/get-gateway` | 1 |
| `action/voip/clear-gateway` | 1 |
| `SIP gateway` | 2 |
| `Mesh-wide outbound registrar` | 1 |
| `No SIP gateway configured` | 1 |
| `Confirm clear gateway` | 2 |
| `gateway.toml` | 2 |

Contiguous `Set gateway` / `Clear gateway` bytes are absent (Refresh +
confirm-clear + refuse copy are packed). The publisher is in the
dest-cut; that is not a live Activity paint.

## Live GET (no credentials)

`mackesd-actions` active since 2026-08-25 06:00:12 EDT. Thread
`voip-bus-respon`. Journal:

```text
2026-08-25T10:00:12.244558Z VOIP gateway Bus responder spawned; serving action/voip/{set-gateway,get-gateway,clear-gateway}
```

`MDE_BUS_ROOT=/run/mde-bus mde-bus request action/voip/get-gateway
--json --timeout-secs 15` at `2026-08-25T10:54:06Z`:

| Field | Value |
|---|---|
| request ULID | `01M0W8WMR9W68Y8VSWSZN9MHWX` |
| request body | none |
| reply ULID | `01M0W8WMX60GQS7MGJ2GV204C2` |
| reply body | `{"present":false}` |
| rc | 0 |

No `password` field in the absent readout. sqlite:
`action/voip/get-gateway` count 1, matching reply 1. After Ctrl+J/N
(FUNC-028) that count stayed 1 — Activity `maybe_request_gateway_get`
did not run.

## `gateway.toml`

Canonical `/mnt/mesh-storage/voip/gateway.toml` **absent**.
`/mnt/mesh-storage/voip/` **absent**. `find` for `gateway.toml` under
`/mnt/mesh-storage`, `/etc/mackesd`, `/var/lib/mackesd`, `/var/lib/mde`
returned no paths. There is no migrated workgroup file to hydrate.

## Non-claims

- `set-gateway` / `clear-gateway` were not published.
- Password redaction on a present readout was not live (no password
  existed).
- Communications Activity form paint was not read from pixels.

## Leftover / blocker

GET on a live Surface Bus is proved (`present: false`, no credentials).
Closing the leftover still needs Activity form paint plus set/get/clear
against a **migrated** workgroup `gateway.toml`, not an invented
registrar. Do not write passwords. Do not flip `production_admitted`.
