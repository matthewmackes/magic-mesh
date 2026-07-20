terraform {
  required_providers {
    libvirt = {
      source = "dmacvicar/libvirt"
    }
  }
}

# The libvirt network the cloud VMs attach to (replaces Neutron). In bridge mode
# it rides an existing host bridge (the Nebula-adjacent br-mesh); in nat/route
# mode libvirt manages the subnet in var.cidr.
resource "libvirt_network" "this" {
  name      = var.name
  mode      = var.mode
  autostart = var.autostart

  # Bridge device only for bridge mode; the managed subnet only for nat/route.
  bridge    = var.mode == "bridge" ? var.bridge : null
  addresses = contains(["nat", "route"], var.mode) ? [var.cidr] : null

  dns {
    enabled = true
  }
}
