# WL-ARCH-010 VM domain identity collision hardening — r184

## Gap closed

The Workload VM actuator previously mapped only the final `WorkloadId` component
to the libvirt domain and qcow2 overlay names. Distinct app workloads sharing a
catalog revision could therefore address the same guest during start, attach,
recovery, or cleanup.

`workload_compute.rs` now derives both names from the full Workload identity,
retains a bounded readable suffix, and appends a deterministic digest. All
libvirt probes, lifecycle commands, Display1 lookup, and overlay cleanup use
that same mapping.

## Hostile regression

`hostile_workload_id_suffixes_cannot_alias_a_vm_domain_or_overlay` compares two
different app identities with the same terminal `catalog-7` component and
requires distinct bounded domain and overlay paths.

## Farm proof

Source snapshot under test: `38b0472b` plus only the scoped workload patch in a
clean temporary worktree. BigBoy (`172.20.0.130`), slot
`arch010-vm-domain-identity-clean-r184`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-vm-domain-identity-clean-r184 \
  install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
  workers::workload_compute::tests::hostile_workload_id_suffixes_cannot_alias_a_vm_domain_or_overlay \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4714 filtered out; finished in 0.00s`.

The equivalent gate against the shared dirty tree was not used as proof because
an unrelated pre-existing `workers/vehicle.rs` edit fails compilation for a
missing `BTreeSet` import.
