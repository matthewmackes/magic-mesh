# The farm spans THREE standalone XCP-ng pools; the xenserver provider is
# single-pool, so one aliased provider per dom0 XAPI endpoint. Same root password
# (TF_VAR_xapi_password from /root/.mcnf-xapi-cred, 0600, off-repo) on all three.
provider "xenserver" {
  alias    = "xhs" # XEN-HOME-SERVICES
  host     = "https://172.20.0.9"
  username = var.xapi_username
  password = var.xapi_password
}
provider "xenserver" {
  alias    = "kvm" # KVM-XCP1
  host     = "https://172.20.145.193"
  username = var.xapi_username
  password = var.xapi_password
}
provider "xenserver" {
  alias    = "big" # XEN-BIGBOY
  host     = "https://172.20.145.165"
  username = var.xapi_username
  password = var.xapi_password
}
