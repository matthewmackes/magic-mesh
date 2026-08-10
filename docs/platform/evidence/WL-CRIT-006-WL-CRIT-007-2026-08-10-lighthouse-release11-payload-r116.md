# Lighthouse release-11 payload and peer-publication correction — 2026-08-10

## Scope

Live upgrade inspection on lighthouse `104.236.118.177` found two independent
corrected-forward blockers after installing the signed
`magic-mesh-lighthouse-12.1.6-10.x86_64` package:

- `mcnf-mesh-secret-recipient.service` failed because the thin RPM shipped its
  reconciler but omitted the reconciler's package-owned
  `/opt/mcnf/automation/secrets/mcnf-secret.sh` dependency.
- authenticated peer publication was withheld as `machine-id-untrusted`
  because the cloud image had left `/etc/machine-id` group-writable at mode
  `0664`.

The installed release-10 package verified cleanly with `rpm -V`; the missing
helper was therefore a manifest omission, not live filesystem corruption.

## Corrected-forward source

Implemented at repository revision `6d812175a6d459744f92bf9a54abd47ca1dc6654`:

- the lighthouse variant advances to release 11 and ships the secret helper;
- all daemon-bearing RPM shapes are required to carry the helper, reconciler,
  unit, and timer exactly once;
- the package's existing tmpfiles convergence normalizes `/etc/machine-id` to
  root-owned mode `0444`, preserving the daemon's fail-closed identity reader.

Focused farm command:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=crit007-lighthouse-secret-helper-r116 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  onboard::role_provision::tests::every_daemon_rpm_bootstraps_mesh_secret_recipients \
  -- --exact --nocapture
```

Result: PASS, 1 passed, 0 failed, 4,666 filtered out. The final warm rerun
completed in 0.38 seconds and exercised the exact-once payload assertion and
machine-ID convergence rule.

## Live recovery proof

The live mode was corrected from `0664` to the standard read-only `0444` without
changing machine-ID content. On the next heartbeat, the daemon wrote
`/run/mesh-health/peer-publication.ok` at `2026-08-10 10:27:33 UTC`. The next
watchdog run at `10:28:03 UTC` reported `mesh-health: ok` and completed
successfully. `/run` had recovered to 49% free; the earlier 0% warning stopped
once transient package pressure cleared and was not an arithmetic defect.

## Release build and signature

BigBoy built the exact revision as a promotable locked release. The optimized
build completed in 12 minutes 48 seconds, and both lighthouse size and payload
verification passed. The final 13.5 MiB artifact is:

```text
magic-mesh-lighthouse-12.1.6-11.x86_64.rpm
sha256 90f239dba648a0b20cf1c4535fcdc670c0649e65874a12d7dcef7aa766c6df6c
```

The RPM carries the project RSA signing-subkey signature. Fedora 44 reported
`digests signatures OK`; the signed `SHA256SUMS` bundle also passed checksum and
GPG verification. `rpm -Uvh --test` passed before deployment.

## Corrected-forward deployment

Lighthouse `.1` was upgraded from release 10 to release 11 after the operator
alert and five-second hold. The transaction preserved the exact hashes of
`/etc/etcd/etcd.env` and `/etc/machine-id`. Post-upgrade proof established:

- `rpm -V magic-mesh-lighthouse` passes;
- `/etc/machine-id` is root-owned mode `0444` and the packaged secret helper is
  root-owned mode `0755`;
- etcd, Nebula, the six grouped daemons, their target, and both recovery timers
  are active;
- all three etcd voters remain healthy at term 5534 and applied index 462538;
- authenticated peer publication remains fresh; and
- `mesh-health.service` reports `mesh-health: ok` with result/status `success/0`.

The newly executable recipient service then exposed pre-existing key drift:
`.1` had registered a valid local public recipient but could not decrypt the
current ciphertext. Surface and seat 15 also failed closed; Dell was the
current holder. After a second alerted hold, Dell ran the designed
scope-preserving `reseal-all`: all four stored secrets were re-encrypted to the
six registered public recipients. Dell retained decryption authority, and `.1`
then completed recipient reconciliation twice with result/status `success/0`.
No private identity or plaintext secret was printed or transferred.

## Remaining boundary

Lighthouses `.2` and `.3` remain healthy voters but have not received release
11 because the available root key does not authenticate to those existing
droplets. This checkpoint proves the corrected package and recipient recovery
on `.1`; it does not claim three-lighthouse release convergence.
