variable "xapi_username" {
  description = "XAPI user (PAM)."
  type        = string
  default     = "root"
}
variable "xapi_password" {
  description = "XAPI password — from TF_VAR_xapi_password (off-repo /root/.mcnf-xapi-cred)."
  type        = string
  sensitive   = true
}

# --- FARM-AUTOSCALE shape model (DAR-31; mirrors ../variables.tf) ------------
# Ported into the XAPI-native root so the no-XO farm scales the same way. Keys are
# the dom0 keys in build-vms.tf's local.dom0 (xen-home-services | kvm-xcp1 |
# xen-bigboy); an unlisted dom0 defaults to `off`.

variable "golden_template_name" {
  description = <<-EOT
    The XCP-ng golden template the build VMs clone from. Built (UEFI) on each pool
    by install-helpers/setup-xcp-golden-template.sh; the toolchain is BAKED IN
    (DAR-34) so there is no separate `-tc` artifact — the name is canonical
    `MDE-VM-golden`. Set to "" to make every build-VM resource inert (the adopted
    baseline + the shape model both blank) — a connectivity-only plan.
  EOT
  type        = string
  default     = "MDE-VM-golden"
}

variable "shape" {
  description = <<-EOT
    Per-dom0 build-VM shape — the autoscaler writes this from live build demand
    (install-helpers/farm-autoscale.sh → *.auto.tfvars). MUTUALLY EXCLUSIVE per
    dom0 (L4): a dom0 runs EITHER one whole-host `big` VM, OR `small_count`
    standard `small` VMs, OR nothing (`off` = scale-to-zero). A shape entry for an
    adopted dom0's small-0 key resizes that live VM; absent a shape, the adopted
    baseline keeps the live VM present (the 0-destroy floor — see build-vms.tf).
  EOT
  type        = map(string)
  default     = {} # default: no shape overlay — only the adopted baseline VMs exist
  validation {
    condition     = alltrue([for s in values(var.shape) : contains(["big", "small", "off"], s)])
    error_message = "Each shape must be one of: big, small, off."
  }
  validation {
    condition     = alltrue([for k in keys(var.shape) : contains(["xen-home-services", "kvm-xcp1", "xen-bigboy"], k)])
    error_message = "shape keys must be known dom0s: xen-home-services, kvm-xcp1, xen-bigboy."
  }
}

variable "small_count" {
  description = <<-EOT
    Per-dom0 number of `small` build VMs when shape=small (ignored for big/off).
    Keyed by dom0 key; an unlisted dom0 defaults to 1. Bounded by the dom0's SR /
    RAM headroom (capped at 4 — a 40-wide IP lane).
  EOT
  type        = map(number)
  default     = {}
  validation {
    condition     = alltrue([for n in values(var.small_count) : n >= 1 && n <= 4])
    error_message = "small_count per dom0 must be between 1 and 4 (SR/RAM headroom)."
  }
  validation {
    condition     = alltrue([for k in keys(var.small_count) : contains(["xen-home-services", "kvm-xcp1", "xen-bigboy"], k)])
    error_message = "small_count keys must be known dom0s: xen-home-services, kvm-xcp1, xen-bigboy."
  }
}

variable "vm_overrides" {
  description = <<-EOT
    Optional per-VM overrides keyed by the build-VM key (`<dom0>` for a big VM,
    `<dom0>-<n>` for the nth small). A map of any of {name, ip_cidr, vcpus,
    mem_gib} — a one-off node bigger/smaller than its shape's default. dom0_key is
    re-applied after the merge so an override cannot mis-place a VM.
  EOT
  type        = map(map(any))
  default     = {}
}

variable "build_vcpus" {
  description = "vCPUs per standard `small` build VM."
  type        = number
  default     = 4
}

variable "build_memory_gib" {
  description = "RAM per standard `small` build VM, GiB (≤16; big VMs sized in build-vms.tf)."
  type        = number
  default     = 16
}
