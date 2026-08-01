# MCNF — Operator's Day-2 Guide

The lifecycle of a running mesh, in the order you'll live it. Deep runbooks
live in [`docs/help/`](docs/help/) — in the repo, and installed on every node at
`/usr/share/mde/help/`. Read them there or here: the egui shell does not render
these runbooks in-product. This page is the map. The trust model and its limits:
[`DISCLAIMER.md`](DISCLAIMER.md) · [`SUPPORT.md`](SUPPORT.md).

## 0. Install + first boot

→ [`docs/help/install.md`](docs/help/install.md)

One RPM (ISO or `dnf install <release URL>`; the RPM drops the
`[magic-mesh]` dnf repo + signing key, so `dnf upgrade` works afterward).
First boot: the role chooser pins one of the two roles — **Lighthouse →
Workstation** (upgrade-only, never downgrade; a headless box is a Workstation
without a display). Per-role expectations:
[`docs/help/node-setup.md`](docs/help/node-setup.md).

### Production boundary

Construct's supported production purpose is a small, mutually trusted workgroup
mesh with headless control-plane services and VM-based desktops. It is not a
zero-trust, multi-tenant, hyperscale, or general consumer desktop platform.

Production promotion requires the fixed six-node baseline: **three
lighthouses plus three workstations**. A static GitHub check result is necessary
but not sufficient; signed release evidence must also contain successful farm,
Fedora compatibility, mesh join, failover, recovery, workload, VDI, and
required hardware gates. A missing or unavailable gate means engineering
preview, not production-ready.

## 1. Stand up the mesh + enroll peers

```bash
# On the first node (the Lighthouse):
mackesd mesh-init --mesh-id <name> --external-addr <ip-or-dns>

# Per joining peer — mint a token on the Lighthouse, redeem on the peer:
mackesd enroll-token --mesh-id <name>     # prints a one-time token
mackesd join '<token>'                    # on the new peer
```

Enrollment is CSR-based: the CSR goes straight to the Lighthouse's `/enroll`
endpoint over TLS pinned to the token's fingerprint, and its auto-signer signs
it under the active CA epoch (manual path: `mackesd ca sign-csr`). Full
sequence: [`packaging/ENROLLMENT.md`](packaging/ENROLLMENT.md).

## 2. Provision the backup — do this on day one

```bash
# Provision the CA-backup passphrase as a root-only systemd credential.
# The lighthouse join path provisions this automatically; for manual setup,
# follow the systemd-creds + LoadCredentialEncrypted example in
# packaging/systemd/mackesd.service. Never export the passphrase or put it in
# a unit Environment= setting or command argument.
```

With the credential provisioned, the daily backup worker writes an encrypted
(XChaCha20-Poly1305 + Argon2id) `state-backup.enc` to the replicated volume.
**Unset, the backup is disabled** — and the daemon tells you so: the alert
`MDE_BACKUP_PASSPHRASE unset` repeats in the journal and
`mackesd_backup_passphrase_set 0` shows in the metrics. Staleness (>48 h)
also alerts.

The current encrypted backup remains mandatory while the selected future
recovery model moves toward replicated live state. Do not disable it until the
peer-replication recovery drill is complete and its signed evidence is attached
to the release record.

Off-cluster copy (recommended, monthly + after CA rotation):

```bash
mackesd ca export --passphrase-stdin --output /safe/offsite/ca-bundle.enc
```

## 3. Watch it

| Surface | What |
|---|---|
| `mackesd healthz` | store view: node-health buckets, audit chain |
| Bus `healthz` response | live workers, breaker, the `ready` verdict |
| `meshctl doctor` / `meshctl fleet status` | binaries, service, overlay, fleet |
| `journalctl -u mackesd | grep mackesd::alert` | every alert, severity-mapped (the headless surface) |
| `/var/lib/node_exporter/textfile_collector/mackesd.prom` | Prometheus gauges: node health, CA days-remaining, router latency histogram, workers/breaker, disk headroom, backup posture |
| `[[alert_hooks]]` in `/etc/mackesd/mackesd.toml` | your command, event JSON on stdin (wire `curl`/pager yourself) |

## 4. Upgrade

- **The platform:** `sudo dnf upgrade magic-mesh` (the repo + key shipped in
  step 0 make this work). Workers restart with the daemon; the role pin and
  store carry over.
- **Fleet desired-state:** author a revision with the
  `action/fleet/push-revision` Bus verb; every node's reconcile worker
  elects the head and converges itself — no push-SSH, no center.

## 5. Roll back

- **Fleet revision:** the `action/fleet/rollback` Bus verb — the log
  keeps prior revisions; nodes converge to the elected head as usual.
- **A bad config experiment on one node:** node-local exceptions
  (`magic-fleet reconcile --except <file>`) keep a node out of a baseline
  domain without forking the fleet.

## 6. Restore / disaster recovery

→ [`docs/help/mesh-recovery.md`](docs/help/mesh-recovery.md) (the full
lighthouse-loss runbook)

```bash
mackesd state-restore --passphrase-stdin <path/to/state-backup.enc>
# or, for the off-cluster CA bundle:
mackesd ca import --passphrase-stdin --input /safe/offsite/ca-bundle.enc
```

Then re-mint enroll tokens for peers that need to rejoin
(`mackesd enroll-token --mesh-id <name>`), and `mackesd take-leadership` if
the recovered node should hold the leader role.

## 7. Certificate lifecycle

- **Watch the cliff:** `mackesd_ca_cert_days_remaining` (metrics) warns at
  ≤30 days. Peer certs don't expire mid-epoch — the CA cert is the cliff.
- **Rotate:** `mackesd ca rotate` — bumps the CA epoch; the supervisor
  re-signs peers under the new epoch automatically.
- **Evict a peer:** `mackesd decommission peer:<name>` (cert revoked,
  fingerprint blocklisted, tunnels refused) — also via
  `meshctl decommission peer:<name>`.
- **Rotate the shared passcode:** `mackesd rotate-passcode`.

After any CA rotation: refresh the off-cluster export (step 2).

## 8. Troubleshoot

→ [`docs/help/troubleshooting.md`](docs/help/troubleshooting.md)

```bash
meshctl doctor                 # the first move, always
meshctl test connectivity      # then the focused probes: dns, firewall
meshctl logs --since 1h
```

A tripped circuit breaker (`mackesd_breaker_tripped` > 0 / the crit alert)
means a worker died repeatedly and stays down by design — fix the cause,
then `systemctl restart mackesd` to re-arm.
