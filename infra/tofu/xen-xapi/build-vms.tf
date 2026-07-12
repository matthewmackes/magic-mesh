# The MCNF build farm, XAPI-native (no XO). FARM-AUTOSCALE shape model ported into
# the XAPI root (DAR-31). Previously THREE hardcoded adopt-only resources
# (build_50/51/52). Now a `for_each` over a `local.build_vm_specs` shape model so
# the farm scales elastically (big / small×N / off per dom0) the same way the
# deprecated XO root (../main.tf) does — but driven by the XAPI-native provider.
#
# CRITICAL MIGRATION (CONTRADICTION-2): porting from the 3 named resources to
# `for_each` CHANGES every resource address (e.g. xenserver_vm.build_50 →
# xenserver_vm.build_xhs["xen-home-services-0"]). OpenTofu reads an address change
# as destroy-old + create-new — which would DESTROY the three LIVE adopted build
# VMs. The `moved {}` blocks at the bottom of this file declare the rename so a
# plan is 0-add / 0-change / 0-destroy against live state (the moved blocks are
# CONSUMED, no destroy is proposed). NEVER remove them without a `tofu state mv`.
#
# Why THREE `for_each` resources (build_xhs / build_kvm / build_big) and not one
# `xenserver_vm.build`: the farm spans 3 standalone XCP-ng pools, so each VM needs
# a different aliased `xenserver` provider (xhs / kvm / big). OpenTofu does NOT
# allow the `provider` meta-argument to vary per `for_each` instance (it must be a
# static reference), so a single `for_each` resource cannot span the 3 pools. The
# canonical pattern is one `for_each` resource PER provider alias, each iterating
# the subset of build_vm_specs whose dom0 is on that pool. This also lines up with
# DAR-32's rendered-provider-alias plan (one resource block per registry dom0).

