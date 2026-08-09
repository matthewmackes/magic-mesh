# WL-ARCH-010 legacy compute-provision hard cut — 2026-08-09

## Outcome

Typed Workloads remains the only production VM creation/lifecycle authority.
The unconditional `compute_provision` worker, its `compute/create/*` request and
`compute/create-ack/*` reply contract, direct `virt-install` path, module,
spawn, and worker-registry entry were deleted.

A repository-wide caller audit found no production request publisher and no
acknowledgement consumer. The worker was therefore reachable but orphaned. Its
private helpers and 41 module-local tests exercised only the retired contract;
none were retained as filler. Host virtual-storage setup remains owned by the
storage plane, while VM creation uses the typed Workload operation and its
approved-image `qemu-img`/`virsh define` adapter.

The certificate-authority responder was retained at this checkpoint pending a
caller audit. The immediate follow-up found no publisher, packaged client, or
versioned operator API and deleted it; canonical enrollment already signs
through the sealed CA module. Cold migration remains: the same worker owns
source and target protocol handling, while all libvirt effects cross its
bounded, durable Workload actuator channel.

This cut does not claim that legacy automatic guest Nebula enrollment, MeshFS
attachment, or exact audio/video flags have converged through typed Workloads.
Those are feature-equivalence/live-hardware proof gaps, not reachable callers
of the deleted authority.

## Focused verification

- BigBoy (`.130`), slot `arch010-compute-cut-registry-r1`: the canonical worker
  census passed with 144 registrations, 82 role-tiered entries, 106 Lighthouse
  workers, and 144 Workstation workers.
- Farm 193 (`.90`), slot `arch010-compute-cut-migrate-r1`: all 53 cold-migration
  adapter, durability, authentication, and recovery tests passed.
- Farm 9 (`.50`), slot `arch010-compute-cut-cert-r1`: all 22 retained
  certificate-authority responder tests passed.
- Farm 194 (`.170`), slot `arch010-compute-cut-check-r1`: locked async-services
  library check passed.
- Farm 196 (`.196`), slot `arch010-compute-cut-lints-r1`: scoped Rust formatting,
  authority self/live, worklist, and documentation-supersession checks passed.
- `git diff --check` passed.

Source SHA-256 pins for this proof are:

```text
49603ab3cf3cc4e52ea52b71f1274dbec4cf7474a8aff1f4385c38757825ce81  bin/mackesd/spawn.rs
2e0ae28cdb25eb4625d474acc9cef7ec5817a125daf577e6e906d5a5ca362801  worker_role.rs
9aa222e21125dd794a4738ba56c45d746d90c835ad43c7700d39c1be21702ba0  workers/mod.rs
b1513f758d37d64635078a87cdebe1b225cbc4a996e284e16224857c80b16038  workers/cert_authority.rs
7d6c870ec90140e8c0f69f216c18f815ae9b4ce7e0592f2238732ba9f04ec989  workers/compute_migrate.rs
2b532383d5463c1d8f58965a094e3a4d32063e2819a738e212f6c32e8197c267  workers/nebula_supervisor.rs
3f898063e2546094eebfa849bbd899d519402cb70bd901bb1da2fc21df54ba86  lint-workload-authority.sh
```

## Remaining boundary

ARCH-010 remains open. Cloud/OpenTofu lifecycle effects, native attachment,
real libvirt restart/crash recovery, guest feature convergence, package proof,
and multi-seat hardware evidence still require audit or live validation.
