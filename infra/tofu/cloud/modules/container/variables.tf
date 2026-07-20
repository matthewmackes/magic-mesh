variable "name" {
  description = "The container workload name — the Quadlet unit + systemd service name."
  type        = string
}

variable "image" {
  description = "The container image ref (registry/name:tag) the Quadlet unit pulls + runs."
  type        = string

  validation {
    condition     = trimspace(var.image) != ""
    error_message = "a service_container workload requires a non-empty `image`."
  }
}

variable "vcpu" {
  description = "CPU quota (whole cores) the rootless container is capped at."
  type        = number
}

variable "memory_mb" {
  description = "Memory cap in MiB the rootless container is limited to."
  type        = number
}

variable "network_isolation" {
  description = "Whether the container gets its own mesh network segment (else it joins the shared managed mesh network)."
  type        = bool
  default     = false
}

variable "network_name" {
  description = "The shared managed mesh network name the container joins when not isolated."
  type        = string
}
