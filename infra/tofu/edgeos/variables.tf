variable "edgeos_host" {
  description = "EdgeRouter management IP."
  type        = string
  default     = "172.20.0.1"
}

variable "edgeos_user" {
  description = "EdgeOS SSH user."
  type        = string
  default     = "ubnt"
}

variable "edgeos_cred_file" {
  description = <<-EOT
    Path to a 0600 file containing ONLY the EdgeOS password. The password is NEVER
    inlined in tofu config or argv — the scripts read it from this file via sshpass -f.
    DAR-5/DAR-10: this is supplied via TF_VAR_edgeos_cred_file by the shared
    automation/lib/tofu-env.sh, which unseals /mcnf/secret/edgeos-cred from the mesh
    store into a TMPFS 0600 file (shredded on exit). There is NO /root/.mcnf-ubnt-cred
    plaintext default — source ./env.sh (see env.sh.example) before tofu.
  EOT
  type        = string

  # No default: the cred MUST come from the store via tofu-env.sh (TF_VAR_…), so a
  # missing source fails loud at plan time rather than silently reading a plaintext
  # dotfile that may not exist on a reconstituted control VM.
  validation {
    condition     = length(var.edgeos_cred_file) > 0
    error_message = "edgeos_cred_file is empty — source ./env.sh (tofu-env.sh unseals /mcnf/secret/edgeos-cred to a tmpfs path)."
  }
}

variable "shared_network" {
  description = "EdgeOS dhcp-server shared-network-name."
  type        = string
  default     = "Home-Production-172_20"
}

variable "subnet" {
  description = "EdgeOS dhcp-server subnet the static-mappings live under."
  type        = string
  default     = "172.20.0.0/16"
}

variable "static_mappings" {
  description = <<-EOT
    Declarative DHCP reservations: name => { mac, ip }. tofu converges the
    router to EXACTLY this set — names present here are created/updated, names
    on the router but absent here are removed. This map is the source of truth,
    so it must list every reservation you intend to keep (the baseline in
    terraform.tfvars was imported from the live router).
  EOT
  type = map(object({
    mac = string
    ip  = string
  }))

  # MAC + IPv4 shape guard — a malformed entry fails plan, not the live router.
  validation {
    condition = alltrue([
      for m in values(var.static_mappings) :
      can(regex("^([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}$", m.mac)) &&
      can(regex("^(\\d{1,3}\\.){3}\\d{1,3}$", m.ip))
    ])
    error_message = "Every static_mappings entry needs a colon-separated MAC and a dotted-quad IPv4."
  }
}

variable "firewall_rulesets" {
  description = <<-EOT
    ROUTER-7 — declarative EdgeOS/VyOS firewall rulesets MCNF manages, keyed by
    ruleset name:
      { "<name>": { "default-action": "drop|accept|reject", "description": "..",
                    "rule": { "<num>": { "<attr>": "<val>", .. } } } }
    ADDITIVE (§6): only the NAMED rulesets are converged (delete+recreate to
    exact, inside a commit-confirm window so a self-lockout auto-reverts);
    rulesets the operator authored elsewhere are left untouched. Empty (the
    default) = manage nothing.
  EOT
  type        = any
  default     = {}
}

variable "nat_rules" {
  description = <<-EOT
    ROUTER-8 — declarative EdgeOS/VyOS destination-NAT (port-forward) rules MCNF
    manages, keyed by rule number:
      { "<num>": { "type": "destination", "inbound-interface": "eth0",
                   "protocol": "tcp", "destination port": "443",
                   "inside-address address": "10.42.0.5",
                   "inside-address port": "4533", "description": ".." } }
    ADDITIVE (§6): only the listed rule NUMBERS are converged; NAT rules the
    operator authored elsewhere are untouched. Empty (default) = manage nothing.
  EOT
  type        = any
  default     = {}
}

variable "vpn_config" {
  description = <<-EOT
    ROUTER-9 — declarative EdgeOS/VyOS VPN endpoint config MCNF manages, keyed by
    the managed config ROOT path → its desired leaves:
      { "interfaces wireguard wg0": { "address": "10.50.0.1/24",
          "private-key": "/config/auth/wg0.key", "port": "51820",
          "peer SITE-B allowed-ips": "10.50.0.2/32",
          "peer SITE-B endpoint": "203.0.113.7:51820" } }
    or { "vpn ipsec site-to-site peer 203.0.113.7": { .. } }.
    ADDITIVE (§6): only the named roots are delete+recreated to exact; VPN config
    the operator authored elsewhere is untouched. Empty (default) = manage nothing.
  EOT
  type        = any
  default     = {}
}
