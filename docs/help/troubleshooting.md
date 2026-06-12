# Troubleshooting

Start every investigation the same way:

```bash
meshctl doctor              # binaries, the mackesd service, the overlay link
meshctl logs --since 1h     # the mesh daemon's journal
```

`meshctl doctor` exits non-zero on a critical failure (missing nebula, dead
service, no overlay IP) and prints exactly which check failed.

## The node won't come up

**`doctor` says `service: mackesd` inactive.**
```bash
systemctl status mackesd
sudo systemctl restart mackesd
meshctl logs --since 10m
```
If it refuses to start, the role may be unpinned — `meshctl install --role <r>`.

**`doctor` says `overlay: nebula1` has no IP.**
The node isn't enrolled, or nebula isn't running. Re-check enrollment:
```bash
meshctl status
systemctl status nebula        # on a lighthouse: nebula-lighthouse.service
```
If unenrolled, get a fresh token from a lighthouse and `meshctl join --token <t>`.

## Can't reach other peers

```bash
meshctl test connectivity   # fleet-wide overlay reachability verdict
meshctl test firewall       # nebula1 must be in the `trusted` zone (PLANES-16)
```
- A failed connectivity verdict names the unreachable directed edges — check those
  peers' `meshctl doctor`.
- If `test firewall` reports nebula1 in a zone other than `trusted`, the firewall
  preset hasn't applied; restart `mackesd` (it reapplies on the next tick) and
  confirm `firewall-cmd` is installed.

## Names don't resolve (`<host>.mesh`)

```bash
meshctl test dns
```
The mesh-dns worker writes a managed `/etc/hosts` block and wires the `.mesh`
domain into systemd-resolved. If neither is present, restart `mackesd`; on a box
without `resolvectl` the `/etc/hosts` block is the always-applied fallback.

## A peer was removed but still connects

Revocation is immediate and fleet-wide via the Nebula blocklist, not the firewall:
```bash
mackesd decommission peer:<name>               # or: meshctl decommission peer:<name>
```
Every node folds the revocation into its Nebula `pki.blocklist` on the next config
refresh and drops the tunnel.

## Config / fleet drift

```bash
meshctl repair              # reconcile this node to the elected fleet baseline
```
The Workbench **Controller → Remediation** panel shows detected drift and the
matched playbook.

## Upgrades

Upgrade transitions surface as desktop alerts (ready / failed / complete). If a
node is stuck `ready_failed`, its `dnf` repo is likely broken — fix the repo and
the next tick re-attempts; the fleet barrier waits on quorum, not on one broken
peer.

## Losing a lighthouse

That's the one case with a dedicated runbook — see `mesh-recovery.md`.

## When you file an issue

Attach the output of `meshctl doctor` and `meshctl logs --since 1h`. Note your
node's role and how many peers are in the mesh (the envelope is ≤8).
