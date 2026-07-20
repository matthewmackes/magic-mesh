variable "name" {
  description = "The Cuttlefish L1 VM (libvirt domain) name — also its cloud-init hostname."
  type        = string
}

variable "vcpu" {
  description = "Requested virtual CPUs (raised to the nested-virt floor of 4)."
  type        = number
}

variable "memory_mb" {
  description = "Requested memory in MiB (raised to the nested-virt floor of 8192)."
  type        = number
}

variable "disk_gb" {
  description = "Requested root disk in GiB (raised to the nested-virt floor of 80)."
  type        = number
}

variable "pool" {
  description = "The libvirt storage pool for the root + cloud-init volumes."
  type        = string
}

variable "base_volume_id" {
  description = "The Debian Cuttlefish base-image volume id this L1 VM's root disk clones from."
  type        = string
}

variable "network_id" {
  description = "The libvirt network id the L1 VM's interface attaches to."
  type        = string
}

variable "user_data" {
  description = "The rendered mesh-join cloud-init user-data (carries the sensitive join token)."
  type        = string
  sensitive   = true
}

variable "network_isolation" {
  description = "Whether this workload requested its own isolated network segment (reserved — the backbone attaches to the shared managed network; noted in the console output)."
  type        = bool
  default     = false
}
