# WL-ARCH-001 Phase B (item 3) — the mde-seal → IaC secrets bridge.
#
# The mesh join/enrollment token is age-sealed in the mesh secret store
# (mcnf-secret.sh / mde-seal, SEC-003 role/scope-sealed) — NOT in tracked config
# or tfvars. This external data source resolves it at apply time by shelling out
# to the store helper (`<helper> get <name>`), so a fresh apply always reads the
# current sealed value with the node's OWN age key. There is NO Ansible Vault +
# no second secret system (WL-ARCH-001 decided-stack #8) — the SAME store bridges
# to Ansible via the lookup plugin (automation/ansible/plugins/lookup/mde_seal.py).
#
# STATE CAVEAT: an `external` data source's result is recorded in tofu state, so
# the resolved token lands in /tofu/state/cloud (the etcd state plane, backend.tf)
# — treat that state as sensitive (it already is: the state plane is DR-sealed).
# The value is marked `sensitive` everywhere it flows so it never prints in a plan
# / apply log. Data sources do NOT run at `tofu validate`, so validate/fmt in CI
# needs no live store.
data "external" "join_token" {
  program = ["/usr/bin/env", "bash", "${path.module}/scripts/mde-seal-external.sh"]

  query = {
    helper = var.mde_seal_helper
    name   = var.join_token_secret
  }
}

locals {
  # The unsealed join token, marked sensitive so it never surfaces in a log.
  join_token = sensitive(data.external.join_token.result["value"])
}
