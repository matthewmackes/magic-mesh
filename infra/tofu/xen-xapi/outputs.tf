# build_farm — every provisioned build VM as name → { uuid, ip } (DAR-31).
# Merged across the four per-pool for_each resources (build_xhs/kvm/big/x194). uuid is
# the XAPI-assigned VM UUID; ip is the static LAN address from the VM's spec
# (ip_cidr without the /prefix) — the IP xcp-build.sh reaches the VM on, and a
# stable value at plan time (no dependency on the 0.2.x provider's computed
# default_ip, which it can't round-trip).
output "build_farm" {
  description = "XAPI-native build farm VMs: name → { uuid, ip }."
  value = {
    for k, v in merge(
      xenserver_vm.build_xhs,
      xenserver_vm.build_kvm,
      xenserver_vm.build_big,
      xenserver_vm.build_x194,
    ) :
    v.name_label => {
      uuid = v.uuid
      ip   = split("/", local.active_build_vms[k].ip_cidr)[0]
    }
  }
}
