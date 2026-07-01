# Zones & promotion pipeline

Operator-locked, 2026-06-22. The platform runs in **two separate zones**. Dev
builds on Xen; releases are **promoted** to the production zone (Eagle, then the
DigitalOcean lighthouses). The two zones are independent meshes with independent
IaC state — no single `tofu apply` ever spans both.

## The two zones

| | **DEV** | **PRODUCTION** |
|---|---|---|
| Compute | **Xen only** — 4 XCP-ng dom0s | **DigitalOcean** droplets **+ Eagle** |
| Hosts | XEN-HOME-SERVICES, KVM-XCP1, XEN-BIGBOY, XEN-194 | `lighthouse-01` (live), `lighthouse-02` (soon), **Eagle** (LAN, production member) |
| Workloads | build + test / CI only | the real fleet (lighthouse-anchored) |
| IaC | `infra/tofu/` (xenorchestra) + Ansible | `infra/tofu/zone1-do/` (digitalocean) + `doctl` |
| Tofu state | `infra/tofu/terraform.tfstate` | `infra/tofu/zone1-do/terraform.tfstate` |

Eagle is a **production** node (the operator-review machine), not a dev/test box.
It joins the production mesh anchored by the DO lighthouse(s).

The dev-zone build farm is **4 dom0s / 9 heavy build slots** (XEN-HOME-SERVICES/.50,
KVM-XCP1/.90, XEN-BIGBOY/.130, XEN-194/.170) — canonical roster in
`install-helpers/farm-topology.sh`.

## Promotion pipeline

```
        ┌──────────────┐      promote       ┌──────────────┐      promote       ┌──────────────┐
        │   1. BUILD   │ ─────────────────▶ │  2. EAGLE    │ ─────────────────▶ │   3. DO      │
        │   Dev / Xen  │                    │  Production  │                    │  Production  │
        └──────────────┘                    └──────────────┘                    └──────────────┘
  cut next-version RPM on the          install RPM on the operator-          roll the lighthouse
  build farm (.50/.90/.130/.170);      review node; join the                 droplet(s) to the new
  L0 build+unit → L1 install →         production mesh; operator             version
  L2 feature → L3 stability            reviews
  on the snapshot-reset pool
```

| Stage | Zone | Action |
|-------|------|--------|
| 1. Build | Dev / Xen | cut the RPM on the farm, run the L0–L3 test pyramid on the snapshot-reset pool |
| 2. Promote → Eagle | Production | install on Eagle, join the production mesh, operator review |
| 3. Promote → DO | Production | roll the DigitalOcean lighthouse droplet(s) |

## OpenTofu management

OpenTofu manages **all** resources in both zones (operator: "OpenTofu can manage
all resource"). The DigitalOcean half lives in `infra/tofu/zone1-do/` and was
imported from the live account — `tofu plan` is clean. The API token is kept
off-repo (`/root/.mcnf-do-token`, `0600`); `doctl` (imperative) and Tofu
(declarative) share it. See `infra/tofu/zone1-do/README.md`.