locals {
  # Per-dom0 substrate — cold facts copied from ../main.tf's local.dom0, with the
  # XAPI-native bits the no-XO root needs instead of the XO data-source lookups:
  #   provider_alias — which aliased `xenserver` provider (providers.tf) drives it,
  #   network_uuid   — the pool's eth0 network UUID (the value the adopt-only
  #                    resources carried inline; XAPI takes the UUID directly,
  #                    whereas the XO root resolved it from a data source).
  # `ip_base` is the FIRST build-VM IP on that dom0; `small` VMs step the last
  # octet +10 each (small-0=base, small-1=base+10, …), capped at 4 (a 40-wide
  # lane); ip_bases are spaced 40 apart so no dom0's lane overlaps another's.
  # `big_vcpus`/`big_mem_gib` size the whole-host `big` VM.
  dom0 = {
    "xen-home-services" = {
      provider_alias = "xhs"
      network_uuid   = "420c5872-dd49-af7f-fe4f-d5e2502429f8"
      ip_base        = "172.20.0.50" # lane .50–.80
      big_name       = "mcnf-build-big-50"
      small_name     = "mcnf-build-home-services"
      big_vcpus      = 3 # ~whole 4-core host, 1 core for dom0
      big_mem_gib    = 18
    }
    "kvm-xcp1" = {
      provider_alias = "kvm"
      network_uuid   = "85bc2d18-849b-4d9a-df9d-ab92ef1e58b8"
      ip_base        = "172.20.0.90" # lane .90–.120
      big_name       = "mcnf-build-big-51"
      small_name     = "mcnf-build-kvm-xcp1"
      big_vcpus      = 3
      big_mem_gib    = 18
    }
    "xen-bigboy" = {
      provider_alias = "big"
      network_uuid   = "8dee4afc-4fc7-60e5-0a3f-7b9b94954631"
      ip_base        = "172.20.0.130" # lane .130–.160
      big_name       = "mcnf-build-big-52"
      small_name     = "mcnf-build-52"
      big_vcpus      = 10 # ~whole 12-core BigBoy, 2 cores for dom0
      big_mem_gib    = 26
    }
    "xen-194" = {
      provider_alias = "x194"
      network_uuid   = "1d940eba-09fb-71e9-e6e5-a7ab5f7259ce" # eth0 (172.20.145.194)
      ip_base        = "172.20.0.170"                         # lane .170–.200
      big_name       = "mcnf-build-big-53"
      small_name     = "mcnf-build-xen-194"
      big_vcpus      = 3 # ~whole 4-core host, 1 core for dom0
      big_mem_gib    = 11
    }
  }

  # Split each dom0's ip_base into the first-3-octets prefix + the last octet, so
  # `small` VMs can step the last octet (+10 each) for distinct LAN IPs.
  ip_prefix3    = { for dk, d in local.dom0 : dk => join(".", slice(split(".", d.ip_base), 0, 3)) }
  ip_last_octet = { for dk, d in local.dom0 : dk => tonumber(element(split(".", d.ip_base), 3)) }

  # ADOPTED BASELINE (the 0-destroy floor): the three live, already-provisioned
  # build VMs (descriptive names on .50/.90/.170, BigBoy kept as mcnf-build-52).
  # They are present
  # in the shape model UNCONDITIONALLY (even with the default shape={}), keyed by
  # the small-0 key "<dom0>-0", so that:
  #   (a) the `moved {}` blocks have a live target — a plan with shape={} is
  #       0-add/0-change/0-destroy (the adopted VMs are RELOCATED, not destroyed),
  #   (b) `provision_build_ready` / xcp-build still find them by the legacy names.
  # The `vcpus`/`mem_gib` match the adopted resources verbatim so no in-place
  # change is proposed either.
  adopted_build_vms = {
    "xen-home-services-0" = {
      dom0_key = "xen-home-services"
      name     = "mcnf-build-home-services"
      ip_cidr  = "172.20.0.50/16"
      vcpus    = 4
      mem_gib  = 12
    }
    "kvm-xcp1-0" = {
      dom0_key = "kvm-xcp1"
      name     = "mcnf-build-kvm-xcp1"
      ip_cidr  = "172.20.0.90/16"
      vcpus    = 4
      mem_gib  = 12
    }
    "xen-bigboy-0" = {
      dom0_key = "xen-bigboy"
      name     = "mcnf-build-52"
      ip_cidr  = "172.20.0.130/16"
      vcpus    = 12
      mem_gib  = 20
    }
    # XEN-194 (added 2026-06-29): a live VM provisioned via xe (4 vCPU / 11 GiB @
    # .170). NOT yet in tofu state — adopt at deploy with
    # `tofu import xenserver_vm.build_x194["xen-194-0"] <uuid>` (no moved{} block:
    # it's net-new to tofu, not a renamed resource).
    "xen-194-0" = {
      dom0_key = "xen-194"
      name     = "mcnf-build-xen-194"
      ip_cidr  = "172.20.0.170/16"
      vcpus    = 4
      mem_gib  = 11
    }
    # F44 BUILDER (added 2026-07-12): the physical seats are Fedora 44 but the
    # golden (and every other build VM) is Fedora 42, so an RPM built elsewhere
    # links ffmpeg-7 sonames that do not exist on F44 (see
    # docs/F44-BUILDER-AND-SEAT-DEPLOY.md). This VM was rolled NATIVELY on F44
    # from the Fedora-Cloud image via install-helpers/setup-xcp-build-vm.sh (NOT
    # cloned from the F42 golden), on the BigBoy pool @ 172.20.0.131. Like
    # xen-194-0 it is NOT yet in tofu state — the http state backend
    # (10.42.0.99:8390) is down; adopt when it returns with:
    #   tofu import 'xenserver_vm.build_big["xen-bigboy-f44"]' cf288dfc-301f-ae18-9b5f-1da2b1ec7704
    # `ignore_changes=[template_name,hard_drive,...]` means import won't try to
    # re-clone/re-template it. To make it a first-class resource, add an F44
    # golden + a per-VM template override (the golden is global today).
    "xen-bigboy-f44" = {
      dom0_key = "xen-bigboy"
      name     = "mcnf-build-f44"
      ip_cidr  = "172.20.0.131/16"
      vcpus    = 10
      mem_gib  = 24
    }
  }

  # Pure shape→VM-set expansion (L1/L4), mirroring ../main.tf. For each dom0 the
  # chosen shape yields a list of build-VM specs:
  #   big   → ONE VM at the dom0's whole-host size (big_vcpus / big_mem_gib)
  #   small → small_count VMs at the standard build_* size, IPs ip_base, +10, +20…
  #   off   → none (scale-to-zero)
  # Keyed `<dom0>` (big) or `<dom0>-<n>` (small) so the for_each instances stay
  # stable as the count grows/shrinks.
  shape_build_vms = merge([
    for dk, d in local.dom0 : (
      lookup(var.shape, dk, "off") == "big" ? {
        (dk) = {
          dom0_key = dk
          name     = d.big_name
          ip_cidr  = "${d.ip_base}/16"
          vcpus    = d.big_vcpus
          mem_gib  = d.big_mem_gib
        }
        } : lookup(var.shape, dk, "off") == "small" ? {
        for i in range(lookup(var.small_count, dk, 1)) :
        "${dk}-${i}" => {
          dom0_key = dk
          name     = i == 0 ? d.small_name : "${d.small_name}-${i}"
          ip_cidr  = "${local.ip_prefix3[dk]}.${local.ip_last_octet[dk] + i * 10}/16"
          vcpus    = var.build_vcpus
          mem_gib  = var.build_memory_gib
        }
      } : {}
    )
  ]...)

  # The effective spec set = the adopted baseline OVERLAID by the shape model. A
  # shape entry for the same key (e.g. shape=small on xen-bigboy → "xen-bigboy-0")
  # WINS (it carries the autoscaler's chosen size), so the model can still resize
  # an adopted VM; absent a shape, the adopted baseline keeps the live VM present
  # (the 0-destroy floor). `golden_template_name == ""` blanks EVERYTHING (the
  # connectivity-only plan) — including the adopted baseline, which the operator
  # opts into knowingly.
  build_vm_specs = var.golden_template_name == "" ? {} : merge(
    local.adopted_build_vms,
    local.shape_build_vms,
  )

  # Per-VM overrides (var.vm_overrides) win for one-off sizing; dom0_key is
  # re-applied AFTER the merge so an override can't clobber it (it keys the pool's
  # provider alias + network — a bad value would mis-place the VM).
  active_build_vms = {
    for k, v in local.build_vm_specs :
    k => merge(v, lookup(var.vm_overrides, k, {}), { dom0_key = v.dom0_key })
  }

  # Partition the flat spec map by dom0 provider alias — one subset per aliased
  # provider, because `provider` can't vary per for_each instance (see header).
  build_vms_xhs  = { for k, v in local.active_build_vms : k => v if v.dom0_key == "xen-home-services" }
  build_vms_kvm  = { for k, v in local.active_build_vms : k => v if v.dom0_key == "kvm-xcp1" }
  build_vms_big  = { for k, v in local.active_build_vms : k => v if v.dom0_key == "xen-bigboy" }
  build_vms_x194 = { for k, v in local.active_build_vms : k => v if v.dom0_key == "xen-194" }
}

