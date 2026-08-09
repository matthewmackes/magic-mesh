# WL-ARCH-010 shell runtime-projection hard cut — 2026-08-08

Status: this authority-removal slice is implemented and focused farm proof
passes. `WL-ARCH-010` remains `Remaining`; native attachment and the full live
lifecycle/recovery matrix are not proved by this slice.

## Change

- Console's Containers & VMs group now has one typed Workloads link. The raw
  `podman ps --all` and `virsh list --all` shortcuts were deleted, so the shell
  cannot present a second VM/container inventory that disagrees with
  `state/workloads/<node>`.
- Datacenter's retired OpenStack/Nova domain-name heuristic, Cloud-managed
  badge, and alternate warning path were deleted. Every displayed VM row now
  comes from the typed `WorkloadStateSnapshot` backend and power dimensions.
- `lint-workload-authority.sh` now rejects raw Podman/libvirt command literals
  and the retired Nova detector/badge symbols in production shell sources. Its
  self-test proves the raw command fixture is rejected and the typed Workloads
  link is accepted.
- The authority inventory records the hard cut. The completed OpenStack
  deletion blueprint now carries the required historical/superseded banner.

## Verification

- BigBoy `.130`, slot `arch010-shell-authority-r4`: release-mode shell test
  `console::tests::the_entry_table_matches_the_locked_taxonomy_and_holds_no_dead_rows`
  passed 1/1 with 1,490 unrelated tests filtered.
- The same warmed BigBoy artifact passed
  `console::tests::the_containers_and_vms_link_routes_to_the_workloads_surface`
  and
  `datacenter::tests::start_and_stop_actions_are_host_targeted_typed_operations`,
  1/1 each with 1,490 unrelated tests filtered.
- Machine `.90`, slot `arch010-authority-lint-r4`: authority lint self-test and
  live repository scan passed.
- Machine `.196`, slot `arch010-docs-r4`: doc-supersession lint passed.
- Machine `.50` exposed pre-existing crate-wide `cargo fmt` drift in unrelated
  shell files; no clean format claim is made and those unrelated files were not
  rewritten. Scoped `git diff --check` passed.

## Source identity

- `console/mod.rs`: `ca584757079edb3b592d81f25ab2081c6bdbad853c0992809366c4bb2930e746`
- `console/tests.rs`: `c14373c9aa68f38fc36ba4228061e305eded6f82713dfb3b10795e0178f95b0f`
- `datacenter.rs`: `29c8fb634705626776d812e9c586bd7693841aedac0fca60027dfa0ef4bd5903`
- `lint-workload-authority.sh`: `be210568143ad8d81d85c5d90771b3c798f8992db4835c335009910a8a561d3f`
- `workload-authority-inventory.md`: `fc6d066c126338112274f3e599c56b16038adc20e4463b9b40df27b1589ab3b5`

This proves one shell presentation authority and typed lifecycle routing. It
does not claim Display1 first-frame/input/audio/clipboard, live VM/container
restart recovery, or five-seat/three-lighthouse release acceptance.
