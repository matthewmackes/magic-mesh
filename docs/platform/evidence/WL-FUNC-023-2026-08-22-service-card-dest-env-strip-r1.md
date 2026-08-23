# WL-FUNC-023 leftover — service-card spawn strips dest env — r1

Date: 2026-08-22  
Classification: child-env refuse; **not** live enroll and **not** dest replace  
Source revision: after `b7c67555a` (this change)  
Farm host: `172.20.0.50` slot `0`  
`production_admitted: false`

Chooser `run_service_card_command` spawned `/usr/bin/mackesd service-card`
with a full inherited environment. Dest identity vars
(`MACKESD_BOOTSTRAP_SSH_KEY`, `MACKESD_BOOTSTRAP_KNOWN_HOSTS`) and
`JOIN_TOKEN` could leak into that privileged child. Leftover (2) says
only the dest-env runner sources those vars.

This was the last Construct `/usr/bin/mackesd` spawn without a dest-env
strip. Power-cycle, magic-setup stream, and first-boot role-pin already
strip the same names.

## Act

`chooser/resources.rs` now `env_remove`s those three names before spawn.
Login env stays unset. Bootstrap dests were not replaced. Seat 15 was
not invoked. Service-card verbs are unchanged.

## Verification

Farm (`.50` slot 0):

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 \
  ./install-helpers/xcp-build.sh cargo test -p mde-shell-egui service_card_strips
test chooser::resources::tests::service_card_strips_bootstrap_dest_env ... ok
test result: ok. 1 passed; 0 failed
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after an unpublished signed candidate exists.
