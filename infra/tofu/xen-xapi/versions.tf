# DATACENTER-1 (prototype) — XAPI-native Tofu provider, validating the no-XO path.
# Isolated from infra/tofu/ (the live xenorchestra-managed farm): its own dir + state.
# The Citrix/CSG `xenserver` provider speaks XAPI directly to a pool master; XCP-ng
# is XAPI-compatible, so this is the "XAPI-direct, drop XO" foundation (Q74/Q77).
terraform {
  required_version = ">= 1.6"
  required_providers {
    xenserver = {
      source  = "xenserver/xenserver"
      version = ">= 0.2.0"
    }
  }
}
