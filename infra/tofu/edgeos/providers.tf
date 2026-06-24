# EdgeOS (EdgeRouter @ 172.20.0.1) DHCP-as-code — isolated tofu root.
#
# WHY a separate root (not folded into infra/tofu/): this manages the LIVE
# network gateway via SSH config edits; keeping its state apart means a bad
# EdgeOS apply can never corrupt the farm-VM (Xen Orchestra) state next door.
#
# WHY no native provider: the device is an EdgeRouter ER-8 (MIPS64 Cavium
# Octeon, EdgeOS v3.0.0). The VyOS tofu provider (foltik/vyos) drives the VyOS
# HTTP API, which EdgeOS does not have, and VyOS can't run on MIPS hardware. So
# the EdgeOS-correct path is direct edit + reload of the Vyatta config over SSH
# (configure / set / delete / commit / save), wrapped here as null_resource +
# external data. Same declarative UX as a provider; the script is the engine.
terraform {
  required_version = ">= 1.6"
  required_providers {
    null     = { source = "hashicorp/null", version = "~> 3.2" }
    external = { source = "hashicorp/external", version = "~> 2.3" }
  }
}
