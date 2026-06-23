# DATACENTER-23 — disaster-recovery backup of the substrate

The MCNF substrate has **no fixed center**. The recoverable system state lives in
the replicated etcd store:

- `/tofu/state/*` — the OpenTofu IaC states (what infrastructure exists, plus the
  provider resource IDs needed to re-adopt it).
- `/mcnf/secret/*` — the mesh secrets. These are **already age-encrypted** in etcd
  by `automation/secrets/mcnf-secret.sh`, so the manifest holds ciphertext.
- `/mcnf/age-recipient` — the public recipient, carried so a restore can re-encrypt
  new secrets against the same identity.

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

## ⚠️ Separate-key-backup caveat

The mesh age **identity** (private key, `${MCNF_AGE_KEY:-/root/.mcnf-age-key}`) and
the **Nebula CA** are **NOT** in this backup and CANNOT be — the master key cannot
live only inside the thing it decrypts. Without the age identity this backup is
**undecryptable**. Back the age key and the Nebula CA up **SEPARATELY and
securely** (offline / out-of-band). `dr-backup.sh` prints this reminder on every run.

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

`mackesd` exposes the backup over the Bus action layer:

- topic `action/dc/dr-backup`, request `{"confirm":true}` (confirm-gated),
- runs `dr-backup.sh` from the repo root,
- reply `{"ok":true,"path":"<dr-*.age path>"}` or `{"error":"..."}`.
