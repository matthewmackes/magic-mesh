locals {
  # The trigger hash: any change to the desired reservation set (or the
  # network/subnet it lives under) re-runs the converge provisioner.
  mappings_json = jsonencode(var.static_mappings)
}

# Converge the EdgeOS DHCP static-mappings to exactly var.static_mappings,
# via direct config edit + reload (configure/set/delete/commit/save over SSH).
# Idempotent: the script no-ops (no commit) when the router already matches.
resource "null_resource" "dhcp_static_mappings" {
  triggers = {
    mappings = local.mappings_json
    network  = var.shared_network
    subnet   = var.subnet
    script   = filemd5("${path.module}/scripts/apply-dhcp.sh")
  }

  provisioner "local-exec" {
    interpreter = ["/usr/bin/env", "bash"]
    command     = "${path.module}/scripts/apply-dhcp.sh"
    environment = {
      EDGEOS_HOST      = var.edgeos_host
      EDGEOS_USER      = var.edgeos_user
      EDGEOS_CRED_FILE = var.edgeos_cred_file
      EDGEOS_NETWORK   = var.shared_network
      EDGEOS_SUBNET    = var.subnet
      EDGEOS_DESIRED   = local.mappings_json
    }
  }
}

# Poll the live DHCP leases (read-only) — surfaced as an output so
# `tofu output dhcp_leases` is the "poll for DHCP addresses" command.
data "external" "dhcp_leases" {
  program = ["/usr/bin/env", "bash", "${path.module}/scripts/poll-leases.sh"]
  query = {
    host      = var.edgeos_host
    user      = var.edgeos_user
    cred_file = var.edgeos_cred_file
  }
}
