# Native F44 builder recovery — r1

Date: 2026-08-22  
Classification: farm recovery; **not** a native RPM cut, prepare close, or freeze  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Operator asked to fix the farm at 133 while other leftover work continued.
There is no build VM, tofu address, or ARP entry at `172.20.0.133` or
`172.20.145.133`. The down native farm builder is `mcnf-build-f44`
(`172.20.0.131`, UUID `cf288dfc-301f-ae18-9b5f-1da2b1ec7704`).

## Act

Documented BigBoy handoff (`docs/F44-BUILDER-AND-SEAT-DEPLOY.md`): F44
needs 24 GiB and cannot share XEN-BIGBOY with `mcnf-build-52` (20 GiB).
Two `cargo test -p mackesd` processes on `.130` had been running 8h and
12h with no progress; they were stopped. Then:

1. `xe vm-shutdown` `mcnf-build-52` → halted; host `memory-free` rose to
   ~30.9 GiB.
2. `xe vm-start` `mcnf-build-f44` → running; `memory-free` settled at
   ~4.9 GiB.
3. SSH `mm@172.20.0.131`: hostname `mcnf-build-f44`, Fedora 44,
   `172.20.0.131/16`.
4. `setup-build-vm-toolchain.sh --host 172.20.0.131` → DONE. rustc 1.94.0,
   cargo-generate-rpm 0.21.0, mpv 2.5.0.

`.50` / `.90` / `.170` / `.196` were left up. Native F44 RPM still needs
`MCNF_RELEASE_INPUT_ARGV_FILE` and a complete preflight. No cut ran here.

## Leftover

FUNC-023 live mint/enroll still needs a real mesh-id (not invented).
Restore `mcnf-build-52` when the F42 BigBoy slots are wanted again; both
VMs have `auto_poweron=true` and cannot stay running together.
