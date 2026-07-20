# Per-appliance router instances (`appliances/`)

WL-RUN-006 generalized this root from a single hardcoded appliance (the farm
gateway) to **per-appliance** management: one tofu **state** + one **tfvars** per
router, so more than one Vyatta-family appliance (EdgeOS / VyOS) can be managed
from this single root without them sharing state or clobbering each other.

## The general form

An appliance is keyed by its **gateway MAC** — the same `router/<mac>` id the
mackesd `router_registry` seals a credential under and the Device-Manager surface
renders as a `HostKind::Router`. Each appliance is one instance:

| Piece | Path | Selects |
| --- | --- | --- |
| Variables | `appliances/<mac>.tfvars` | `appliance_id`, `edgeos_host`, reservations, firewall/NAT/VPN |
| State | `state/router/<mac>` (etcd http backend key) | `appliances/<mac>.backend.hcl` |
| Credential | `router/<mac>` in the mesh secret store | `edgeos_cred_file` (unsealed by `tofu-env.sh`) |

Drive an instance through the wrapper (it selects the right var-file **and**
backend-config, and refuses a mismatched pair):

```sh
# manage a second router (converge its config):
./scripts/tofu-appliance.sh aa:bb:cc:dd:ee:ff plan
./scripts/tofu-appliance.sh aa:bb:cc:dd:ee:ff apply

# the farm gateway is the reserved `gateway` alias (auto-loaded terraform.tfvars):
./scripts/tofu-appliance.sh gateway plan
```

## The grandfathered gateway

The **farm gateway** (172.20.0.1) is the default instance: its values live in the
auto-loaded `terraform.tfvars` (`appliance_id = "gateway"`) and its LIVE state is
grandfathered at `state/edgeos` (backend.tf). Re-keying it to `state/router/<mac>`
is an **operator-gated migration** (a `tofu init -migrate-state`, same class as the
local→etcd migration in the top-level README), never automatic — so nothing here
moves the gateway's live state on its own.

## Adding a second appliance

1. Seal the credential: `router/<mac>` in the mesh secret store.
2. Copy the templates and fill them in:
   ```sh
   cp appliances/example-router.tfvars.example       appliances/aa-bb-cc-dd-ee-ff.tfvars
   cp appliances/example-router.backend.hcl.example  appliances/aa-bb-cc-dd-ee-ff.backend.hcl
   ```
   (The wrapper normalizes MAC `:` → `-` for the filename, so both
   `aa:bb:cc:dd:ee:ff` and `aa-bb-cc-dd-ee-ff` select the same files.)
3. `./scripts/tofu-appliance.sh aa:bb:cc:dd:ee:ff plan` → review → `apply`.

Real `appliances/*.tfvars` and `appliances/*.backend.hcl` are **gitignored** (they
carry per-site reservations + backend addresses); only the `*.example` templates
are tracked.
