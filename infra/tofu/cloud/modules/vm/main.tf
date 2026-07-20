terraform {
  required_providers {
    libvirt = {
      source = "dmacvicar/libvirt"
    }
  }
}

# The VM's root disk — a copy-on-write clone of the shared base image, grown to
# the requested size.
resource "libvirt_volume" "root" {
  name           = "${var.name}.qcow2"
  pool           = var.pool
  base_volume_id = var.base_volume_id
  size           = var.disk_gb * 1024 * 1024 * 1024
  format         = "qcow2"
}

# The mesh-join cloud-init disk (SEC-001 join path). user_data carries the
# sensitive join token the mde-seal bridge unsealed at apply time.
resource "libvirt_cloudinit_disk" "this" {
  name      = "${var.name}-cloudinit.iso"
  pool      = var.pool
  user_data = var.user_data
}

# The domain. SPICE console binds locally (127.0.0.1) so the mesh console broker
# is the only reachable path (VDI-VM-1); the interface rides the cloud network.
resource "libvirt_domain" "this" {
  name      = var.name
  memory    = var.memory_mb
  vcpu      = var.vcpu
  cloudinit = libvirt_cloudinit_disk.this.id

  network_interface {
    network_id     = var.network_id
    wait_for_lease = false
  }

  disk {
    volume_id = libvirt_volume.root.id
  }

  console {
    type        = "pty"
    target_port = "0"
    target_type = "serial"
  }

  graphics {
    type        = "spice"
    listen_type = "address"
    autoport    = true
  }
}
