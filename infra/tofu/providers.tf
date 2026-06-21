# DS-1 — OpenTofu provider config for the MCNF test fleet.
# Drives Xen Orchestra (which fronts both XCP-ng pools). See docs/ops/environment-rebuild.md.
terraform {
  required_version = ">= 1.6"
  required_providers {
    xenorchestra = {
      source  = "vatesfr/xenorchestra"
      version = "~> 0.30"
    }
  }
  # Bootstrap backend: local state on the control host (gitignored).
  # Migrates to a mesh-native backend once etcd is up (DS-8).
  backend "local" {
    path = "terraform.tfstate"
  }
}

# url + token are supplied via TF_VAR_xoa_url / TF_VAR_xoa_token (env, sourced from the
# DS-8 secret store) — never written into config. token is marked sensitive.
provider "xenorchestra" {
  url      = var.xoa_url
  token    = var.xoa_token
  insecure = true # XAPI/XO present self-signed certs in this airgapped dev env
}
