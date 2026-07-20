# WL-ARCH-001 Phase B — the cloud root: provision the libvirt network + the
# declared VM set. Ansible (the configure leg) takes over from here, driving each
# VM's /etc/mackesd/site.yml convergence over the mesh-derived dynamic inventory.

# The libvirt network on the Nebula-adjacent bridge (replaces Neutron).
module "network" {
  source = "./modules/network"

  name      = var.network.name
  mode      = var.network.mode
  bridge    = var.network.bridge
  cidr      = var.network.cidr
  autostart = var.network.autostart
}

# The shared base OS image, imported once into the pool. Each VM's root disk is a
# copy-on-write clone of it (bootc image-mode base; extends packaging/bootc/).
resource "libvirt_volume" "base" {
  name   = "${var.network.name}-base.qcow2"
  pool   = var.pool
  source = var.base_image_source
  format = "qcow2"
}

# One libvirt domain per declared VM — a CoW clone of the base + a mesh-join
# cloud-init disk. `for_each` makes the set declarative: add/remove a key in
# var.vms to create/destroy a VM (the Heat/Nova verb replacement).
module "vm" {
  source   = "./modules/vm"
  for_each = var.vms

  name           = each.key
  vcpu           = each.value.vcpu
  memory_mb      = each.value.memory_mb
  disk_gb        = each.value.disk_gb
  pool           = var.pool
  base_volume_id = libvirt_volume.base.id
  network_id     = module.network.network_id

  # Mesh-join cloud-init (SEC-001 join path). The join token is the sensitive
  # value the mde-seal bridge (secrets.tf) unsealed at apply time — rendered here
  # so the module never re-reads the store.
  user_data = templatefile("${path.module}/cloud-init/mesh-join.yaml.tftpl", {
    hostname              = each.key
    ssh_authorized_key    = var.mesh_join.ssh_authorized_key
    lighthouse_overlay_ip = var.mesh_join.lighthouse_overlay_ip
    join_token            = local.join_token
  })
}
