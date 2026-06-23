# DATACENTER-2 — state on SUBSTRATE-V2: the http backend talks to the etcd-backed
# state service (automation/state-backend/), so farm state + its lock live in the
# mesh-replicated etcd store, not a single host's local file. Any leader-eligible
# node can plan/apply against the same state, with locking. No-fixed-center IaC.
terraform {
  backend "http" {
    address        = "http://172.20.145.192:8390/state/xen-xapi"
    lock_address   = "http://172.20.145.192:8390/state/xen-xapi"
    unlock_address = "http://172.20.145.192:8390/state/xen-xapi"
    lock_method    = "LOCK"
    unlock_method  = "UNLOCK"
  }
}
