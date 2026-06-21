# DS-1 — variables. The per-feature gate is 3 LH + 3 peers (§10 V4); the release gate
# overrides peer_count to 9 (3 LH + 9 peers). One -var flips the topology.

variable "xoa_url" {
  description = "Xen Orchestra websocket URL (ws://control-host:8080). Set via TF_VAR_xoa_url."
  type        = string
}

variable "xoa_token" {
  description = "XO API token. Set via TF_VAR_xoa_token from the DS-8 secret store; never in-repo."
  type        = string
  sensitive   = true
}

variable "pool_name" {
  description = "Xen Orchestra pool to place the test fleet on (Host B / KVM-XCP1 is the test bed)."
  type        = string
  default     = "KVM-XCP1"
}

variable "template_name" {
  description = "Golden-image template to clone (built by install-helpers/build-mde-vm-golden.sh, DS-5)."
  type        = string
  default     = "mcnf-golden"
}

variable "sr_name" {
  description = "Storage repository for the VM disks (XCP-ng default local SR)."
  type        = string
  default     = "Local storage"
}

variable "disk_gb" {
  description = "Root disk size per test VM in GiB."
  type        = number
  default     = 20
}

variable "network_name" {
  description = "XO network the VMs attach to (the management/underlay network)."
  type        = string
  default     = "Pool-wide network associated with eth0"
}

variable "lighthouse_count" {
  description = "Number of lighthouse nodes (§8 envelope: up to 3)."
  type        = number
  default     = 3
}

variable "peer_count" {
  description = "Number of headless/full peers. Per-feature gate = 3; release gate = 9."
  type        = number
  default     = 3

  validation {
    condition     = var.peer_count >= 0 && var.peer_count <= 9
    error_message = "peer_count must be within the §8 envelope (0..9)."
  }
}

variable "vcpus" {
  description = "vCPUs per test VM (hosts have 4 physical cores — keep small, VMs share cores)."
  type        = number
  default     = 1
}

variable "memory_gb" {
  description = "RAM per test VM in GiB."
  type        = number
  default     = 2
}

variable "ssh_authorized_key" {
  description = "Control-host public key seeded into each VM via cloud-init."
  type        = string
}
