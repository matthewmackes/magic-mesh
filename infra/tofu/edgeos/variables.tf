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
  description = "Path to a 0600 file containing ONLY the EdgeOS password. The password is NEVER inlined in tofu config or argv — the scripts read it from this file via sshpass -f."
  type        = string
  default     = "/root/.mcnf-ubnt-cred"
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
