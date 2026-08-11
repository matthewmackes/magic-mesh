# App VM capability-admission checkpoint

Date: 2026-08-10  
Epic: `WL-FUNC-018` S3/S4

## Defect and correction

The catalog-backed Front Door App-VM provision path constructed a bounded
`AppVmLaunchRequest` but did not apply its stronger admitted-policy check before
calling the root-mutation authorizer. An unsupported capability such as
`host_socket` could therefore cross the authorization boundary. The path now
requires `validate_admitted()` before it derives the capability target or
invokes authorization.

## Focused farm proof

Host `.90`, slot `func018-appvm-capability-admission-r192`:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=func018-appvm-capability-admission-r192 \
install-helpers/xcp-build.sh cargo test -p mde-shell-egui --lib \
  front_door::tests::unsupported_flatpak_capability_cannot_reach_app_vm_authorization \
  -- --exact --nocapture

1 passed; 0 failed
```

The hostile regression observes the authorization callback and proves an
unsupported host capability is refused before authorization or Bus payload
creation. This is source/farm admission evidence; image supply, live App-VM
boot, VDI presentation, and physical-seat proof remain open.
