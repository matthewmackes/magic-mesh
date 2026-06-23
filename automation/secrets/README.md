# Mesh secret store — age + etcd (DATACENTER-3 / DS-8)

Secrets are **age-encrypted** and stored in **etcd**, so the control plane carries
no host-local plaintext: any leader-eligible node holding the mesh age identity
decrypts the same secret from the replicated store.

```
ciphertext → etcd /mcnf/secret/<name>      recipient → etcd /mcnf/age-recipient
```

The only host-local artifact is the mesh age **identity** (`/root/.mcnf-age-key`,
0600) — distributed to eligible nodes like the mesh SSH key.

## Use

```bash
./mcnf-secret.sh init                 # generate the mesh age key + publish recipient
./mcnf-secret.sh put do-token < file  # encrypt stdin → etcd
./mcnf-secret.sh get do-token         # decrypt → stdout
./mcnf-secret.sh list
```

## In use

Both Tofu workspaces resolve their creds from the store (their `env.sh`):
- `infra/tofu/zone1-do` → `DIGITALOCEAN_TOKEN` = `mcnf-secret.sh get do-token`
- `infra/tofu/xen-xapi` → `TF_VAR_xapi_password` = `mcnf-secret.sh get xapi-password`

Verified: with the host cred file removed, `tofu plan` still resolves the XAPI
password from the store (`0-destroy`), and etcd holds the `age-encryption.org/v1`
ciphertext, never the plaintext.

## Remaining (to fully close DS-8)

- Add the UniFi cred (lands with the gateway source, DATACENTER-14).
- Distribute the age identity to other eligible nodes (today: one host).
- Wire the `datacenter_orchestrator` worker to the store; then retire the
  `/root/.mcnf-*` fallback files (kept for now).
