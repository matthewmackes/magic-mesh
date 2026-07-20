# WL-ARCH-006 U11 — the cloud root: partition the declared workload set by delivery
# type and converge the local hypervisor / container host to it. The libvirt network
# is shared; VM-family workloads (desktop/service/app) and Android (Cuttlefish)
# workloads become libvirt domains; service-container workloads become Quadlet units
# the Ansible container-host role installs. Ansible (the configure leg) takes over
# from here, driving each workload's /etc/mackesd/site.yml convergence over the
# mesh-derived dynamic inventory.

locals {
  # Partition var.vms by delivery type (the reenvisioned per-delivery-type cockpit
  # axis). Adding/removing a key in the rendered per-node tfvars creates/destroys
  # exactly that workload on this placement node.
  vm_workloads = {
    for name, w in var.vms : name => w
    if contains(["desktop_vm", "service_vm", "app_vm"], w.delivery_type)
  }
  android_workloads = {
    for name, w in var.vms : name => w
    if w.delivery_type == "android_vm"
  }
  container_workloads = {
    for name, w in var.vms : name => w
    if w.delivery_type == "service_container"
  }

  # Mesh-join cloud-init, rendered once per libvirt-domain workload (VM + Android).
  # The join token is the sensitive value the mde-seal bridge (secrets.tf) unsealed
  # at apply time.
  domain_user_data = {
    for name, w in merge(local.vm_workloads, local.android_workloads) :
    name => templatefile("${path.module}/cloud-init/mesh-join.yaml.tftpl", {
      hostname              = name
      ssh_authorized_key    = var.mesh_join.ssh_authorized_key
      lighthouse_overlay_ip = var.mesh_join.lighthouse_overlay_ip
      join_token            = local.join_token
    })
  }
}

# The libvirt network on the Nebula-adjacent bridge (replaces Neutron).
module "network" {
  source = "./modules/network"

  name      = var.network.name
  mode      = var.network.mode
  bridge    = var.network.bridge
  cidr      = var.network.cidr
  autostart = var.network.autostart
}

# The shared VM-family base OS image, imported once into the pool. Each VM's root
# disk is a copy-on-write clone of it (bootc image-mode base; extends
# packaging/bootc/). Only imported when a VM-family workload is declared.
resource "libvirt_volume" "base" {
  count = length(local.vm_workloads) > 0 ? 1 : 0

  name   = "${var.network.name}-base.qcow2"
  pool   = var.pool
  source = var.base_image_source
  format = "qcow2"
}

# The Debian Cuttlefish base image, imported once when an android_vm workload is
# declared (the nested-virt L1 host base — see modules/android).
resource "libvirt_volume" "android_base" {
  count = length(local.android_workloads) > 0 ? 1 : 0

  name   = "${var.network.name}-android-base.qcow2"
  pool   = var.pool
  source = var.android_base_image_source
  format = "qcow2"
}

# One libvirt domain per declared VM-family workload — a CoW clone of the base + a
# mesh-join cloud-init disk. `for_each` makes the set declarative.
module "vm" {
  source   = "./modules/vm"
  for_each = local.vm_workloads

  name           = each.key
  vcpu           = each.value.vcpu
  memory_mb      = each.value.memory_mb
  disk_gb        = each.value.disk_gb
  pool           = var.pool
  base_volume_id = one(libvirt_volume.base[*].id)
  network_id     = module.network.network_id
  user_data      = local.domain_user_data[each.key]
}

# One Cuttlefish L1 VM per declared android_vm workload — a nested-virt-capable
# Debian host that runs `cvd` (the Android guest lives in crosvm inside it). The
# module raises the size to its nested-virt floor (>=8G / >=4 vcpu / >=80G).
module "android" {
  source   = "./modules/android"
  for_each = local.android_workloads

  name              = each.key
  vcpu              = each.value.vcpu
  memory_mb         = each.value.memory_mb
  disk_gb           = each.value.disk_gb
  pool              = var.pool
  base_volume_id    = one(libvirt_volume.android_base[*].id)
  network_id        = module.network.network_id
  user_data         = local.domain_user_data[each.key]
  network_isolation = each.value.network_isolation
}

# One container spec per declared service_container workload — the desired-state the
# Ansible container-host role renders into a rootless Quadlet `.container` unit (no
# libvirt resource; tofu owns the DECLARATION, Ansible owns the install).
module "container" {
  source   = "./modules/container"
  for_each = local.container_workloads

  name              = each.key
  image             = each.value.image
  vcpu              = each.value.vcpu
  memory_mb         = each.value.memory_mb
  network_isolation = each.value.network_isolation
  network_name      = var.network.name
}
