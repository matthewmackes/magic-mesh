# The MCNF build farm, XAPI-native (no XO). One VM per pool, ADOPTED via import
# (already provisioned), so ignore_changes covers the create-only / 0.2.x-non-
# round-trippable fields — an adopted VM plans clean (proven: DATACENTER-1).
resource "xenserver_vm" "build_50" {
  provider          = xenserver.xhs
  name_label        = "mcnf-build-50"
  template_name     = "MDE-VM-golden"
  static_mem_max    = 17179869184 # 16 GiB
  vcpus             = 4
  check_ip_timeout  = 0
  network_interface = [{ device = "0", network_uuid = "420c5872-dd49-af7f-fe4f-d5e2502429f8" }]
  lifecycle {
    ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]
  }
}
resource "xenserver_vm" "build_51" {
  provider          = xenserver.kvm
  name_label        = "mcnf-build-51"
  template_name     = "MDE-VM-golden"
  static_mem_max    = 17179869184 # 16 GiB
  vcpus             = 4
  check_ip_timeout  = 0
  network_interface = [{ device = "0", network_uuid = "85bc2d18-849b-4d9a-df9d-ab92ef1e58b8" }]
  lifecycle {
    ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]
  }
}
resource "xenserver_vm" "build_52" {
  provider          = xenserver.big
  name_label        = "mcnf-build-52"
  template_name     = "MDE-VM-golden"
  static_mem_max    = 25769803776 # 24 GiB
  vcpus             = 8
  check_ip_timeout  = 0
  network_interface = [{ device = "0", network_uuid = "8dee4afc-4fc7-60e5-0a3f-7b9b94954631" }]
  lifecycle {
    ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]
  }
}
