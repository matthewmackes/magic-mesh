variable "xo_url" {
  description = "Xen Orchestra websocket URL."
  type        = string
  default     = "ws://172.20.145.192:8080"
}

variable "xo_insecure" {
  description = "Skip TLS verification (XO CE here is plain ws on the LAN)."
  type        = bool
  default     = true
}

variable "golden_template_name" {
  description = <<-EOT
    The XCP-2 golden template (e.g. "MDE-VM-golden") the build VMs clone from.
    Empty by default: until the golden template exists, the build-VM resources
    are INERT (count 0) so `tofu plan` proves XO connectivity + validates config
    without trying to clone a template that isn't there yet. Set this once XCP-2
    lands to enable `tofu apply`.
  EOT
  type        = string
  default     = ""
}

variable "build_vcpus" {
  description = "vCPUs per build VM (each XCP host has 4 physical cores)."
  type        = number
  default     = 4
}

variable "build_memory_gib" {
  description = "RAM per build VM, GiB."
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
