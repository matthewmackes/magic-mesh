# WL-CRIT-007 / WL-FUNC-019 — resource credential activation recovery (r110)

Date: 2026-08-10

Base revision: `b2895c9f`

## Defect and correction

The resource-publisher credential oneshot honestly failed when SecretStore was
unavailable during boot, but it never retried. A transient startup ordering or
transport failure could therefore leave resource publication unavailable until
an operator or package upgrade restarted the unit.

Transient failures now retry every 30 seconds, capped at six starts in five
minutes. Missing secrets, invalid key material, and local configuration errors
use terminal sysexits values and remain visibly failed without retry. The unit
does not mask failure as success and cannot enter an unbounded restart storm.

## Focused farm proof

Machine 193 (`172.20.0.90`), slot
`crit007-resource-credential-retry-r1-20260810`, passed the helper self-test and
hostile structural cases for storm pacing, policy override, excessive burst,
permanent-failure retry, and failure masking. Exit status was 0.

Source SHA-256:

- `bfba6418fedf69deba9a1d7d6ad40ab4f58c9cc15761b155f1c0a70bc9e1da77`
  — `install-helpers/provision-resource-publisher-credential.sh`
- `8daab8c49bce9db87c83c3c18a12fdd1ca1e9189e33a9ebece9dafcd5f9f5d7e`
  — `packaging/systemd/mcnf-resource-publisher-credential.service`

This corrects transient activation recovery. A genuinely absent host-approved
secret remains an explicit deployment blocker, not a green state.
