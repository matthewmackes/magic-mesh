variable "region" {
  description = "DigitalOcean region the production fleet lives in."
  type        = string
  default     = "nyc3"
}

variable "lighthouse_size" {
  description = "The only supported thin lighthouse size (1 shared vCPU, 512 MiB RAM, 10 GiB SSD)."
  type        = string
  default     = "s-1vcpu-512mb-10gb"

  validation {
    condition     = var.lighthouse_size == "s-1vcpu-512mb-10gb"
    error_message = "Lighthouses must use the thin s-1vcpu-512mb-10gb profile; media/fileshare and larger variants are retired."
  }
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
