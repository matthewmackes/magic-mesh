# DATACENTER-23 / DAR-37..43 — disaster recovery of the substrate

The MCNF substrate has **no fixed center**. The recoverable state lives in the
replicated etcd store, plus (Full tier) the Forgejo CI DB on the control VM:

- `/tofu/state/*` — the OpenTofu IaC states (incl. `/tofu/state/edgeos`, DAR-9b).
- `/mcnf/secret/*` — the mesh secrets, **already age-encrypted** in etcd.
- `/mcnf/age-recipient` + `/mcnf/age-recipients/*` — the recipient set, so a
  restore can re-seal new secrets to the same identities.
- Forgejo sqlite DB + repos — control-VM-local CI state.

## Manifest v2 (DAR-38)

`dr-backup.sh` writes a single age-encrypted `dr-<ts>.age` with `dr_backup_version=2`
and five components:

| Component        | Capture |
|------------------|---------|
| `tofu-state`     | `/tofu/state/*` via the etcd v3 range API (read-only) |
| `secrets`        | `/mcnf/secret/*` (age ciphertext → **age-in-age**) |
| `age-recipients` | `/mcnf/age-recipient` + `/mcnf/age-recipients/*` |
| `forgejo-data`   | Forgejo sqlite + repos with a **sqlite quiesce** (below) |
| `etcd-snapshot`  | a **consistent** etcd v3 snapshot from the **leader** endpoint, revision recorded |

The whole manifest is single-`age`-encrypted to the mesh recipient; secrets stay
age-in-age. The top-level `entries` array is preserved for back-compat so the v1
`dr-restore.sh` re-puts the kv keys unchanged.

### Sqlite quiesce — why the restore isn't corrupt-but-loadable
A naive `tar` of a live sqlite DB can capture a torn write. `dr-backup.sh`:
1. **preferred:** `sqlite3 .backup` — an online, atomic, consistent copy WITHOUT
   stopping Forgejo;
2. **fallback (no sqlite3):** WAL checkpoint + copy the DB (+ any `-wal`/`-shm`);
3. **hard-consistent (documented):** stop Forgejo, `tar`, restart — for a paranoid
   cold backup.

`dr-reconstitute.sh --restore` then content-VERIFIES the restored DB: `.tables`
succeeds, an **admin row exists** in `user`, and the **named seed repo** is present
— not a healthz-only pass on an empty/torn DB (resolves the critique's STUB).

### Consistent etcd snapshot
`dr-backup.sh` streams `/v3/maintenance/snapshot` from the resolved **leader**
endpoint (a point-in-time bbolt snapshot) and records its revision. If the
maintenance API is unavailable the snapshot is omitted and the per-key manifest
still gives a portable restore — the backup never fails.

## On-mesh first line (DAR-39)

`dr-snapshot-onmesh.sh` produces a fresh `dr-<ts>.age` and copies it into
`$MCNF_MESHFS_DIR/dr/` (Syncthing-replicated), updating `dr/INDEX.json`
(ts + sha256 + size + components) and pruning to `MCNF_DR_KEEP` (default 14)
newest. It verifies the mesh dir is writable BEFORE relying on it. This is the
Full-tier `mcnf-dr-backup.{service,timer}` payload (leader-gated daily).

## Off-fleet (operator-run — the explicit NEXT step)

The on-mesh line survives single-node loss; a **total-fleet** loss needs an
off-fleet copy. Both off-fleet pushes are **operator-run** (the safety classifier
hard-blocks an automated agent from egress past the trust boundary):

```sh
# the newest DR artifact → s3://mcnf-dr-4533/age/  (Spaces key from the store)
automation/dr/dr-push-offfleet.sh            # --dry-run (default): show the command, push nothing
automation/dr/dr-push-offfleet.sh --push     # OPERATOR ONLY

# the SEPARATE master bundle (Nebula CA + mesh age identity), passphrase-sealed,
# to a DISTINCT keys/ prefix (the key cannot live inside the thing it decrypts):
export MDE_BACKUP_PASSPHRASE=…               # never logged
automation/dr/dr-ca-bundle.sh                # --dry-run (default)
automation/dr/dr-ca-bundle.sh --push         # OPERATOR ONLY → s3://mcnf-dr-4533/keys/
```

## Env (DAR-37, `dr-env.sh`)

Every DR script sources `dr-env.sh`, which resolves `MCNF_ETCD` from
`/etc/mackesd/etcd-endpoints` (the DAR-1b resolver — **NO** `http://172.20.145.192:2379`
default) and echoes the path defaults:

```sh
automation/dr/dr-env.sh print-config
# MCNF_ETCD / MCNF_AGE_KEY / MCNF_MESHFS_DIR=/mnt/mesh-storage /
# MCNF_FORGEJO_DATA=/var/lib/mcnf-forgejo / MCNF_DR_BUCKET=mcnf-dr-4533
```

## Backup / restore / reconstitute

```sh
automation/dr/dr-backup.sh                          # → a v2 dr-<ts>.age (prints the path)
automation/dr/dr-restore.sh <dr.age>                # safe temp prefix (/dr-restore-test/) by default
automation/dr/dr-restore.sh <dr.age> --prod         # restore to the ORIGINAL live keys (DANGER)

automation/dr/dr-reconstitute.sh --verify  <dr.age> # dearmor + list components, NO mutation (exit 0 = restorable)
automation/dr/dr-reconstitute.sh --restore <dr.age> # restore + content-verify (admin row + named repo)
```

## ⚠️ Separate-key-backup caveat

The mesh age **identity** (`$MCNF_AGE_KEY`) and the **Nebula CA** are NOT in the
`dr-<ts>.age` manifest and cannot be — a master key cannot live only inside the
thing it decrypts. Back them up via `dr-ca-bundle.sh` (passphrase-sealed, separate
`keys/` prefix). Without the age identity the manifest is undecryptable; `dr-backup.sh`
prints this reminder on every run.

## RPC

`mackesd` exposes the backup over the Bus action layer: topic `action/dc/dr-backup`,
request `{"confirm":true}` (confirm-gated), runs `dr-backup.sh`, reply
`{"ok":true,"path":"<dr-*.age path>"}`.
