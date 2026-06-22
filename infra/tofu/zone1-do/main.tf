# =============================================================================
# Zone 1 (production) DigitalOcean fleet — imported live state, managed as code.
# Inventory snapshot 2026-06-22 (doctl `mackes` context, acct matthewmackes@gmail.com,
# droplet limit 25, region nyc3):
#   droplet 579112110 lighthouse-01  174.138.68.216 / 10.132.0.3  2GB/2vCPU/60GB  Fedora43  tag=magic-lighthouse
#   droplet 440041476 ASTERISK-1     64.225.27.199  / 172.18.0.2  1GB/1vCPU/25GB  Ubuntu    tag=ASTERISK (voip.)
#   domain  matthewmackes.com  (A: lighthouse-01/-02/-03, voip, @)
#   sshkeys mackes-mesh-claude(57249387) Unit-(57026644) Eagle(57026397)
# NOTE: DNS advertises lighthouse-02 (45.55.33.179) + lighthouse-03 (138.197.32.202)
#       but NO droplets exist for them — stale records (the old fleet). Kept in
#       state so the drift is visible; recreate the droplets or prune the records
#       deliberately, never by accident.
# =============================================================================

# ---- SSH keys registered in the DO account -------------------------------------
resource "digitalocean_ssh_key" "mackes_mesh_claude" {
  name       = "mackes-mesh-claude"
  public_key = "ssh-ed25519 AAAA-imported-placeholder mackes-mesh-claude"
  lifecycle { ignore_changes = [public_key] } # imported; real key lives in DO
}
resource "digitalocean_ssh_key" "unit" {
  name       = "Unit-"
  public_key = "ssh-ed25519 AAAA-imported-placeholder Unit-"
  lifecycle { ignore_changes = [public_key] }
}
resource "digitalocean_ssh_key" "eagle" {
  name       = "Eagle"
  public_key = "ssh-ed25519 AAAA-imported-placeholder Eagle"
  lifecycle { ignore_changes = [public_key] }
}

# ---- DNS zone + records --------------------------------------------------------
resource "digitalocean_domain" "primary" {
  name = var.domain
}

resource "digitalocean_record" "lighthouse_01" {
  domain = digitalocean_domain.primary.id
  type   = "A"
  name   = "lighthouse-01"
  value  = "174.138.68.216"
  ttl    = 3600
}
resource "digitalocean_record" "lighthouse_02" {
  domain = digitalocean_domain.primary.id
  type   = "A"
  name   = "lighthouse-02"
  value  = "45.55.33.179" # STALE — no droplet exists for this record
  ttl    = 3600
}
resource "digitalocean_record" "lighthouse_03" {
  domain = digitalocean_domain.primary.id
  type   = "A"
  name   = "lighthouse-03"
  value  = "138.197.32.202" # STALE — no droplet exists for this record
  ttl    = 60
}
resource "digitalocean_record" "voip" {
  domain = digitalocean_domain.primary.id
  type   = "A"
  name   = "voip"
  value  = "64.225.27.199"
  ttl    = 3600
}
resource "digitalocean_record" "apex" {
  domain = digitalocean_domain.primary.id
  type   = "A"
  name   = "@"
  value  = "185.199.109.153" # GitHub Pages
  ttl    = 3600
}

# ---- Droplets (LIVE — fully managed) -------------------------------------------
# OpenTofu has full authority over these (operator, 2026-06-22). ignore_changes is
# kept only to stop a ROUTINE plan from churning on force-new/computed attributes
# (image is a custom image on ASTERISK; user_data/ssh_keys are set at create) — it
# prevents accidental rebuilds, not deliberate ones. To rebuild a lighthouse,
# change the attribute deliberately (remove it from ignore_changes in that edit).
resource "digitalocean_droplet" "lighthouse_01" {
  name     = "lighthouse-01"
  region   = var.region
  size     = var.lighthouse_size
  image    = var.lighthouse_image
  tags     = ["magic-lighthouse"]
  vpc_uuid = "46dd7574-51c4-4802-93bf-3c1f2049e6b2" # default-nyc3

  lifecycle {
    ignore_changes = [image, user_data, ssh_keys, tags]
  }
}

resource "digitalocean_droplet" "asterisk" {
  name     = "ASTERISK-1-MACKES"
  region   = var.region
  size     = "s-1vcpu-1gb"
  image    = "ubuntu-22-04-x64" # real one is a custom image; ignored below
  tags     = ["ASTERISK"]
  vpc_uuid = "278ca425-b303-4f17-aaa7-a7b16db3093d" # nyc3-vpc-01-MACKES

  lifecycle {
    ignore_changes = [image, user_data, ssh_keys, tags]
  }
}

# ---- Cutting a NEW lighthouse (Zone 1 grow path) -------------------------------
# To add lighthouse-04, uncomment + `tofu apply`. It clones the standard size/image
# and registers the mesh key; then bootstrap mackesd + `mackesd found --role
# lighthouse` and add its A record (lighthouse-04) below.
#
# resource "digitalocean_droplet" "lighthouse_04" {
#   name     = "lighthouse-04"
#   region   = var.region
#   size     = var.lighthouse_size
#   image    = var.lighthouse_image
#   tags     = ["magic-lighthouse"]
#   ssh_keys = [digitalocean_ssh_key.mackes_mesh_claude.id]
# }
