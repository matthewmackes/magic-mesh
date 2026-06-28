# Mesh secret store — age + etcd (DATACENTER-3 / DS-8, DAR-3 secret-zero)

Secrets are **age-encrypted** and stored in **etcd**, so the control plane carries
no host-local plaintext: any node holding ONE of the registered age identities
decrypts the same secret from the replicated store.

```
ciphertext              → etcd /mcnf/secret/<name>
legacy single recipient → etcd /mcnf/age-recipient        (back-compat)
per-node recipient set   → etcd /mcnf/age-recipients/<id>  (DAR-3)
```

The age **identity** (private) is the only host-local artifact (`/root/.mcnf-age-key`,
0600) — it is **never** printed, logged, or transmitted. Only the public recipient
(`age1…`) is ever written to the mesh.

## Endpoint resolution (v2 / DAR-1b)

`MCNF_ETCD` no longer defaults to the dead `172.20.145.192:2379`. Every etcd op
resolves the endpoint in order: explicit `MCNF_ETCD` → first entry of
`/etc/mackesd/etcd-endpoints` → **fail loud** with a `run setup-etcd.sh` hint.

## Use

```bash
./mcnf-secret.sh init                 # legacy: generate the mesh age key + publish the single recipient
./mcnf-secret.sh put do-token < file  # encrypt stdin → etcd (to the FULL recipient set)
./mcnf-secret.sh get do-token         # decrypt → stdout (exit 3 if absent)
./mcnf-secret.sh list                 # list stored secret names
./mcnf-secret.sh recipients           # list the recipient SET (public keys only)
```

## Secret-zero: the control VM mints its own key (DAR-3)

A fresh control VM is **NOT** handed any master key. There is no passphrase or
private key in tofu state. The flow is:

```bash
# On the control VM at first boot (private key never leaves the VM):
./mcnf-secret.sh init-self            # mints /root/.mcnf-age-key (0600); registers
                                      # ONLY its public recipient at /mcnf/age-recipients/<node-id>

# On the operator/leader (a holder of the CURRENT mesh age key):
./mcnf-secret.sh reseal-to age1<vm>   # re-encrypts every /mcnf/secret/* multi-recipient
                                      # so the VM's own key can now `get` every cred
# (./mcnf-secret.sh reseal-all re-seals to the full registered set, no extra arg)
```

The control VM **cannot self-reseal** — it holds no master key, so the values are
not decryptable with its fresh identity until the operator's `reseal-to` runs.
`reseal-to`/`reseal-all` must therefore be run by the operator/leader, by design.

### Re-seal atomicity

The whole walk is wrapped in an etcd **lease-backed lock** (`/mcnf/reseal/lock`,
create-if-absent CAS) so two operators can't interleave writes; a crashed holder
auto-releases after the lease TTL. A **completion marker** (`/mcnf/reseal/marker`)
records `{status:"started",…}` before the walk and `{status:"completed",…}` after
(or `{status:"failed",…}` on a decrypt failure) — so a crash mid-walk leaves an
*incomplete* marker a later run (or `backoffice-up` Phase 0) can detect. Each
secret is rewritten in a **single etcd put** (etcd per-key writes are atomic), and
the resealed count only advances after the put returns, so no key is left
half-written.

## Rotation

```bash
./mcnf-secret.sh rotate do-token < new-value                 # atomic overwrite (exit 3 if absent)
./mcnf-secret.sh rotate do-token --revoke-cmd 'doctl ...' < new-value   # + provider-side revoke
```

## DR CA/identity bundle — separate, passphrase-sealed (DAR-2)

The on-VM mechanism above moves only **public** recipients across the mesh. The
mesh CA + age identity are backed up by a SEPARATE operator-run, passphrase-sealed
bundle via the `mackesd secret-seal` / `secret-unseal` CLI (the ONE place the
Argon2id+XChaCha20 `ca::backup` envelope is used for arbitrary bytes):

```bash
mackesd secret-seal   --passphrase-file <0600-file> < ca-identity-blob > bundle.age
mackesd secret-unseal --passphrase-file <0600-file> < bundle.age       > ca-identity-blob
```

The passphrase comes from a **file** (0600), never argv/env — so it can't leak via
`ps` / `/proc/<pid>/{cmdline,environ}`.

## Self-test