# --- One for_each resource per pool/provider-alias --------------------------
# Each adopts the same clean-plan recipe as the old hardcoded resources:
# ignore_changes covers the create-only / 0.2.x-non-round-trippable fields so an
# adopted VM plans clean (proven: DATACENTER-1 import parity).

resource "xenserver_vm" "build_xhs" {
  for_each          = local.build_vms_xhs
  provider          = xenserver.xhs # XEN-HOME-SERVICES
  name_label        = each.value.name
  template_name     = var.golden_template_name
  static_mem_max    = each.value.mem_gib * 1073741824 # GiB → bytes
  vcpus             = each.value.vcpus
  check_ip_timeout  = 0
  network_interface = [{ device = "0", network_uuid = local.dom0["xen-home-services"].network_uuid }]
  lifecycle {
    ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]
  }
}

resource "xenserver_vm" "build_kvm" {
  for_each          = local.build_vms_kvm
  provider          = xenserver.kvm # KVM-XCP1
  name_label        = each.value.name
  template_name     = var.golden_template_name
  static_mem_max    = each.value.mem_gib * 1073741824
  vcpus             = each.value.vcpus
  check_ip_timeout  = 0
  network_interface = [{ device = "0", network_uuid = local.dom0["kvm-xcp1"].network_uuid }]
  lifecycle {
    ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]
  }
}

resource "xenserver_vm" "build_big" {
  for_each          = local.build_vms_big
  provider          = xenserver.big # XEN-BIGBOY
  name_label        = each.value.name
  template_name     = var.golden_template_name
  static_mem_max    = each.value.mem_gib * 1073741824
  vcpus             = each.value.vcpus
  check_ip_timeout  = 0
  network_interface = [{ device = "0", network_uuid = local.dom0["xen-bigboy"].network_uuid }]
  lifecycle {
    ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]
  }
}

resource "xenserver_vm" "build_x194" {
  for_each          = local.build_vms_x194
  provider          = xenserver.x194 # XEN-194
  name_label        = each.value.name
  template_name     = var.golden_template_name
  static_mem_max    = each.value.mem_gib * 1073741824
  vcpus             = each.value.vcpus
  check_ip_timeout  = 0
  network_interface = [{ device = "0", network_uuid = local.dom0["xen-194"].network_uuid }]
  lifecycle {
    ignore_changes = [hard_drive, template_name, boot_mode, boot_order, cores_per_socket, dynamic_mem_max, dynamic_mem_min, static_mem_min, name_description, cdrom]
  }
}

# --- moved {} — the load-bearing 0-destroy migration (CONTRADICTION-2) -------
# Map each old hardcoded address to its new for_each key in the matching
# per-alias resource. The live adopted VMs are the small-0 of their dom0, so they
# relocate to the "<dom0>-0" key. With these in place a
# `tofu plan` against the live state RELOCATES the resources (0-add/0-change/
# 0-destroy) instead of destroy+recreate. Removing a moved block re-introduces the
# destroy — keep all three.
moved {
  from = xenserver_vm.build_50
  to   = xenserver_vm.build_xhs["xen-home-services-0"]
}
moved {
  from = xenserver_vm.build_51
  to   = xenserver_vm.build_kvm["kvm-xcp1-0"]
}
moved {
  from = xenserver_vm.build_52
  to   = xenserver_vm.build_big["xen-bigboy-0"]
}
