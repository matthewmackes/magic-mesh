# EdgeOS DHCP-as-code (`infra/tofu/edgeos/`)

Manages the **EdgeRouter @ 172.20.0.1** (EdgeOS v3.0.0, MIPS) DHCP **static
reservations** declaratively, and polls live **DHCP leases** — both through
OpenTofu. Isolated state from the farm-VM root so a bad apply here can't touch
Xen Orchestra.

## How it works
No native EdgeOS provider exists (the VyOS provider needs the VyOS HTTP API,
which EdgeOS lacks; VyOS can't run on this MIPS hardware). So this root drives
the device the EdgeOS-correct way — **direct edit + reload of the Vyatta config
over SSH**:

- `scripts/apply-dhcp.sh` — converges the router's static-mappings to exactly
  `var.static_mappings` (`configure` → `set`/`delete` the diff → `commit` →
  `save`). Idempotent: no config session is opened when already converged.
  Refuses to apply an empty set unless `EDGEOS_ALLOW_EMPTY=1`.
- `scripts/poll-leases.sh` — read-only `show dhcp leases`, surfaced as the
  `dhcp_leases` output.

The password is read from a `0600` cred file (`var.edgeos_cred_file`, default
`/root/.mcnf-ubnt-cred`) via `sshpass -f` — never inlined in config or argv.

## Usage
```sh
cd infra/tofu/edgeos
tofu init
tofu apply                       # converge reservations + refresh leases
tofu output dhcp_leases          # poll: ip => "mac|expiry|hostname"
tofu output lease_count
```

### Add / change / remove a reservation
Edit `terraform.tfvars` (`static_mappings` is the source of truth), then
`tofu apply`. Adding an entry creates it; removing an entry deletes it from the
router. `static_mappings` must list **every** reservation you intend to keep.

## Safety
- `terraform.tfvars` was imported from the live router on 2026-06-24 (10
  reservations) so the first apply is a no-op.
- The live config is backed up on the router at `/config/config.boot.bak-mcnf`.
- MAC/IPv4 shape is validated at plan time.
