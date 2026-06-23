# Zone 1 (Production) — DigitalOcean as code

OpenTofu module that manages the **production** DigitalOcean fleet: the mesh
lighthouses (soon two), the Asterisk/VoIP droplet, the `matthewmackes.com` DNS
zone, and the account's SSH keys.

## Zones (operator, 2026-06-22)

| Zone | Compute | Workloads |
|------|---------|-----------|
| **Production** | **DigitalOcean** droplets (`lighthouse-01`, soon `lighthouse-02`) + **Eagle** (LAN node, production *member* of the lighthouse-anchored mesh) | the real fleet |
| **Dev** | **Xen** hosts only (`../` — the xenorchestra state: 3 dom0s + build/test VMs) | dev/build/test only |

The two zones are **separate Tofu states** on purpose — no single `tofu apply`
spans production DO and the dev Xen farm. OpenTofu manages **all** resources in
both; this directory is the production/DO half.

## Use

```bash
cd infra/tofu/zone1-do
cp env.sh.example env.sh          # reads /root/.mcnf-do-token (0600, off-repo)
source ./env.sh
tofu plan                         # should be clean once imported (see below)
```

## First-time import (already-live resources)

The DO account predates this module, so existing resources are **imported**, not
created. `tofu-import.sh` does the whole set (droplets, domain, DNS records, SSH
keys) idempotently; after it runs, `tofu plan` is clean (the only `ignore_changes`
exist so DO-computed/force-new attributes don't churn a routine plan).

## Grow path — a second lighthouse

`lighthouse-02`/`-03` A records already exist in DNS but point at **destroyed**
droplets (stale from the old fleet). To stand up the real second production
lighthouse, uncomment the `lighthouse_04`-style block in `main.tf` (rename to
`-02`), point its A record at the new IP, `tofu apply`, then bootstrap `mackesd`
and `mackesd found --role lighthouse` on it.

## Token

`DIGITALOCEAN_TOKEN` comes from `/root/.mcnf-do-token` (0600), extracted from the
doctl `mackes` context. It is **never** committed. `doctl` (CLI, imperative) and
this module (declarative) share the same token.
