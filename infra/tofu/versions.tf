# DEVOPS-SUBSTRATE / Farm Automation Manager — provider + version pins.
# The durable replacement for the install-helpers/*xcp* bash provisioners:
# declares the build farm as code against live Xen Orchestra (XO drives XAPI,
# so there's no `xe`-over-ssh quoting class of bugs).
terraform {
  required_version = ">= 1.6"
  required_providers {
    xenorchestra = {
      source  = "vatesfr/xenorchestra"
      version = ">= 0.31.0"
    }
  }
}
