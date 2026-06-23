# DigitalOcean connection. The API token is a SECRET and never lives in the repo:
# it comes from the DIGITALOCEAN_TOKEN env var (stored 0600 at /root/.mcnf-do-token,
# extracted from the doctl `mackes` context). Source it with `source ./env.sh`.
provider "digitalocean" {
  # token: read from $DIGITALOCEAN_TOKEN — intentionally NOT set here.
}
