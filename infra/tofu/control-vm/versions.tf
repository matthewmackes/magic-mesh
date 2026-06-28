# DEVOPS-AUTOMATION-REBUILD / DAR-12 — the control-VM tofu root.
# REUSES the xen-xapi (XAPI-native, no XO) provider proven in DATACENTER-1: the
# `xenserver` provider speaks XAPI directly to a pool master, so there is no Xen
# Orchestra to lose. Pinned to the same 0.2.x line the build-farm root validated
# against (the create-with-cloud_config seed path is the 0.2.x risk CONTROLVM-9
# live-verifies; the HCL here is provider-version-compatible with xen-xapi).
terraform {
  required_version = ">= 1.6"
  required_providers {
    xenserver = {
      source  = "xenserver/xenserver"
      version = ">= 0.2.0"
    }
  }
}