`./mcnf-secret.sh selftest` runs an OFFLINE test that mocks etcd with a local dir
and **touches no live store**. It drives the real `init`/`put`/`init-self`/
`reseal-to`/`rotate` arms and asserts: two registered recipients both decrypt the
same secret after a reseal; the VM key file is 0600; the VM key cannot read before
reseal; the completion marker reaches `completed`; and **no secret value and no age
private key ever appear in any logged output**.

## The credential set (DAR-5)

Every backoffice credential lives under `/mcnf/secret/<name>` as age ciphertext —
NO `/root/.mcnf-*` plaintext is required on a reconstituted control VM:

| secret name          | provider / use                         | old plaintext source (folded by DAR-5)        |
| -------------------- | -------------------------------------- | --------------------------------------------- |
| `do-token`           | DigitalOcean (zone1-do)                | (already folded — DATACENTER-3)               |
| `xapi-password`      | XCP-ng XAPI (xen-xapi, control-vm)     | (already folded — DATACENTER-3)               |
| `xo-token`           | Xen Orchestra (deprecated env.sh)      | `/root/.mcnf-xo-token` (xo-mint-token.sh)     |
| `edgeos-cred`        | EdgeOS SSH password (edgeos)           | `/root/.mcnf-ubnt-cred`                       |
| `dns-token`          | DNS-01 provider token (reserved)       | none yet — `$MCNF_DNS_TOKEN_FILE` if present  |
| `sccache-access-key` | minio/S3 root user (sccache farm)      | in-repo literal `mcnfcache` (now removed)     |
| `sccache-secret-key` | minio/S3 root password                 | in-repo literal `mcnfcache2026` (now removed) |
| `join-token`         | `mackesd join` enroll (control-vm)     | tofu var → cloud-init (never a file)          |
| `dr-spaces-key`      | off-fleet DO Spaces push (DR)          | (folded by the DR scripts — DAR-39/41)        |
| `forgejo-*`          | CI admin pass / runner token / SECRET  | sealed by forgejo-seed.sh                     |

### Folding the legacy plaintext files

`automation/secrets/migrate-cred-files.sh` reads each present plaintext source and
`put`s it into the store — value piped on **stdin** (never argv/log). It is
**dry-run by default** (touches NO live store); a real fold needs `--apply` and is
OPERATOR-run:

```bash
./migrate-cred-files.sh                 # PLAN: which sources are present (no writes)
./migrate-cred-files.sh --apply         # OPERATOR: fold every present source
./migrate-cred-files.sh --apply --only edgeos-cred,xo-token   # a subset
./migrate-cred-files.sh selftest        # offline test (stub put; NO live store)
```

After a fold, an operator re-seals a fresh control VM in (`reseal-to <vm-recipient>`),
verifies the consumers resolve from the store, then `shred -u`s the old files.

## In use

Tofu roots resolve their creds from the store via their `env.sh` (cp from
`env.sh.example`, gitignored). The shared `automation/lib/tofu-env.sh` (DAR-10) is
the cred-source indirection — `tofu_env_load <root>` unseals the right secrets into
process-scoped env with THIS node's own age key, and (for EdgeOS) materializes the
cred into a **tmpfs 0600 file shredded on exit**:

- `infra/tofu/zone1-do` → `DIGITALOCEAN_TOKEN` = `get do-token`
- `infra/tofu/xen-xapi` → `TF_VAR_xapi_password` = `get xapi-password`
- `infra/tofu/control-vm` → `TF_VAR_xapi_password` + `TF_VAR_join_token`
- `infra/tofu/edgeos` → `TF_VAR_edgeos_cred_file` = tmpfs path holding `get edgeos-cred`
- `infra/tofu/env.sh.example` (XO, deprecated) → `XOA_TOKEN` = `get xo-token`
- `automation/cache/sccache-backend-up.sh` + `infra/ansible/sccache.yml` →
  `get sccache-access-key` / `get sccache-secret-key`

Verified: with the host cred file removed, `tofu plan` still resolves the cred from
the store (`0-destroy`), and etcd holds the `age-encryption.org/v1` ciphertext,
never the plaintext.

## Remaining (follow-ups, tracked in WORKLIST DAR-*)

- DAR-13/19: the control-VM cloud-init runs `init-self` at boot; the provisioner
  prompts the operator for `reseal-to <vm-recipient>` (live-gated).
