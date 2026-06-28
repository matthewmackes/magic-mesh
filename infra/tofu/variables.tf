# DAR / DEVOPS-AUTOMATION-REBUILD §2.8: this whole top-level root is the DEPRECATED
# Xen Orchestra path. XO is dead on the live fleet (ws connection-refused) and the
# farm is XAPI-managed via infra/tofu/xen-xapi/ — do NOT apply this root. The dead
# LAN websocket default is removed so no stale literal lingers in a tracked .tf; if
# anyone resurrects an XO, they must supply xo_url explicitly.
variable "xo_url" {
  description = "DEPRECATED (XO path retired — use infra/tofu/xen-xapi/). Xen Orchestra websocket URL; no default — must be supplied if the dead XO root is ever revived."
  type        = string
}

variable "xo_insecure" {
  description = "DEPRECATED (XO path retired). Skip TLS verification (XO CE was plain ws on the LAN)."
  type        = bool
  default     = true
}

variable "golden_template_name" {
  description = <<-EOT
    The XCP-2 golden template the build VMs clone from. Built on both pools
    (UEFI) by install-helpers/setup-xcp-golden-template.sh. Set to "" to make the
    build-VM resources inert (count 0) — useful for a connectivity-only plan.
  EOT
  # DAR-34: the toolchain (rustc/cargo/generate-rpm/mold) is BAKED INTO the ONE
  # canonical template `MDE-VM-golden` — no separate `-tc` artifact, no name drift.
  # An elastic clone is build-ready at first boot with no ~15-min toolchain step
  # (what makes scale-from-zero practical). The bake is conveyed by the template
  # CONTENT, not a name suffix; `provision_build_ready` collapses to the baseline
  # snapshot. (Historically this default carried a `-tc` toolchain-bake suffix; the
  # bake is now in the canonical template, so the suffix is retired.)
  type    = string
  default = "MDE-VM-golden"
}

# --- FARM-AUTOSCALE shape model (docs/design/farm-autoscale.md, FA-1) ---

variable "shape" {
  description = <<-EOT
    Per-dom0 build-VM shape — the autoscaler writes this from live build demand
    (install-helpers/farm-autoscale.sh → *.auto.tfvars). MUTUALLY EXCLUSIVE per
    dom0 (L4): a dom0 runs EITHER one whole-host `big` VM, OR `small_count`
    standard `small` VMs, OR nothing (`off` = scale-to-zero). Keys are the dom0
    keys in main.tf's `local.dom0` (xen-home-services | kvm-xcp1 | xen-bigboy);
    an unlisted dom0 defaults to `off`.
  EOT
  type        = map(string)
  default     = {} # all dom0s `off` until the autoscaler (or operator) sets a shape
  validation {
    condition     = alltrue([for s in values(var.shape) : contains(["big", "small", "off"], s)])
    error_message = "Each shape must be one of: big, small, off."
  }
  validation {
    # Reject a typo'd dom0 key — it would silently match nothing (no VM created)
    # and the operator/autoscaler would believe a VM exists. Keys MUST be known
    # dom0s (kept in sync with local.dom0 in main.tf — only 3 hosts, cold facts).
    condition     = alltrue([for k in keys(var.shape) : contains(["xen-home-services", "kvm-xcp1", "xen-bigboy"], k)])
    error_message = "shape keys must be known dom0s: xen-home-services, kvm-xcp1, xen-bigboy."
  }
}

variable "small_count" {
  description = <<-EOT
    Per-dom0 number of `small` build VMs when shape=small (ignored for big/off).
    Keyed by dom0 key; an unlisted dom0 defaults to 1. Bounded by the dom0's SR /
    RAM headroom (the design caps concurrent VMs per dom0).
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
    mem_gib} — a one-off node bigger/smaller than its shape's default. Empty by
    default; the shape model sizes every VM on its own.
  EOT
  type        = map(map(any))
  default     = {}
}

variable "build_vcpus" {
  description = "vCPUs per standard `small` build VM (each XCP host has 4 physical cores)."
  type        = number
  default     = 4
}

variable "build_memory_gib" {
  description = "RAM per standard `small` build VM, GiB (≤16; big VMs are sized in main.tf)."
  type        = number
  default     = 16
}

variable "build_disk_gib" {
  description = "Root disk per build VM, GiB (cloud-init growpart expands the rootfs)."
  type        = number
  default     = 80
}

variable "gateway" {
  description = "LAN gateway the build VMs route through."
  type        = string
  default     = "172.20.0.1"
}

variable "dns" {
  description = "Resolver list for the build VMs."
  type        = list(string)
  default     = ["8.8.8.8", "1.1.1.1"]
}
