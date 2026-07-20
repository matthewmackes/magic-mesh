# WL-ARCH-006 U12 — the Android (Cuttlefish) L1 VM wrapper.
#
# Two-layer Android: this module provisions the *outer* (L1) libvirt VM — a
# nested-virt-capable Debian host — over which the `cuttlefish_host` Ansible role
# runs `cvd start --start_vnc_server`. The Android guest itself lives in crosvm
# INSIDE this L1 VM (invisible to `virsh domdisplay`), so the guest screen is served
# by the in-guest VNC server, not a libvirt display.
#
# Nested virt is enabled by the host-passthrough CPU mode (exposes vmx/svm to the
# guest); the size is raised to a Cuttlefish-capable floor (>=8G RAM / >=4 vcpu /
# >=80G disk) regardless of the requested spec.

terraform {
  required_providers {
    libvirt = {
      source = "dmacvicar/libvirt"
    }
  }
}

locals {
  # The nested-virt floor — Cuttlefish needs real headroom, so a smaller request is
  # raised (never lowered) to these minimums.
  floor_vcpu      = 4
  floor_memory_mb = 8192
  floor_disk_gb   = 80

  vcpu      = max(var.vcpu, local.floor_vcpu)
  memory_mb = max(var.memory_mb, local.floor_memory_mb)
  disk_gb   = max(var.disk_gb, local.floor_disk_gb)
}

# The L1 VM's root disk — a copy-on-write clone of the Debian Cuttlefish base image,
# grown to the (floored) size.
resource "libvirt_volume" "root" {
  name           = "${var.name}.qcow2"
  pool           = var.pool
  base_volume_id = var.base_volume_id
  size           = local.disk_gb * 1024 * 1024 * 1024
  format         = "qcow2"
}

# The mesh-join cloud-init disk (SEC-001 join path).
resource "libvirt_cloudinit_disk" "this" {
  name      = "${var.name}-cloudinit.iso"
  pool      = var.pool
  user_data = var.user_data
}

# The L1 domain. host-passthrough CPU exposes nested-virt (vmx/svm) so `cvd` can run
# crosvm inside; a VNC head is present, but the Android guest screen is served by the
# in-guest `cvd --start_vnc_server` (console_proto = vnc, in-guest).
resource "libvirt_domain" "this" {
  name      = var.name
  memory    = local.memory_mb
  vcpu      = local.vcpu
  cloudinit = libvirt_cloudinit_disk.this.id

  # Nested virtualization: pass the host CPU through so the guest sees vmx/svm.
  cpu {
    mode = "host-passthrough"
  }

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

  # A VNC head on the L1 host; the Android guest screen itself rides the in-guest
  # `cvd --start_vnc_server` (surfaced as the console handle by the console-attach
  # verb).
  graphics {
    type        = "vnc"
    listen_type = "address"
    autoport    = true
  }
}
