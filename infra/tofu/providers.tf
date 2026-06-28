# XO connection. The token is a SECRET and never lives in the repo — it comes
# from the XOA_TOKEN env var, unsealed from the MESH SECRET STORE (DAR-5:
# `mcnf-secret.sh get xo-token`; mint a fresh one with
# `install-helpers/xo-mint-token.sh --to-store`). `url` + `insecure` are non-secret.
# Source them with: `source ./env.sh` (see env.sh.example).
provider "xenorchestra" {
  url      = var.xo_url      # ws://<control-host>:8080
  insecure = var.xo_insecure # XO CE over plain ws / self-signed
  # token: read from $XOA_TOKEN — intentionally NOT set here.
}
