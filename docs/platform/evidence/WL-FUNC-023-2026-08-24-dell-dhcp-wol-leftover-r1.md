# WL-FUNC-023 leftover — Dell DHCP reservation, seat still unpowered (2026-08-24)

Operator authorized DHCP mutation. Red `AI-GENERATED-ALERT` and five-second
hold. `production_admitted` unchanged. Dell was not SSH-mutated.

## Observation

Control host `172.20.145.192` has a kernel route to `172.20.146.0/16` via
`eno1`, but ARP for `172.20.146.225` was `FAILED` then `INCOMPLETE`. Ping
and SSH time out (`No route to host`). Surface `172.20.146.79` on the same
prefix is reachable.

EdgeOS `172.20.0.1` DHCP leases have **no** `DELL-LAPTOP` / `172.20.146.225`
entry. Router ARP still maps `172.20.146.225` → `be:61:cf:5b:ea:4d` (locally
administered). Static mapping `DELL-LAPTOP-FEDORA` remains
`dc:a9:71:fe:58:71` → `172.20.145.25` (also silent).

## Corrected-forward

Additive EdgeOS commit (not a full `apply-dhcp.sh` converge, which would
drop live mappings absent from `infra/tofu/edgeos/terraform.tfvars`):

```
set service dhcp-server shared-network-name Home-Production-172_20 subnet 172.20.0.0/16 static-mapping DELL-LAPTOP ip-address 172.20.146.225
set service dhcp-server shared-network-name Home-Production-172_20 subnet 172.20.0.0/16 static-mapping DELL-LAPTOP mac-address 'be:61:cf:5b:ea:4d'
```

Wake-on-LAN magic packets were sent to both `be:61:cf:5b:ea:4d` and
`dc:a9:71:fe:58:71`. The host did not answer ICMP or ARP complete.

## Result

DHCP reservation is in place for the historical Dell underlay. The laptop
is not on the L2. Collaboration-identity dest and leftover-3 enroll for Dell
wait on power/SSH. Do not invent a mesh-id or copy Seat 15's node-scoped
receipt onto Dell.
