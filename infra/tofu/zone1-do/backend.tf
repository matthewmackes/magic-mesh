# DATACENTER-2 — DO zone state on SUBSTRATE-V2 (etcd-backed http backend), same as
# the Xen farm. Separate state key (/state/zone1-do) — zones stay independent.
terraform {
  backend "http" {
    address        = "http://172.20.145.192:8390/state/zone1-do"
    lock_address   = "http://172.20.145.192:8390/state/zone1-do"
    unlock_address = "http://172.20.145.192:8390/state/zone1-do"
    lock_method    = "LOCK"
    unlock_method  = "UNLOCK"
  }
}
