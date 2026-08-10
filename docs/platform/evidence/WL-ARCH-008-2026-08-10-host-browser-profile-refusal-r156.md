# WL-ARCH-008 — host-browser profile refusal (r156)

Date: 2026-08-10

Browser VM image-manifest verification now refuses profiles that set
`BROWSER_VM_HOST_BROWSER=true`, preserving the VM-only production boundary.

## Verifier proof

```text
python3 packaging/browser-vm/verify-image-manifest.py self-test --repo-root . --profile packaging/browser-vm/profile.env
Browser VM image manifest self-tests passed
```

Guest image quality and live three-seat proof remain open.
