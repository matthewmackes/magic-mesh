# WL-FUNC-020 authenticated Cuttlefish relay — 2026-08-11

- Scope: the installed CloudWorker's Cuttlefish guest transport now binds a
  connected relay to a canonical protected runtime directory, a non-writable
  socket owned by that directory's owner, and the kernel-reported peer UID.
  Validation finishes before catalog, package, lifecycle, or readiness request
  bytes are sent.
- Production path: signed Android catalog and Workloads row → installed
  CloudWorker → Cuttlefish provider polling → guest readiness and VDI relay.
- Farm: BigBoy `172.20.0.130`, slot `2`.
- Focused gate:
  `workers::cloud::verbs::cuttlefish_guest::tests::transport_rejects_writable_guest_relay_before_sending_authority_data`:
  PASS, 1 passed, 0 failed, 4,804 filtered out.
- Remaining epic boundary: typed Workload lifecycle delegation, governed app
  launch, and Remote Sessions attachment are not yet wired end to end.
