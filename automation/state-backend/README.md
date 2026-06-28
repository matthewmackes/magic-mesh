# Tofu state backend — etcd-backed (DATACENTER-2 / SUBSTRATE-V2 / DEVOPS-AUTOMATION-REBUILD)

OpenTofu `http`-backend service that stores Tofu **state + lock in etcd** (the
SUBSTRATE-V2 store), so IaC state is mesh-replicated and locked rather than a
single host's local file. Any leader-eligible node can plan/apply against the same
state — no-fixed-center IaC.

```
state → etcd /tofu/state/<name>     lock → etcd /tofu/lock/<name> (atomic txn)
```

The `/tofu/state/*` + `/tofu/lock/*` prefixes are **FROZEN** (the live prefixes DR
already captures) — do not rename.

## Endpoints + bind (DAR-1b / DAR-6 / DAR-7)

The etcd quorum is the **live lighthouses** (nyc3 / fra1 / sfo3 + Eagle), resolved
per node from `/etc/mackesd/etcd-endpoints` (written by `setup-etcd.sh`, parsed
like `substrate/etcd.rs::endpoints_from_file`). There is **no `172.20.145.192:2379`
default anymore** — that LAN control node is dead. Resolution order (shared
`automation/lib/etcd-endpoints.sh`): explicit `MCNF_ETCD` env → the endpoints file
→ **fail loud**. The service tolerates losing any single quorum member (naive
try-next failover).

The backend **binds the overlay IP** (`STATE_BACKEND_BIND`, default the detected
`nebula`/`mde-neb` address), never `0.0.0.0` — the overlay-only bind is the only
thing fronting plain-HTTP unauthenticated etcd.

## Run

```bash
./state-backend-up.sh          # podman container on <overlay-ip>:8390 (host net)
```

For the full ordered come-along (precheck → service → generate backends), use the
bootstrap hook:

```bash
./state-backend-bootstrap.sh --control-ip <overlay-ip>            # safe half
./state-backend-bootstrap.sh --control-ip <overlay-ip> --init-roots   # also tofu init -migrate-state (LIVE)
```

`tofu-state-etcd.py` is pure Python stdlib (runs on a stock python image). It is
stateless — all durable state is in etcd — so it can run on any node; restart-safe.

## Wire a workspace (DAR-8 / DAR-9)

OpenTofu backend blocks cannot interpolate variables, so the per-mesh **address is
not committed**. Each root's tracked `backend.tf` keeps only the lock/unlock
methods; the address comes from a generated, gitignored `<root>.backend.hcl`:

```bash
./gen-backend-config.sh --control-ip <overlay-ip>     # writes <root>.backend.hcl per root
tofu -chdir=infra/tofu/<root> init -backend-config=<root>.backend.hcl -migrate-state
```

`infra/tofu/backend.tf.tmpl` (`__CONTROL_IP__` / `__ROOT__`) is the single template
source for both the generated `.backend.hcl` and any literal-free skeleton.

## Roots

`xen-xapi` (`/state/xen-xapi`), `zone1-do` (`/state/zone1-do`), and — once migrated
— `edgeos` (`/state/edgeos`, DAR-9b) and `control-vm` (`/state/control-vm`). edgeos
was LOCAL state; its one-time local→etcd migration is **operator-run / live-gated**
via `migrate-edgeos-state.sh` (empty-target precheck + 0-add/0-destroy parity gate).
