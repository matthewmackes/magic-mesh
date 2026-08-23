# WL-FUNC-023 leftover — first-boot role-pin strips dest env — r1

Date: 2026-08-22  
Classification: child-env refuse; **not** live enroll and **not** dest replace  
Source revision: after `3ba87f117` (this change)  
Farm host: `172.20.0.50` slot `0`  
`production_admitted: false`

OW-1 `mde-role-chooser` spawned `mackesd role-pin` with a full inherited
environment. Dest identity vars (`MACKESD_BOOTSTRAP_SSH_KEY`,
`MACKESD_BOOTSTRAP_KNOWN_HOSTS`) and `JOIN_TOKEN` could leak into that
first-boot child. Leftover (2) says only the dest-env runner sources
those vars.

## Act

`pin_role_command` now `env_remove`s those three names before spawn.
Login env stays unset. Bootstrap dests were not replaced. Seat 15 was
not invoked. Role-pin remains the first-boot upgrade-only pin.

## Verification

Farm (`.50` slot 0):

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 \
  ./install-helpers/xcp-build.sh cargo test -p mde-role-chooser pin_role_strips
test tests::pin_role_strips_bootstrap_dest_env ... ok
test result: ok. 1 passed; 0 failed
```

Leftover freeze bar is still live mint and enroll/offboard+reenroll
after an unpublished signed candidate exists.
