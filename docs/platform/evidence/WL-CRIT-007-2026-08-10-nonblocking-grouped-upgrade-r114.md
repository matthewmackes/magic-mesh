# WL-CRIT-007 — non-blocking grouped upgrade restart (r114)

Date: 2026-08-10

Base revision: `68450f54`

## Defect and correction

Release 31 upgraded successfully on Dell, seat 15, and Surface, but each RPM
transaction held the RPM database lock while `systemctl try-restart
mackesd.target` synchronously converged etcd, credentials, and all six grouped
services. Dell's observed grouped restart remained in that call for more than
60 seconds. This recreated a silent long wait in the upgrade path even though
the DRM shell is intentionally independent of full daemon-target convergence.

The base RPM now keeps the shell restart synchronous, ensuring the installed UI
binary replaces the running process before the transaction returns, but queues
the grouped target's corrected-forward restart with `systemctl --no-block`.
Systemd continues to enforce every service's ordering, readiness, retry, and
failure state without unnecessarily retaining the RPM transaction lock.

## Focused farm proof

Machine 9 (`172.20.0.50`), slot `crit007-rpm-nonblocking-r114`, passed the exact
RPM seat-service activation contract and extracted scriptlet shell-syntax test.
The guard requires the non-blocking grouped restart after package setup and
rejects reintroduction of the synchronous form.

Source SHA-256:

- `41982d2bad20d7d6a26e6c93e679fa8370916e1ceeea4217542509e72c222e88`
  — `crates/mesh/mackesd/Cargo.toml`
- `6bb110b29d43c7b3b4a3886366d3928a20288ae8ee4cf5daaf2c2cefc1ac3aa6`
  — `install-helpers/test-rpm-seat-service-activation.sh`

This corrects the package-transaction wait for the next RPM. It does not alter
the already signed release-31 bytes or claim a new live package installation.
