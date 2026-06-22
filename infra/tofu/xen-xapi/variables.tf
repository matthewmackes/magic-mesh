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
