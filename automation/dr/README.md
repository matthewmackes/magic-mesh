# DATACENTER-23 — disaster-recovery backup of the substrate

The MCNF substrate has **no fixed center**. The recoverable system state lives in
the replicated etcd store:

- `/tofu/state/*` — the OpenTofu IaC states (what infrastructure exists, plus the
  provider resource IDs needed to re-adopt it).
- `/mcnf/secret/*` — the mesh secrets. These are **already age-encrypted** in etcd
  by `automation/secrets/mcnf-secret.sh`, so the manifest holds ciphertext.
- `/mcnf/age-recipient` — the public recipient, carried so a restore can re-encrypt
  new secrets against the same identity.
- the **Nebula CA** (`ca.crt` + `ca.key` from `${MCNF_CA_DIR:-/var/lib/mackesd/nebula-ca}`)
  — DATACENTER-23, manifest `dr_backup_version: 2`. The CA is the **root of the
  mesh's identity**; folding it into the same age manifest means one artifact +
  the out-of-band age key re-founds the *same* Nebula (existing peer certs stay
  valid). The CA private key is only ever written **age-encrypted** to the mesh
  recipient, so the artifact at rest never exposes a usable CA key.

`dr-backup.sh` pulls those three sources via the etcd v3 range API (read-only,
base64 keys/values exactly like `mcnf-secret.sh`), assembles a JSON manifest, and
`age`-encrypts the whole manifest to the mesh recipient. The on-disk artifact is
double-protected for secrets (age-in-age) and single-age for the tofu state.

## What is backed up

| Source                | Contents                                  |
|-----------------------|-------------------------------------------|
| `/tofu/state/*`       | OpenTofu states (infra inventory + IDs)   |
| `/mcnf/secret/*`      | mesh secrets (already age-encrypted)      |
| `/mcnf/age-recipient` | the mesh public recipient                 |

Output: `${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}/dr-<UTC-timestamp>.age`
(timestamp `YYYYMMDDTHHMMSSZ`). The file is `age` ciphertext — `head` shows the
`age-encryption.org` header.

## Off-fleet push

So a LAN-wide loss can't take the only DR copy with it, the produced artifact is
optionally pushed off-fleet:

- `MCNF_DR_OFFFLEET` — an scp/rsync destination (e.g. `user@host:/backups/`);
  `dr-backup.sh`/`dr-ca-backup.sh` prefer `rsync`, falling back to `scp`.
- `MCNF_DR_OFFFLEET_CMD` — a generic escape hatch: a command run with the
  artifact path appended (e.g. `rclone copyto`, `b2 upload-file my-bucket`).

A push failure **warns but never fails the backup** — the local copy is kept.

## ⚠️ Separate-key-backup caveat

The mesh age **identity** (private key, `${MCNF_AGE_KEY:-/root/.mcnf-age-key}`) is
**NOT** in this backup and CANNOT be — the master key cannot live only inside the
thing it decrypts. Without the age identity the backup is **undecryptable**. Back
the age key up **SEPARATELY and securely** (offline / out-of-band). With it, a
single `dr-*.age` now restores the tofu state, the secrets, **and** the Nebula CA.
`dr-backup.sh` prints this reminder on every run.

## CA-only backup (separate-key discipline)

`dr-ca-backup.sh` backs up **only** the Nebula CA, age-encrypted to the mesh
recipient, on its own cadence / destination (ideally cold storage):

```sh
automation/dr/dr-ca-backup.sh           # → dr-ca-<ts>.age (exit 3 if not the CA holder)
```

## Rebirth (guided control-plane restore)

`dr-rebirth.sh` brings the no-fixed-center control plane back from cold:
**restore** etcd state → **re-found** the Nebula CA on disk → **re-elect** a
leader (restart `mackesd`). **Safe by default** (dry run validates + prints the
plan, writes nothing); `--execute` performs the rebirth and refuses if etcd is
unreachable (never a half-clobber).

```sh
automation/dr/dr-rebirth.sh ~/mcnf-dr-backups/dr-<ts>.age            # dry run (plan only)
automation/dr/dr-rebirth.sh ~/mcnf-dr-backups/dr-<ts>.age --execute  # DANGER: perform it
```

## Backup

```sh
automation/dr/dr-backup.sh
# prints the path to the dr-<ts>.age artifact
```

Read-only on etcd. Env: `MCNF_ETCD`, `MCNF_AGE_KEY`, `MCNF_DR_DIR`.

## Restore

By default the restore lands under a **temp prefix** (`/dr-restore-test/`) so a
round-trip can be verified without touching production. The original key
`/tofu/state/xen-xapi` is rewritten to `/dr-restore-test/tofu/state/xen-xapi`.

```sh
# safe round-trip into the temp prefix (default):
automation/dr/dr-restore.sh ~/mcnf-dr-backups/dr-<ts>.age

# explicit temp prefix:
automation/dr/dr-restore.sh ~/mcnf-dr-backups/dr-<ts>.age /dr-restore-test/

# DANGER: restore to the ORIGINAL live keys (clobbers production):
automation/dr/dr-restore.sh ~/mcnf-dr-backups/dr-<ts>.age --prod
```

Restore decrypts with `age -d -i $MCNF_AGE_KEY` and re-`put`s each key. The
secret values stay age-encrypted (they are restored as the ciphertext they were
stored as), so production secrets are never written in plaintext.

## RPC

`mackesd` exposes the DR flows over the Bus action layer (`ipc/host_ops.rs`):

- `action/dc/dr-backup` `{"confirm":true}` → `{"ok":true,"path":"<dr-*.age>"}`.
- `action/dc/dr-ca-backup` `{"confirm":true}` → CA-only backup path.
- `action/dc/dr-rebirth` `{"file":"<dr-*.age>"}` → dry-run plan; add
  `{"execute":true,"confirm":true}` to perform the live rebirth. `file` is
  validated (`.age`, `[A-Za-z0-9._/-]`, no `..`) before any spawn.

All `action/dc/*` verbs pass the DATACENTER-7 RBAC gate first (mutating DR verbs
require the `operator` role).
