# DHCP static reservations on the EdgeRouter (172.20.0.1), source of truth.
# Imported 2026-06-24 from the live `show configuration commands` output.
# tofu converges the router to EXACTLY this set — to add a reservation, add an
# entry and `tofu apply`; to remove one, delete its entry and `tofu apply`.
static_mappings = {
  "DELL-LAPTOP-FEDORA" = { mac = "dc:a9:71:fe:58:71", ip = "172.20.145.25" }
  "fileserver-2"       = { mac = "18:03:73:c4:4f:c2", ip = "172.20.145.122" }
  "HP-PRINTER"         = { mac = "00:1a:4b:2d:8e:23", ip = "172.20.150.153" }
  "KVM3"               = { mac = "6c:4b:90:04:7f:e9", ip = "172.20.145.194" }
  "mcnf-a"             = { mac = "f2:f2:0b:c5:dc:00", ip = "172.20.121.10" }
  "mcnf-b"             = { mac = "52:82:79:ca:43:d8", ip = "172.20.121.11" }
  "MyQ-6BE"            = { mac = "cc:6a:10:00:c4:a2", ip = "172.20.145.10" }
  "rocky9-kvm1"        = { mac = "00:23:24:c2:0f:1c", ip = "172.20.145.193" }
  "rocky9-kvm2"        = { mac = "6c:4b:90:04:7c:a8", ip = "172.20.145.192" }
  "XBOXONE"            = { mac = "2c:54:91:0d:fc:30", ip = "172.20.145.33" }
}
