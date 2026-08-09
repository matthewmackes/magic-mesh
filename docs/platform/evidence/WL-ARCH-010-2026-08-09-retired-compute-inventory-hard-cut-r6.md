# WL-ARCH-010 retired compute-inventory hard cut — 2026-08-09

## Authority correction

`probe_nmap` still read every peer's retired `compute-inventory.json` and merged
its VM overlay addresses into the active scan target set. That file was a stale
second runtime roster: a removed or migrated VM could continue influencing
network discovery after typed Workloads stopped presenting it.

The reader and its compatibility tests are deleted. Probe targets now come only
from enrolled peer identity bundles, bounded physical-LAN CIDRs, validated
wide-LAN neighbor observations, known governed service addresses, and explicit
operator targets. ADR-0007 records that Workloads supersedes replicated compute
inventory. The workload-authority lint now scans `probe_nmap`, rejects the
retired filename/reader, and proves the rejection in its hostile self-test.

## Verification

- Machine 193 (`172.20.0.90`), slot `arch010-small-profile-r2`:
  `cargo test -p mackesd --lib probe_nmap::tests::resolve_targets_ --locked`
  passed 3/3, including a negative fixture that plants a retired compute
  inventory and proves it cannot alter target resolution.
- Machine 9 (`172.20.0.50`), slot `arch010-authority-final-r7`:
  `lint-workload-authority.sh --self-test` and the live source scan passed on
  the integrated tree.
- Machine 196 (`172.20.0.196`), slot `integrated-rustfmt-final-r7`: scoped
  `rustfmt --check` passed for `probe_nmap.rs`; `git diff --check` passed.

## Source hashes

```text
602db4ae45b1765ad468a11abeb8080f78700f19af5094ed6bcb86224956a014  crates/mesh/mackesd/src/probe_nmap.rs
365874fe5c4a1bd0363e64e336c6c8f28428abc9210f9027a67724c9ca9e1c72  install-helpers/lint-workload-authority.sh
39c49a0c66ee6cbad21c55ad8bccc23e7b47f5a7dd3015c5123f6fd6fbb1cca6  docs/platform/workload-authority-inventory.md
de49cba69c242f000c366c267354b45c8e2c482e08c693bfcbc0596defb98a19  docs/DECISIONS.md
```

## Remaining acceptance gap

Other directly spawned Datacenter/XCP and cloud lifecycle/roster authorities
remain to be removed, and live Workload restart/attachment proof remains.
ARCH-010 stays `Remaining`.
