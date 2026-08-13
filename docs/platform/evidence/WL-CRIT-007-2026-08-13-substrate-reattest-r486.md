# WL-CRIT-007 substrate re-attestation before grouped and desktop recovery

Date: 2026-08-13

## Scope

The dedicated boot/resume/network-return helper previously treated a successful
etcd or Syncthing start as durable for the rest of the recovery run. Either
configured substrate service could fail while grouped mackesd children were
settling, allowing grouped or desktop mutation and a false `recovered` status
from stale substrate readiness.

Recovery now re-attests the complete configured etcd/Syncthing pair immediately
before grouped-daemon mutation and again before XDG/session restoration. A lost
service fails closed with the bounded states `substrate-lost-before-grouped` or
`substrate-lost-before-desktop`; no downstream desktop mutation or convergence
claim occurs.

## Farm evidence

- Host `.196`, explicit slot
  `crit007-substrate-reattest-fixture-20260813`:
  `sudo -n install-helpers/test-mesh-peer-recovery.sh` — PASS. The complete
  peer-recovery fault suite passed, including loss of etcd after Syncthing start
  and loss of etcd after grouped readiness. The first case performed only the
  bounded etcd/Syncthing starts and did not touch grouped services; the second
  case did not run XDG/session mutation or publish recovery.
- Host `.170`, explicit slot
  `crit007-substrate-reattest-syntax-20260813`:
  `bash -n install-helpers/mesh-peer-recovery.sh install-helpers/test-mesh-peer-recovery.sh`
  — PASS.

## Remaining acceptance

This closes the bounded substrate-crash windows in the boot/resume/peer-return
helper. WL-CRIT-007 remains open for direct physical suspend/resume and the
remaining selected-seat/lighthouse corrected-forward matrix, including proof of
one authenticated peer/session, restored workload/VDI state, and no duplicate
process or data loss after real transitions.
