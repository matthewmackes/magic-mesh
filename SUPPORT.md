# Support & Production Envelope

MCNF is **production workgroup-grade within a stated envelope** — not a
managed service. This document states what's supported, what isn't, and where to
go for help.

## Supported envelope

| Dimension | Supported | Notes |
|-----------|-----------|-------|
| Peers | **1–8 in one mesh** | The §8 trust-envelope lock. Split larger fleets into separate workgroups. |
| Roles | Lighthouse, Server, Workstation | One signed RPM; chosen at install (`meshctl install --role <r>`). |
| OS | Fedora (Cosmic spin) | The Magic-on-Cosmic ISO + the GitHub-hosted RPM (Releases asset / GitHub Pages dnf repo) are the supported install paths. |
| Transport | Nebula overlay only | No unencrypted fallback; underlay stays firewalled. |
| Storage | Syncthing replicated volume | Backups remain the operator's responsibility. |
| Desktop | Cosmic | The GUI is strictly IBM Carbon (Gray 10/90/100). |
| Language | **en-US only** | All GUIs, CLI output, logs, and docs are English. Localization is deliberately out of envelope for a ≤8-peer workgroup product (EFF-49); revisit only if the envelope ever widens. |

Running **beyond 8 peers**, on non-Fedora hosts, or in regulated / safety-critical
/ high-availability settings is **out of envelope** and unsupported without
independent review.

## What "supported" means here

- **Best-effort, community support.** Issues and discussion happen in the project
  repository. There is **no SLA, paid support tier, incident-response retainer, or
  recovery service** unless separately agreed in writing.
- **Self-service operations.** The platform is designed so one operator runs it
  with `meshctl` and the Workbench. The lifecycle gestures are documented in
  `docs/help/` (installed to `/usr/share/mde/help/`, browsable in the Workbench
  Help panel).
- **You own recovery.** Keep backups. The lighthouse-loss runbook is
  `docs/help/mesh-recovery.md`.

## Getting help, in order

1. `meshctl doctor` — checks binaries, the mackesd service, and the overlay link.
2. `docs/help/troubleshooting.md` — common failures and fixes.
3. `meshctl logs --since 1h` — the mesh daemon's journal.
4. The Workbench **Health** and **Logs/metrics** panels.
5. The project repository's issues, with the output of `meshctl doctor` attached.

## Reporting a security issue

The open-mesh trust model (flat trust among ≤8 peers) is documented in
`DISCLAIMER.md` as an accepted trade-off, not a bug. Genuine vulnerabilities
(cert/enrollment bypass, crypto downgrade, revocation that doesn't evict) should
be reported privately to the maintainers before public disclosure.
