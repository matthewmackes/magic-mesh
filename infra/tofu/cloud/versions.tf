# WL-ARCH-001 Phase B — the OpenTofu + Ansible cloud backend (provider + version
# pins). This root REPLACES the deleted OpenStack control plane: it provisions
# local libvirt/KVM VMs declaratively (OpenTofu) and hands the configure step to
# Ansible (see automation/ansible/). Local-first (E12) — no external cloud.
#
# The dmacvicar/libvirt provider drives the local libvirtd over its native API
# (qemu:///system by default); external + null back the mde-seal secrets bridge
# (secrets.tf) and the mesh-join cloud-init hashing, matching the edgeos root's
# "the script is the engine" idiom for the bits no native provider covers.
terraform {
  required_version = ">= 1.6"
  required_providers {
    libvirt = {
      # Pinned to 0.8.x: the stable classic-block schema (disk {} /
      # network_interface {} / graphics {}). The 0.9.x line rewrote domain devices
      # into a single `devices` attribute — a different, still-settling contract.
      source  = "dmacvicar/libvirt"
      version = "~> 0.8.3"
    }
    external = {
      source  = "hashicorp/external"
      version = "~> 2.3"
    }
    null = {
      source  = "hashicorp/null"
      version = "~> 3.2"
    }
  }
}
