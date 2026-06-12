# Magic Mesh — Operator's Day-2 Guide

The lifecycle of a running mesh, in the order you'll live it. Deep runbooks
live in [`docs/help/`](docs/help/) (installed to `/usr/share/mde/help/`,
browsable in the Workbench **Help** panel); this page is the map. The trust
model and its limits: [`DISCLAIMER.md`](DISCLAIMER.md) ·
[`SUPPORT.md`](SUPPORT.md).

## 0. Install + first boot

→ [`docs/help/install.md`](docs/help/install.md)

One RPM (ISO or `dnf install <release URL>`; the RPM drops the
`[magic-mesh]` dnf repo + signing key, so `dnf upgrade` works afterward).
First boot: the role chooser pins **Lighthouse ⊂ Server ⊂ Workstation**
(upgrade-only, never downgrade). Per-role expectations:
[`docs/help/node-setup.md`](docs/help/node-setup.md).

## 1. Stand up the mesh + enroll peers

```bash
# On the first node (the Lighthouse):
mackesd mesh-init --mesh-id <name> --external-addr <ip-or-dns>

# Per joining peer — mint a token on the Lighthouse, redeem on the peer:
mackesd enroll-token --mesh-id <name>     # prints a one-time token
mackesd enroll --token <token>            # on the new peer
```

Enrollment is CSR-based: the CSR lands on the replicated volume and the
Lighthouse's auto-signer signs it under the active CA epoch (manual path:
`mackesd ca sign-csr`). Full sequence: [`packaging/ENROLLMENT.md`](packaging/ENROLLMENT.md).

## 2. Provision the backup — do this on day one

```bash
# Export in the daemon's environment (systemd drop-in or /etc/mackesd env):
export MDE_BACKUP_PASSPHRASE='<8+ random chars, stored in your password manager>'
```

With the passphrase set, the daily backup worker writes an encrypted
(XChaCha20-Poly1305 + Argon2id) `state-backup.enc` to the replicated volume.
**Unset, the backup is disabled** — and the daemon tells you so: the alert
`MDE_BACKUP_PASSPHRASE unset` repeats in the journal and
`mackesd_backup_passphrase_set 0` shows in the metrics. Staleness (>48 h)
also alerts.

Off-cluster copy (recommended, monthly + after CA rotation):

```bash
mackesd ca export --output /safe/offsite/ca-bundle.enc   # same passphrase env
```

## 3. Watch it

| Surface | What |
|---|---|
| `mackesd healthz` | store view: node-health buckets, audit chain |
| Bus healthz (Workbench Overview) | + live workers, breaker, the `ready` verdict |
| `meshctl doctor` / `meshctl fleet status` | binaries, service, overlay, fleet |
| `journalctl -u mackesd | grep mackesd::alert` | every alert, severity-mapped (the headless surface) |
| `/var/lib/node_exporter/textfile_collector/mackesd.prom` | Prometheus gauges: node health, CA days-remaining, router latency histogram, workers/breaker, disk headroom, backup posture |
| `[[alert_hooks]]` in `/etc/mackesd/mackesd.toml` | your command, event JSON on stdin (wire `curl`/pager yourself) |

## 4. Upgrade

- **The platform:** `sudo dnf upgrade magic-mesh` (the repo + key shipped in
  step 0 make this work). Workers restart with the daemon; the role pin and
  store carry over.
- **Fleet desired-state:** author a revision (Workbench → Fleet, or the
  `action/fleet/push-revision` Bus verb); every node's reconcile worker
  elects the head and converges itself — no push-SSH, no center.

## 5. Roll back

- **Fleet revision:** `action/fleet/rollback` (Workbench → Fleet) — the log
  keeps prior revisions; nodes converge to the elected head as usual.
- **A bad config experiment on one node:** node-local exceptions
  (`magic-fleet reconcile --except <file>`) keep a node out of a baseline
  domain without forking the fleet.

## 6. Restore / disaster recovery

→ [`docs/help/mesh-recovery.md`](docs/help/mesh-recovery.md) (the full
lighthouse-loss runbook)

```bash
mackesd state-restore <path/to/state-backup.enc>   # MDE_BACKUP_PASSPHRASE set
# or, for the off-cluster CA bundle:
mackesd ca import --input /safe/offsite/ca-bundle.enc
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
