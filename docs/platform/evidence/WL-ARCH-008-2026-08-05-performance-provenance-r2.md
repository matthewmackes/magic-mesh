# WL-ARCH-008 — Browser acceptance provenance binding (2026-08-05)

The live-acceptance verifier now rejects a performance artifact whose libvirt
`domain_uuid` does not match the deployment receipt. This closes a provenance
gap without changing the 27 FPS threshold, the 905-second gate, or any live-seat
state. A hostile mismatch fixture is covered by the verifier self-test.

## Verification

- Acceptance verifier self-test: `2 positive, 29 negative`.
- App VM contract checks passed.
- Browser performance runner compile/self-test passed.
- T480 remains below the unchanged threshold (`14.582 presented FPS / 3.589
  RVFC FPS`), so Browser VM production acceptance remains rejected.
