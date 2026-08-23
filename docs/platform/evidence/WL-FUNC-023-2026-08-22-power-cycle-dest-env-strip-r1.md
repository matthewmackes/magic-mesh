# WL-FUNC-023 leftover — power-cycle lifecycle spawn strips dest env — r1

Date: 2026-08-22  
Classification: child-env refuse; **not** live enroll and **not** dest replace  
Source revision: after `71ea05c72` (this change)  
Farm host: `172.20.0.50` slot `0`  
`production_admitted: false`

Construct Safe Power Cycle spawned `/usr/bin/magic-setup` with a full
inherited environment. Dest identity vars
(`MACKESD_BOOTSTRAP_SSH_KEY`, `MACKESD_BOOTSTRAP_KNOWN_HOSTS`) and
`JOIN_TOKEN` could leak into that child. Leftover (2) says only the
dest-env runner sources those vars.

## Act

`power_cycle.rs` now builds the launcher through
`lifecycle_launcher_command`, which `env_remove`s those three names.
Login env stays unset. Bootstrap dests were not replaced. Seat 15 was
not invoked.

## Verification

Farm (`.50` slot 0):

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 \
  ./install-helpers/xcp-build.sh cargo test -p mde-shell-egui lifecycle_launcher_strips
test power_cycle::tests::lifecycle_launcher_strips_bootstrap_dest_env ... ok
test result: ok. 1 passed; 0 failed
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after an unpublished signed candidate exists.
