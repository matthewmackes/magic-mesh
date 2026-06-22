# The golden template to clone (XCP-2). Gated: only read when the name is set,
# so an empty default doesn't fail plan on a not-yet-built template.
data "xenorchestra_template" "golden" {
  for_each   = local.active_build_vms
  name_label = var.golden_template_name
  pool_id    = data.xenorchestra_pool.p[each.key].id
}

# One build VM per pool. Inert until var.golden_template_name is set (XCP-2).
resource "xenorchestra_vm" "build" {
  for_each = local.active_build_vms

  name_label  = each.value.name
  name_description = "MCNF build farm node (managed by OpenTofu / DEVOPS-SUBSTRATE)"
  template    = data.xenorchestra_template.golden[each.key].id
  auto_poweron = true # survive a host reboot (matches the bash provisioner)

  # The golden template is UEFI (XCP-2); a clone defaults to BIOS unless asked,
  # so pin it explicitly to keep the farm on the verified UEFI path.
  hvm_boot_firmware = "uefi"
  secure_boot       = false

  cpus       = var.build_vcpus
  memory_max = var.build_memory_gib * 1024 * 1024 * 1024

  cloud_config = templatefile("${path.module}/cloud-init/build-vm.yaml.tftpl", {
    hostname   = each.value.name
    ip_cidr    = each.value.ip_cidr
    gateway    = var.gateway
    dns        = join(";", var.dns)
    ssh_pubkey = trimspace(file(var.ssh_pubkey_path))
  })

  network {
    network_id = data.xenorchestra_network.lan[each.key].id
  }

  disk {
    sr_id      = data.xenorchestra_sr.local[each.key].id
    name_label = "mcnf-build-root"
    size       = var.build_disk_gib * 1024 * 1024 * 1024
  }

  # cloud-init grows the rootfs + reinstalls the toolchain; the target dir +
  # build cache living on the disk are not tofu's concern.
  lifecycle {
    ignore_changes = [cloud_config]
  }
}

variable "ssh_pubkey_path" {
  description = "Public key authorized on the build VMs' mm user."
  type        = string
  default     = "/root/.ssh/mackes_mesh_ed25519.pub"
}
