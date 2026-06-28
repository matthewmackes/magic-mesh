# DAR-12 — state on SUBSTRATE-V2: the control-VM root's state lives in the same
# etcd-backed http state service (automation/state-backend/) as the other roots,
# under its OWN key `/state/control-vm` (lock 1/7). Any leader-eligible node can
# plan/apply the same state, with locking. No-fixed-center IaC.
#
# NO LITERAL ADDRESS (DAR-9 / §2.8): OpenTofu backend blocks cannot interpolate
# variables, so the state-backend address is NOT written here (a literal `.192`
# would not come along to a new mesh and cannot be shadowed by a var). The address
# is supplied per-mesh at init time:
#   tofu init -backend-config=control-vm.backend.hcl
# where control-vm.backend.hcl is GENERATED (gen-backend-config.sh, DAR-8) and
# gitignored, carrying:
#   address        = "http://<control-vm-overlay-ip>:8390/state/control-vm"
#   lock_address   = "http://<control-vm-overlay-ip>:8390/state/control-vm"
#   unlock_address = "http://<control-vm-overlay-ip>:8390/state/control-vm"
# This block keeps only the lock semantics (the one part that is mesh-invariant).
terraform {
  backend "http" {
    lock_method   = "LOCK"
    unlock_method = "UNLOCK"
  }
}
