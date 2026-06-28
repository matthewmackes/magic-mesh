# DAR-12 / DAR-13 — the ONE control VM, cloned from MDE-VM-golden on the founding
# dom0 and sized by the tier. This is a CREATE-with-seed (not the adopt path the
# xen-xapi build VMs use): a fresh clone that self-enrolls via cloud-init.
#
# CLOUD-INIT SEED SEAM (xenserver 0.2.x): unlike the xenorchestra provider (which
# has a first-class `cloud_config`), the XAPI-native `xenserver_vm` resource (0.2.2)
# exposes NO cloud-init argument — its only injection point is `other_config`
# (the XAPI VM other-config map). XCP-NG's cloud-init NoCloud datasource reads the
# user-data from other-config under `vm-data/user-data`, so the rendered template
# is delivered there. (The 0.2.x create-with-seed path is the CONTROLVM-9 risk the
# design flags for LIVE attach-verification on a throwaway clone before relying on
# it; this root produces the HCL, the attach is verified live, operator-gated.)
resource "xenserver_vm" "control" {
  provider      = xenserver.founder
  name_label    = var.control_vm_name
  template_name = var.golden_template_name

  static_mem_max = local.shape.mem_gib * local.gib
  vcpus          = local.shape.vcpus

  # Don't block apply on a DHCP lease report; the VM takes a STATIC LAN IP from the
  # NM keyfile (cloud-init) and its durable identity is the overlay IP from join.
  check_ip_timeout = 0

  network_interface = [{ device = "0", network_uuid = var.network_uuid }]

  # The cloud-init user-data carries NO secret in cleartext-at-rest sense beyond the
  # join token, which itself arrives via the sensitive var.join_token (sourced from
  # the secret store at apply) — there is NO age private key and NO unseal passphrase
  # in this payload (the VM mints its OWN key at first boot via mcnf-secret.sh
  # init-self; see DAR-13). The whole map is marked sensitive so the rendered
  # user-data (which embeds the token) is redacted from plan/CLI output and is not
  # echoed; the token never appears as a literal in any committed file.
  other_config = {
    "vm-data/user-data" = templatefile("${path.module}/cloud-init/control-vm.yaml.tftpl", {
      hostname        = var.control_vm_name
      ip_cidr         = var.control_ip_cidr
      gateway         = var.gateway
      dns             = join(";", var.dns)
      ssh_pubkey      = trimspace(file(var.ssh_pubkey_path))
      join_token      = var.join_token
      mesh_id         = var.mesh_id
      qnm_path        = var.qnm_path
      lighthouse_ips  = jsonencode(var.lighthouse_overlay_ips)
      etcd_anchors    = join(",", var.etcd_anchor_overlay_ips)
      backoffice_tier = var.backoffice_tier
    })
  }

  # The 0.2.x provider can't round-trip a few create-only fields, and the clone's
  # disk/boot details are golden-template-owned; ignore them so a converged VM
  # plans clean (the adopt recipe from xen-xapi). cloud_config equivalent
  # (other_config) is also ignored post-create: re-seeding a running VM would
  # propose a spurious change, and re-enroll is a runtime concern, not tofu's.
  lifecycle {
    ignore_changes = [
      template_name, boot_mode, boot_order, cores_per_socket,
      dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description,
      cdrom, hard_drive, other_config,
    ]
  }
}
