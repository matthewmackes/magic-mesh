variable "name" {
  description = "The libvirt network name."
  type        = string
}

variable "mode" {
  description = "Forwarding mode: nat|bridge|none|route."
  type        = string
}

variable "bridge" {
  description = "Host bridge device for bridge mode (e.g. the Nebula-adjacent br-mesh)."
  type        = string
}

variable "cidr" {
  description = "Managed subnet (CIDR) for nat/route mode."
  type        = string
}

variable "autostart" {
  description = "Whether the network auto-starts with libvirtd."
  type        = bool
}
