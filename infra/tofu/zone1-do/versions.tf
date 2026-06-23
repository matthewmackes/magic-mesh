# DEVOPS-SUBSTRATE / Zone 1 (PRODUCTION) — DigitalOcean as code.
#
# The two zones are SEPARATE (operator, 2026-06-22): Zone 1 = production, served
# by lighthouses on DigitalOcean droplets; Zone 2 = internal testing on the Xen
# farm (../  — the xenorchestra state). They are kept in distinct Tofu states on
# purpose: no single `tofu apply` may ever span a production lighthouse AND a
# throwaway test VM. This module owns ONLY Zone 1 / DigitalOcean.
terraform {
  required_version = ">= 1.6"
  required_providers {
    digitalocean = {
      source  = "digitalocean/digitalocean"
      version = ">= 2.40"
    }
  }
}
