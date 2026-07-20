# The libvirt connection. Local-first (E12): the default is the local system
# hypervisor (qemu:///system). A remote dom0/seat can be driven by pointing
# `libvirt_uri` at qemu+ssh://root@<host>/system (over the Nebula overlay) — the
# SSH key is the mesh key, never a secret in this config.
provider "libvirt" {
  uri = var.libvirt_uri
}
