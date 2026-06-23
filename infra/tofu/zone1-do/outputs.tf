output "lighthouses" {
  description = "Production lighthouse droplets (public/private IPv4)."
  value = {
    "lighthouse-01" = {
      id      = digitalocean_droplet.lighthouse_01.id
      public  = digitalocean_droplet.lighthouse_01.ipv4_address
      private = digitalocean_droplet.lighthouse_01.ipv4_address_private
    }
  }
}

output "asterisk_ip" {
  description = "VoIP/Asterisk droplet public IPv4 (voip.matthewmackes.com)."
  value       = digitalocean_droplet.asterisk.ipv4_address
}
