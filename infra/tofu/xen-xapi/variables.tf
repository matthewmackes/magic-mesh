variable "xapi_host" {
  description = "Pool-master XAPI URL (https://<dom0>). Default = the KVM-XCP1 test bed."
  type        = string
  default     = "https://172.20.145.193"
}
variable "xapi_username" {
  description = "XAPI user (PAM)."
  type        = string
  default     = "root"
}
variable "xapi_password" {
  description = "XAPI password — from TF_VAR_xapi_password (off-repo /root/.mcnf-xapi-cred)."
  type        = string
  sensitive   = true
}
