# XAPI connection. host/username are non-secret; the password is a SECRET supplied
# via TF_VAR_xapi_password (env.sh reads it from /root/.mcnf-xapi-cred, 0600,
# off-repo) — never in the repo. This points at ONE dom0's XAPI directly (no XO).
provider "xenserver" {
  host     = var.xapi_host
  username = var.xapi_username
  password = var.xapi_password
}
