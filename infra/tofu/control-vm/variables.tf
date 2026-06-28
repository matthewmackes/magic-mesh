# DAR-12 / DAR-13 — inputs for the one control VM. Every per-mesh fact (dom0
# endpoint, network, IPs, mesh identity, anchors) is a variable so the SAME root
# founds the backoffice on any new Nebula; NO LAN literal (`172.20.145.192`),
# project, or pool UUID is baked in. tfvars are GENERATED from mesh identity
# (gen-tfvars.sh, DAR-33), never hand-edited.

# ---------------------------------------------------------------------------
# Provider / dom0 (lock 5)
# ---------------------------------------------------------------------------
variable "founder_xapi_host" {
  description = "XAPI URL of the FOUNDING dom0 the control VM is created on (e.g. https://<founder-dom0-lan-ip>)."
  type        = string
  validation {
    condition     = can(regex("^https://", var.founder_xapi_host))
    error_message = "founder_xapi_host must be an https:// XAPI endpoint."
  }
}

variable "xapi_username" {
  description = "XAPI user (PAM)."
  type        = string
  default     = "root"
}

variable "xapi_password" {
  description = "XAPI password — from TF_VAR_xapi_password (env.sh sources it from the mesh secret store: mcnf-secret.sh get xapi-password). NEVER a literal."
  type        = string
  sensitive   = true
}

# ---------------------------------------------------------------------------
# Tier (lock 4) — one artifact, two sizes. Drives both the VM shape AND which
# systemd units `mackesd converge` enables (see cloud-init/control-vm.yaml.tftpl).
# ---------------------------------------------------------------------------
variable "backoffice_tier" {
  description = "minimal (state-backend + secrets + tofu roots) or full (+ CI + reconciler + build farm + DR)."
  type        = string
  default     = "minimal"
  validation {
    condition     = contains(["minimal", "full"], var.backoffice_tier)
    error_message = "backoffice_tier must be \"minimal\" or \"full\"."
  }
}

# ---------------------------------------------------------------------------
# Golden template (lock 5 / §2.8 GAP 5) — CANONICAL NAME = MDE-VM-golden (the name
# the template-builder produces and the live build VMs clone; the `-tc` variant is
# retired). The control VM clones this same baked-toolchain template.
# ---------------------------------------------------------------------------
variable "golden_template_name" {
  description = "Canonical golden template to clone (§2.8: MDE-VM-golden, not the retired -tc)."
  type        = string
  default     = "MDE-VM-golden"
}

variable "control_vm_name" {
  description = "name_label of the control VM."
  type        = string
  default     = "mcnf-control"
}

# ---------------------------------------------------------------------------
# Network (LAN seed; the durable identity is the OVERLAY IP minted at join).
# The NM static-IP keyfile fix is reused verbatim from build-vm.yaml.tftpl.
# ---------------------------------------------------------------------------
variable "network_uuid" {
  description = "Founding dom0 pool-network UUID the control VM's NIC attaches to."
  type        = string
}

variable "control_ip_cidr" {
  description = "Static LAN IP/CIDR for the control VM's first NIC (e.g. 172.20.0.40/16)."
  type        = string
}

variable "gateway" {
  description = "LAN default gateway for the control VM."
  type        = string
}

variable "dns" {
  description = "DNS servers for the control VM (joined into the NM keyfile)."
  type        = list(string)
  default     = ["1.1.1.1", "9.9.9.9"]
}

variable "ssh_pubkey_path" {
  description = "Public key authorized on the control VM's mm user."
  type        = string
  default     = "/root/.ssh/mackes_mesh_ed25519.pub"
}

# ---------------------------------------------------------------------------
# Mesh enrollment (lock 6) — headless join as a FULL mesh peer (--role server).
# ---------------------------------------------------------------------------
variable "join_token" {
  description = "Enroll token for `mackesd join <token> --role server`. SENSITIVE — sourced at apply from the mesh secret store (TF_VAR_join_token=$(mcnf-secret.sh get join-token)); NEVER a literal in HCL/tfvars."
  type        = string
  sensitive   = true
}

variable "mesh_id" {
  description = "Mesh id this control VM belongs to (written into /etc/mackesd/site.yml)."
  type        = string
}

variable "lighthouse_overlay_ips" {
  description = "Lighthouse OVERLAY IP roster (site.yml mde_lighthouses; peers point at all of them). Per-mesh, NOT baked."
  type        = list(string)
}

variable "etcd_anchor_overlay_ips" {
  description = "Founder etcd quorum OVERLAY IPs for `setup-etcd.sh --client-only --anchors <csv>` (lock 2/7). NOT the dead .192:2379 default — the live quorum is the lighthouses."
  type        = list(string)
}

variable "qnm_path" {
  description = "Mesh-storage mount path written into site.yml."
  type        = string
  default     = "/mnt/mesh-storage"
}

# ---------------------------------------------------------------------------
# Derived sizing — minimal 4 vCPU / 8 GiB / 60 GiB; full 8 / 16 / 120 (design §2.1).
# A single tier gate; no per-field overrides (the control VM is a fixed shape per
# tier, unlike the elastic build farm).
# ---------------------------------------------------------------------------
locals {
  gib = 1024 * 1024 * 1024

  tier_shape = {
    minimal = { vcpus = 4, mem_gib = 8, disk_gib = 60 }
    full    = { vcpus = 8, mem_gib = 16, disk_gib = 120 }
  }

  shape = local.tier_shape[var.backoffice_tier]
}
