# WL-ARCH-009 — required process-group launch boundary (2026-08-08)

`mackesd serve` no longer accepts the transitional launch shape with no process
group. The CLI requires `--group <GROUP>`, passes the concrete six-group enum to
`run_serve`, and installs that group on the supervisor before any worker spawn.
This removes the production command-line path to the in-process all-groups
daemon while preserving the existing exact-token parser and registry admission.

The hostile regression invokes the historically accepted `mackesd serve`
command with no group and requires clap to return `MissingRequiredArgument` with
`--group <GROUP>` named in the failure. It therefore exercises the production
CLI parser rather than a duplicate validation helper.

Focused farm verification:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=arch009-required-process-group-20260808-r1 \
install-helpers/xcp-build.sh cargo test --locked -p mackesd --bin mackesd \
  serve_process_group_cli_tests::serve_without_a_process_group_fails_closed \
  -- --nocapture

test result: ok. 1 passed; 0 failed; 0 ignored; 55 filtered out
```

The passing source hash for `crates/mesh/mackesd/src/bin/mackesd.rs` was
`50cf45fd44a205c242debca3df62b0868dc96cc8239fc0cf01d31afa7fc91f6d`.

This is a bounded process-launch checkpoint, not completion of ARCH-009 S4.
The supervisor type still contains its transitional optional admission seam,
and the six systemd units, package cutover, cgroup budgets, crash isolation, and
live recovery proofs remain open. The supervisor and spawn paths were not
changed because concurrent ARCH-010 work owns `workers/mod.rs`,
`workers/console_broker.rs`, and `bin/mackesd/spawn.rs`.
