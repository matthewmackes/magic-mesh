output "build_farm" {
  description = "XAPI-native build farm VMs (uuid per pool)."
  value = {
    "mcnf-build-50" = xenserver_vm.build_50.uuid
    "mcnf-build-51" = xenserver_vm.build_51.uuid
    "mcnf-build-52" = xenserver_vm.build_52.uuid
  }
}
