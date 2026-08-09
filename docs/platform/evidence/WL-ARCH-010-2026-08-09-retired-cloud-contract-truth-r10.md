# WL-ARCH-010 retired cloud contract truth — 2026-08-09

## Outcome

The retained cloud API, daemon comments, and Workloads shell now describe the
same authority boundary as the runtime. OpenTofu is read/dry-plan only, Ansible
Configure remains the one armed cloud mutation, and VM/container lifecycle is a
typed Workload-row operation. Shared wire constants for retired verbs remain
only for compatibility refusals; their documentation no longer promises a live
implementation.

The guided provisioning flow ends after desired-state persistence and a dry
plan, then directs the operator to Start/Attach on the resulting Workload row.
The dead `Applied` GUI state, live-apply capability gate, direct-provision
progress mapping, stale container-deploy copy and fixtures, and one test for a
deleted plan-only apply validation were removed. The retained negative tests
still prove that passive rendering and Plan never publish the retired provision
verb. Service-container image build now fails with the actionable requirement
for an approved OCI image in a typed Workload declaration.

The privileged-consumer inventory no longer lists deleted VM/container worker
roots. The authority lint now rejects restoration of those worker files and
shell publication of either retired cloud provision verb, while requiring the
compatibility `container-deploy` handler to keep failing closed.

## Farm verification

- BigBoy (`172.20.0.130`), slot `arch010-contract-truth-cloud-r1`:
  `cargo test -p mackesd --lib --features async-services workers::cloud --locked -- --nocapture`
  passed 201/201; the final service-container refusal test also passed 1/1 after
  its contract-accurate rename.
- Machine 193 (`172.20.0.90`), slot `arch010-contract-truth-types-r1`:
  `cargo test -p mackes-mesh-types cloud:: --locked -- --nocapture` passed
  37/37.
- Machine 9 (`172.20.0.50`), slot `arch010-contract-truth-shell-r1`:
  `cargo test -p mde-shell-egui iac::tests --locked -- --nocapture` passed
  56/56 on the final synchronized source.
- Machine 194 (`172.20.0.170`), slot
  `arch010-contract-truth-shell-check-r1`: the cold shell check exhausted the
  farm VM filesystem and produced no source verdict. Only that disposable
  workspace was removed; this infrastructure failure is not counted as proof.
- Machine 196 (`172.20.0.196`), slot `arch010-contract-truth-lints-r2`: scoped
  Rust formatting, authority self-test/live lint, worklist self-test/live lint,
  and document supersession lint passed. The active worklist remains honest at
  18 Remaining.

## Source hashes

```text
88b029ff826e9f72a42fd01d5fb18a011ad011983586814bf3f3f15e029c6dad  crates/desktop/mde-shell-egui/src/iac/mod.rs
8f2b21ab4133b863cb95df725adc1d30c6bc357e130cea6a334345de77bdf341  crates/desktop/mde-shell-egui/src/iac/provision_form.rs
3a2cdc0b5ad4abdfb540ee1e9e0fce9ab9472a50abda9023cc3ff1198044559a  crates/desktop/mde-shell-egui/src/iac/tests.rs
66e8e4a0750e780f9976cf34ae3879ab76c2f4075294252664736f8c62b09b26  crates/mesh/mackes-mesh-types/src/cloud.rs
f096821ccd849174d72bdde3ed1c4b3a76bd8c16d7ef9d3c5403eabefe8acec4  crates/mesh/mackesd/src/workers/cloud/runner.rs
13d1d9ea3ed3b7e2af093f0e11880dac47a1d8604cea7d3e863b5fb02416f950  crates/mesh/mackesd/src/workers/cloud/verbs.rs
d5d8faa9541feffd2455252d6baa4544f53b0807b704268223de2d3742ff49cd  crates/mesh/mackesd/src/workers/cloud/verbs/image.rs
0036331944c8b9f20f43a482bf28b1fe191da7029b4ebd0ba5c3a9982937dfaa  docs/security/privileged-bus-consumer-inventory.md
9a3f443eba21196b1446b6ce2d5feb689b25ca599e30ffec61cc9d766c9d38fd  install-helpers/lint-workload-authority.sh
```

## Remaining boundary

This correction removes stale contracts and executable-looking UI around an
already retired authority, but does not close WL-ARCH-010. Typed lifecycle
integration, native attachment, package/live-seat proof, and the remaining
authority audit keep the epic `Remaining`.
