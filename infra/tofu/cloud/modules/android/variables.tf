variable "name" {
  description = "The Cuttlefish L1 VM (libvirt domain) name — also its cloud-init hostname."
  type        = string

  validation {
    condition     = can(regex("^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$", var.name))
    error_message = "name must be a bounded workload identity using ASCII letters, digits, ., _, :, or -."
  }
}

variable "vcpu" {
  description = "Requested virtual CPUs (raised to the nested-virt floor of 4)."
  type        = number

  validation {
    condition     = var.vcpu == floor(var.vcpu) && var.vcpu >= 1 && var.vcpu <= 64
    error_message = "vcpu must be an integer between 1 and 64; the module raises it to the Cuttlefish floor of 4."
  }
}

variable "memory_mb" {
  description = "Requested memory in MiB (raised to the nested-virt floor of 8192)."
  type        = number

  validation {
    condition     = var.memory_mb == floor(var.memory_mb) && var.memory_mb >= 256 && var.memory_mb <= 262144
    error_message = "memory_mb must be an integer between 256 and 262144 MiB; the module raises it to the Cuttlefish floor of 8192 MiB."
  }
}

variable "disk_gb" {
  description = "Requested root disk in GiB (raised to the nested-virt floor of 80)."
  type        = number

  validation {
    condition     = var.disk_gb == floor(var.disk_gb) && var.disk_gb >= 1 && var.disk_gb <= 4096
    error_message = "disk_gb must be an integer between 1 and 4096 GiB; the module raises it to the Cuttlefish floor of 80 GiB."
  }
}

variable "pool" {
  description = "The libvirt storage pool for the root + cloud-init volumes."
  type        = string

  validation {
    condition     = contains(["mde-vms", "default"], var.pool)
    error_message = "pool must be the managed mde-vms pool or the explicit libvirt default pool."
  }
}

variable "base_volume_id" {
  description = "The Debian Cuttlefish base-image volume id this L1 VM's root disk clones from."
  type        = string

  validation {
    condition     = length(var.base_volume_id) > 0 && length(var.base_volume_id) <= 512 && !can(regex("[[:cntrl:]]", var.base_volume_id))
    error_message = "base_volume_id must be a non-empty bounded libvirt volume id without control characters."
  }
}

variable "network_id" {
  description = "The libvirt network id the L1 VM's interface attaches to."
  type        = string

  validation {
    condition     = length(var.network_id) > 0 && length(var.network_id) <= 512 && !can(regex("[[:cntrl:]]", var.network_id))
    error_message = "network_id must be a non-empty bounded libvirt network id without control characters."
  }
}

variable "user_data" {
  description = "The rendered mesh-join cloud-init user-data (carries the sensitive join token)."
  type        = string
  sensitive   = true

  validation {
    condition     = length(var.user_data) > 0 && length(var.user_data) <= 1024 * 1024 && !can(regex("[\\x00-\\x08\\x0B\\x0C\\x0E-\\x1F]", var.user_data))
    error_message = "user_data must be a bounded non-empty cloud-init document without forbidden control characters."
  }
}

variable "network_isolation" {
  description = "Whether this workload requested its own isolated network segment (reserved — the backbone attaches to the shared managed network; noted in the console output)."
  type        = bool
  default     = false
}
