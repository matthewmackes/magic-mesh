# WL-FUNC-023 leftover — magic-setup stream strips dest env — r1

Date: 2026-08-22  
Classification: child-env refuse; **not** live enroll and **not** dest replace  
Source revision: after `3a09c8a02` (this change)  
Farm host: `172.20.0.90` slot `0`  
`production_admitted: false`

`magic-setup` streamed `mackesd found`/`join`/`add-peer`/`remove-peer`
through `run_streaming`, which inherited the full parent environment.
Dest identity vars (`MACKESD_BOOTSTRAP_SSH_KEY`,
`MACKESD_BOOTSTRAP_KNOWN_HOSTS`) and `JOIN_TOKEN` could leak into those
children. Leftover (2) says only the dest-env runner sources those vars.

## Act

`setup_action.rs` now `env_remove`s those three names on every streamed
child. Login env stays unset. Bootstrap dests were not replaced. Seat 15
was not invoked. Join/found remain the product first-enroll path.

## Verification

Farm (`.90` slot 0):

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=0 \
  ./install-helpers/xcp-build.sh cargo test -p mde-enroll run_streaming_strips
test setup_action::tests::run_streaming_strips_bootstrap_dest_env ... ok
test result: ok. 1 passed; 0 failed
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after an unpublished signed candidate exists.
