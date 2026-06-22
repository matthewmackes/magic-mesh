# Tofu state backend — etcd-backed (DATACENTER-2 / SUBSTRATE-V2)

OpenTofu `http`-backend service that stores Tofu **state + lock in etcd** (the
SUBSTRATE-V2 store), so IaC state is mesh-replicated and locked rather than a
single host's local file. Any leader-eligible node can plan/apply against the same
state — no-fixed-center IaC.

```
state → etcd /tofu/state/<name>     lock → etcd /tofu/lock/<name> (atomic txn)
```

## Run

```bash
./state-backend-up.sh          # podman container on :8390 → etcd (host network)
```

`tofu-state-etcd.py` is pure Python stdlib (runs on a stock python image). It is
stateless — all durable state is in etcd — so it can run on any node (or several
behind a VIP); restart-safe.

## Wire a workspace

```hcl
terraform {
  backend "http" {
    address        = "http://172.20.145.192:8390/state/<name>"
    lock_address   = "http://172.20.145.192:8390/state/<name>"
    unlock_address = "http://172.20.145.192:8390/state/<name>"
    lock_method    = "LOCK"
    unlock_method  = "UNLOCK"
  }
}
# then: tofu init -migrate-state
```

In use: `infra/tofu/xen-xapi` (`/state/xen-xapi`) + `infra/tofu/zone1-do`
(`/state/zone1-do`). Both migrated + plan clean from etcd; a held lock blocks a
concurrent `tofu plan` (verified).

## Caveat (full no-center)

etcd is a **single node** today (`172.20.145.192:2379`), so the store is durable +
shared + locked, but true mesh-replication needs etcd **clustered** across nodes —
that hardening is the remaining SUBSTRATE-V2 step.
