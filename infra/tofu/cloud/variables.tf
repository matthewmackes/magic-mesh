variable "libvirt_uri" {
  description = <<-EOT
    The libvirt connection URI. Local-first (E12) default is the local system
    hypervisor. Drive a remote dom0/seat over the mesh with
    qemu+ssh://root@<overlay-ip>/system.
  EOT
  type        = string
  default     = "qemu:///system"
}

variable "pool" {
  description = "The libvirt storage pool VM volumes + cloud-init disks land in."
  type        = string
  default     = "default"
}

variable "base_image_source" {
  description = <<-EOT
    Source of the base OS image the VMs clone from — a path or URL to a bootc/
    qcow2 image (extends packaging/bootc/; WL-ARCH-001 item 4). Every VM's root
    volume is a copy-on-write clone of this base.
  EOT
  type        = string
  default     = "/var/lib/libvirt/images/mde-base.qcow2"
}

variable "network" {
  description = <<-EOT
    The libvirt network the VMs attach to, on the Nebula-adjacent bridge (replaces
    Neutron). `mode` is nat|bridge|none|route; `bridge` names the host bridge for
    bridge mode (e.g. the Nebula-adjacent br-mesh); `cidr` is the managed subnet
    for nat/route mode.
  EOT
  type = object({
    name      = string
    mode      = string
    bridge    = string
    cidr      = string
    autostart = bool
  })
  default = {
    name      = "mde-cloud"
    mode      = "nat"
    bridge    = "br-mesh"
    cidr      = "10.44.0.0/24"
    autostart = true
  }

  validation {
    condition     = contains(["nat", "bridge", "none", "route"], var.network.mode)
    error_message = "network.mode must be one of nat|bridge|none|route."
  }
}

variable "vms" {
  description = <<-EOT
    Declarative VM set: name => { vcpu, memory_mb, disk_gb }. OpenTofu converges
    the local hypervisor to EXACTLY this set (the Heat/Nova replacement) — a name
    present here is created, a name removed is destroyed. Ansible (the configure
    leg) then drives each VM's /etc/mackesd/site.yml convergence.
  EOT
  type = map(object({
    vcpu      = number
    memory_mb = number
    disk_gb   = number
  }))
  default = {}

  validation {
    condition = alltrue([
      for v in values(var.vms) : v.vcpu >= 1 && v.memory_mb >= 512 && v.disk_gb >= 1
    ])
    error_message = "each VM needs vcpu>=1, memory_mb>=512, disk_gb>=1."
  }
}

variable "mesh_join" {
  description = <<-EOT
    Mesh-join parameters baked into each VM's cloud-init (SEC-001 join path). The
    join token is NOT a literal here — it is resolved at apply time from the mesh
    secret store via the mde-seal bridge (secrets.tf / var.join_token_secret), so
    no enrollment secret lives in tracked config or tfvars.
  EOT
  type = object({
    lighthouse_overlay_ip = string
    ssh_authorized_key    = string
  })
  default = {
    lighthouse_overlay_ip = "10.42.0.1"
    ssh_authorized_key    = ""
  }
}

variable "join_token_secret" {
  description = <<-EOT
    The mesh-secret-store name (mcnf-secret.sh) the Nebula enrollment/join token is
    sealed under. Resolved at apply time by the mde-seal external data source
    (secrets.tf) — NEVER inlined in config, tfvars, or argv.
  EOT
  type        = string
  default     = "nebula-join-token"
}

variable "mde_seal_helper" {
  description = <<-EOT
    Path to the mesh secret-store CLI the secrets bridge shells out to
    (`<helper> get <name>`). Defaults to the in-repo automation/secrets path;
    override for a deployed node where it lives on $PATH as mcnf-secret.sh.
  EOT
  type        = string
  default     = "../../../automation/secrets/mcnf-secret.sh"
}
