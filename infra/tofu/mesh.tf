# DS-1 — the test mesh: N lighthouses + M peers cloned from the golden template.
# Data sources resolve names → XO ids at plan time (requires XO reachable + populated).

data "xenorchestra_pool" "fleet" {
  name_label = var.pool_name
}

data "xenorchestra_template" "golden" {
  name_label = var.template_name
  pool_id    = data.xenorchestra_pool.fleet.id
}

data "xenorchestra_network" "net" {
  name_label = var.network_name
  pool_id    = data.xenorchestra_pool.fleet.id
}

data "xenorchestra_sr" "store" {
  name_label = var.sr_name
  pool_id    = data.xenorchestra_pool.fleet.id
}

# cloud-init: seed the control-host SSH key + hostname so Ansible (DS-2) can reach each node.
resource "xenorchestra_cloud_config" "node" {
  name     = "mcnf-test-node"
  template = <<-EOF
    #cloud-config
    ssh_authorized_keys:
      - ${var.ssh_authorized_key}
    hostname: "{name}"
    package_update: false
  EOF
}

locals {
  nodes = concat(
    [for i in range(var.lighthouse_count) : { name = format("mcnf-lh%d", i + 1), role = "lighthouse" }],
    [for i in range(var.peer_count) : { name = format("mcnf-peer%d", i + 1), role = "peer" }],
  )
}

resource "xenorchestra_vm" "node" {
  for_each = { for n in local.nodes : n.name => n }

  name_label       = each.value.name
  name_description = "MCNF test fleet (${each.value.role}) — managed by OpenTofu (DS-1)"
  template         = data.xenorchestra_template.golden.id
  cloud_config     = xenorchestra_cloud_config.node.template
  cpus             = var.vcpus
  memory_max       = var.memory_gb * 1024 * 1024 * 1024

  network {
    network_id = data.xenorchestra_network.net.id
  }

  disk {
    sr_id      = data.xenorchestra_sr.store.id
    name_label = "${each.value.name}-root"
    size       = var.disk_gb * 1024 * 1024 * 1024
  }

  # Tag so teardown/snapshot selection targets the fleet without touching build-farm VMs.
  tags = ["mcnf-test-fleet", each.value.role]

  # Test VMs are cattle — let Tofu replace rather than block on in-place edits.
  lifecycle {
    create_before_destroy = false
  }
}
