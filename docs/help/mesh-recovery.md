# Mesh Recovery — Losing a Lighthouse

The Lighthouse is your mesh's relay, Nebula CA, and leader control plane. Losing
the **only** one is the single painful failure in a small workgroup, because new
enrollments and cert signing need a CA, and peers behind NAT may need the relay.
This runbook covers preventing it, recovering when a second lighthouse exists, and
rebuilding when none does.

> **Prevent it first.** As soon as your mesh has a few peers, promote a second
> node to Lighthouse. With two, the loss of one is a non-event — the survivor
> keeps relaying, signing, and leading.

## Triage

```bash
meshctl fleet status        # which nodes are up, who's a lighthouse
meshctl test connectivity   # overlay reachability verdict
meshctl doctor              # on each surviving node
```

If peers still reach each other (a second lighthouse or direct paths exist), you
are not in a hard outage — go to **Case A**. If nothing reaches anything and your
only lighthouse is gone, go to **Case B**.

## Case A — a second lighthouse exists (or you can promote one)

1. Confirm a survivor is acting as leader/CA:
   ```bash
   meshctl fleet status
   ```
2. If no surviving node is a lighthouse, promote a healthy Server:
   ```bash
   # On the chosen node:
   meshctl install --role lighthouse
   mackesd take-leadership          # claim the leader lease
   ```
   The Nebula supervisor writes the role marker and starts the
   lighthouse + relay units; peers re-home to it on their next tick.
3. Verify enrollment works again by minting a token and re-checking:
   ```bash
   mackesd enroll-token --mesh-id <mesh>
   meshctl test connectivity
   ```

## Case B — the only lighthouse is gone

You must rebuild the control plane. The mesh's **storage** (Syncthing) and the
**replicated fleet state** survive on the peers — every node holds its own replica
of `/mnt/mesh-storage`, so there is no central storage master to restore. What
you're restoring is the **CA**.

1. **Pick a new lighthouse host** (a healthy peer, or a fresh install):
   ```bash
   meshctl install --role lighthouse
   ```
2. **Restore the CA** from the most recent `state-backup.enc` that replicated off
   the dead node into the local store:
   ```bash
   mackesd state-restore <path/to/state-backup.enc>
   ```
   The files on `/mnt/mesh-storage` need no restore step — they live on every
   surviving peer and re-converge over Syncthing once the new lighthouse is up.
3. **Re-establish the CA + overlay.** If the CA material did not replicate, mint a
   fresh mesh on this lighthouse and **re-enroll** the surviving peers (their old
   certs were signed by the lost CA):
   ```bash
   meshctl mesh init
   mackesd enroll-token --mesh-id <mesh>   # issue one token per surviving peer
   # On each peer:
   meshctl join --token <token>
   ```
   Re-enrolling rotates every peer onto the new CA; the old certs stop being
   trusted, which is the correct security outcome after a CA loss.
4. **Verify the rebuild:**
   ```bash
   meshctl doctor
   meshctl fleet status
   meshctl test connectivity
   ```

## After recovery

- **Add a second lighthouse now** so you never repeat Case B.
- Confirm Syncthing replication health (`systemctl status syncthing` on each peer;
  the mesh-health watchdog also alerts on an out-of-sync file plane).
- The Workbench **Controller → Audit** panel records the lifecycle operations
  (re-mint, re-enroll, leadership change) on the hash-chained audit timeline.

## What you cannot recover

Data that was only ever on the dead node and never replicated (goal 1, or written
between the last snapshot and the loss) is gone. The mesh is not a backup — keep
external backups of anything you cannot lose, per `DISCLAIMER.md`.
