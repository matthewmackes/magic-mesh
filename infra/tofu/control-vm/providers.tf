# DAR-12 — ONE aliased `xenserver` provider aimed at the FOUNDING dom0 (lock 5).
# This mirrors the xen-xapi aliased-provider pattern (one provider per dom0 XAPI
# endpoint); the control VM is created on the founding dom0 ONLY, so this root
# carries a single alias rather than the farm's three. The endpoint is a per-mesh
# variable (NO hardcoded LAN IP) so the same root founds the backoffice on any new
# Nebula's founding XCP-NG machine.
#
# SECRET HANDLING (lock 8): the XAPI password is NEVER a literal here. It arrives
# as the `sensitive` var.xapi_password, sourced at apply time from the mesh secret
# store via `env.sh` (`TF_VAR_xapi_password=$(mcnf-secret.sh get xapi-password)`),
# exactly as xen-xapi does — no host-local plaintext, no value in the committed HCL.
provider "xenserver" {
  alias    = "founder"
  host     = var.founder_xapi_host
  username = var.xapi_username
  password = var.xapi_password
}
