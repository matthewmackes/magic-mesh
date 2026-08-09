# Eagle release 23 corrected-forward alignment r7

Date: 2026-08-09  
Revision: `1cdf3d2f2ac788335fb3142a11922f25f7d6400f`  
Farm: machine 193 (`172.20.0.90`), slot `eagle-release23-r7`

## Result

The governed rollout failed closed before warning, installation, service
restart, or reboot. Eagle remained on `magic-mesh-12.1.6-12.x86_64`.

The exact target preflight passed: `T470S-EAGLE`, machine ID
`4a25b0f8b4f94339a8e4c882b0db13b2`, Fedora 44 x86_64, and overlay
`10.42.0.6/17`. The package-owned warning helper is present and password-backed
sudo authentication succeeded. Non-interactive `sudo -n` remains unavailable.
The warning helper was deliberately not invoked after artifact admission
failed, so the seat did not receive a false update warning.

## Candidate provenance refusal

Repository and package tooling found no authoritative release-23 artifact or
source receipt:

- the `gh-pages` Fedora 44 channel contains no release-23 RPM;
- GitHub Releases has no release-23 asset (the newest published release is
  `v12.1.1`);
- machine 193 had no cached release-23 RPM before this run;
- seat 15 has no candidate manifest, release receipt, or source-revision
  receipt in the governed system/package paths inspected.

Seat 15 retained `/tmp/magic-mesh-12.1.6-23.x86_64.rpm`. It was copied only
into the farm slot's `quarantine/` directory for read-only analysis and was
never treated as an installation source. Its SHA-256 is
`9ffcba3861aaad07098dfea64215d6a407a0e2212261b858f77846f2c6c29148`.
Its complete RPM file-digest table exactly matches seat 15's installed RPMDB
record, and `rpm -V magic-mesh` on seat 15 was clean. Those facts bind the
quarantined bytes to seat 15's installed payload, but do not establish source
provenance.

The artifact failed mandatory admission:

- `rpm -K` reported only header and payload digests; `SIGPGP` is `(none)`;
- `SOURCERPM`, `BUILDHOST`, `PACKAGER`, and `VENDOR` are all absent;
- no immutable 40-character source revision or signed candidate manifest is
  carried alongside the package;
- `verify-rpm-payload.sh payload` failed because
  `/usr/lib/systemd/system/mcnf-resource-publisher-credential.service` is
  absent from the RPM even though the current governed base manifest requires
  it.

The payload otherwise has the workstation role shape needed by CRIT-007: the
base package name and architecture are correct; `mackesd`,
`mde-shell-egui`, `mesh-peer-recovery`, `seat-update-warning`, the recovery
unit, sleep hook, and NetworkManager dispatcher are present; KVM requirements
are present; and the compressed package is 85.6 MiB. These checks cannot
override the signature, source-receipt, and complete-payload failures.

## No-mutation post-state

After all checks Eagle still had the same boot ID
`0eb63ff9-eed0-420a-93cf-ac37c7e0e2ec`, package release `-12`, and package
install timestamp `1786193039`. It had one `mackesd` process, one shell process,
and two login sessions. No RPM/DNF transaction, warning publication, daemon or
session restart, handoff attempt, recovery action, suspend, or reboot occurred.

## Exact prerequisite

Cut or locate a workstation release-23 RPM through the governed repository
tooling from an immutable source revision, publish its source receipt and
candidate manifest, sign it with the trusted Magic Mesh RPM key, and require
the manifest to bind the RPM digest, full payload, role, and runtime binary
digests. The package must include the complete current base payload, including
`mcnf-resource-publisher-credential.service`. Once those gates pass, use the
verified password authority (or a narrowly governed non-interactive sudo rule),
publish the mandatory visible warning, install corrected-forward on this exact
Eagle identity, and run the bounded recovery and reciprocal-peer checks. Do not
roll back.

WL-CRIT-007 recovery acceptance and WL-FUNC-021 cross-seat handoff remain open;
this record proves a safe refusal, not rollout or live handoff acceptance.
