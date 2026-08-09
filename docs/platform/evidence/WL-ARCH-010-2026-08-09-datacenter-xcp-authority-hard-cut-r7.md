# WL-ARCH-010 Datacenter/XCP VM authority hard cut — 2026-08-09

## Outcome

Typed Workloads remains the only production VM lifecycle and runtime-state
authority. This slice deletes the remaining XCP-specific production actuator
and roster paths:

- all twelve `action/dc/vm-*` responder verbs and their direct XAPI/Tofu VM
  effects;
- XAPI `vm-list` sampling and `event/dc/vm/*` publication;
- `xcp_provision`, `xcp_host`, `action/provision/*`, and
  `compute/xcp-host/*`;
- the runtime `mackes-xcp` crate, workspace/dependency edges, spawn sites, and
  worker-registry entries;
- the retired Server and Hypervisor install profiles and selectable Hypervisor
  capability tag.

Retained pre-upgrade VM rows fail closed: the Datacenter responder does not
consume or reply, the inventory differ cannot admit or publish them, and the
Datacenter job/audit observers admit only verbs that still have a registered
production responder. Non-VM Datacenter behavior remains: adopted-XCP host,
SR/VDI/ISO/template/network inventory and storage operations, gateway state,
DigitalOcean lighthouse/genesis planning, and OpenTofu actions.

The authority inventory and privileged-consumer ledger now describe the live
boundary. `lint-workload-authority.sh` fails if the deleted modules/crate,
registrations, Datacenter VM verbs, XAPI VM roster sampling, or workspace
dependency return.

## Focused verification

- Farm 193 (`.90`), slot `arch010-xcp-dc-r2`: `cargo test -p mackesd --lib
  --features async-services retired_vm --locked -- --nocapture` passed 4/4.
- Farm 9 (`.50`), slot `arch010-xcp-profiles-r2`: `cargo test -p mackesd --lib
  --features async-services install_profiles::tests --locked -- --nocapture`
  passed 14/14.
- Farm 194 (`.170`), slot `arch010-xcp-tags-r1`: `cargo test -p
  mackes-mesh-types cap_tags::tests --locked -- --nocapture` passed 4/4.
- Farm 196 (`.196`), slot `arch010-xcp-bus-r1`: focused `mde-bus` audit
  classification passed 1/1.
- BigBoy (`.130`), slot `arch010-xcp-registry-r2`: the final worker-registry
  census test passed with 145 total registrations and 82 role-tiered entries.
- Farm `.50`, slot `arch010-xcp-fmt-r2`: scoped Rust formatting passed for the
  changed implementation files. `worker_role.rs` and `workers/mod.rs` retain
  pre-existing navigation/clock and module-order reflows outside this slice.
- Farm `.196`, slot `arch010-xcp-lints-r1`: authority self/live, worklist
  self/live, and documentation-supersession lints passed.
- Farm `.170`, slot `arch010-xcp-metadata-r1`: locked workspace metadata
  resolved with no `mackes-xcp` package or dependency edge.
- `git diff --check` passed.

Source SHA-256 pins for this proof are:

```text
622ec6c9c925f6e7c7033353867f403d2a535d803b81744593bceb72ddc5bb7a  Cargo.toml
024382374fa1d7980b8d8adbd3b5deed8fa2ed810bceb03b303724842c489c67  Cargo.lock
8bcbbabb77b04d04981f5fa71ba1e5699a7c2bf0cc772ae8988e1a582702f2c2  ipc/datacenter.rs
e4d07211e4107ce82b3311760a8abf28652742a65dfe6d93df971943055b5ceb  workers/datacenter_orchestrator.rs
b184b311765e262234c98b0d61a7dfd7ebd08f0bca488c62fcd1a41a67b8fb48  workers/dc_jobs.rs
0f3b534f7f9586d9585ca9bb19212b91e8d665d052b8d145eae7e81ad6b01a11  workers/dc_auditor.rs
ad7ddeec713ee393ae5adcb9f0ba013a67830b0453e916b937ed837fddac0c67  worker_role.rs
629b96cb8e46f53db77294fbde24092933b46419eba83f1d98d8864881ae4808  install_profiles.rs
7074ae0ef35a8c01b0c21c7af6910dc7b7e685294b125da7a56f7f079c5ce13d  cap_tags.rs
c9440eb5a19751efa78d335eb96a3e883e6a73e62276a9137650eb3ce4383ba4  lint-workload-authority.sh
```

The initial full-workspace formatting probe was intentionally not treated as a
slice failure: it reported unrelated existing formatting drift across the
workspace. The scoped check above covers the changed source.

## Remaining boundary

This checkpoint does not close ARCH-010 S1 or the epic. The legacy
`compute_provision`/`compute/create` domain-creation path and any surviving
cloud caller overlap still require the same hard-cut audit. Real
libvirt/virtqemud lifecycle, restart recovery, native attachment, package, and
multi-seat evidence also remain open.
