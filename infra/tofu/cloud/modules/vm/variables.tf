variable "name" {
  description = "The VM (libvirt domain) name — also its cloud-init hostname."
  type        = string
}

variable "vcpu" {
  description = "Virtual CPU count."
  type        = number
}

variable "memory_mb" {
  description = "Memory in MiB."
  type        = number
}

variable "disk_gb" {
  description = "Root disk size in GiB (the CoW clone is grown to this)."
  type        = number
}

variable "pool" {
  description = "The libvirt storage pool for the root + cloud-init volumes."
  type        = string
}

variable "base_volume_id" {
  description = "The shared base-image volume id this VM's root disk clones from."
  type        = string
}

variable "network_id" {
  description = "The libvirt network id the VM's interface attaches to."
  type        = string
}

variable "user_data" {
  description = "The rendered cloud-init user-data (carries the sensitive mesh-join token)."
  type        = string
  sensitive   = true
}
