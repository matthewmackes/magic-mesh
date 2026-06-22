variable "region" {
  description = "DigitalOcean region the production fleet lives in."
  type        = string
  default     = "nyc3"
}

variable "lighthouse_size" {
  description = "Droplet size slug for a lighthouse (2GB/2vCPU = s-2vcpu-2gb)."
  type        = string
  default     = "s-2vcpu-2gb"
}

variable "lighthouse_image" {
  description = "Base image slug for a freshly-cut lighthouse."
  type        = string
  default     = "fedora-43-x64"
}

variable "domain" {
  description = "DNS zone hosting the lighthouse-NN records."
  type        = string
  default     = "matthewmackes.com"
}
