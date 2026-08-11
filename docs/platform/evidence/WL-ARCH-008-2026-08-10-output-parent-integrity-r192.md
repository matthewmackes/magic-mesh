# WL-ARCH-008 — portable destination-parent integrity (r192)

Date: 2026-08-10

The portable Browser profile migration now checks every existing component of
the requested bundle output parent without resolving it. A symlink or a
non-directory component is refused before the staging directory is created, so
an attacker cannot redirect a successful bundle publication into another tree.

## Farm proof

Farm: `.90` (`172.20.0.90`), slot `arch008-output-parent-integrity-r192`.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch008-output-parent-integrity-r192 install-helpers/xcp-build.sh sync
printf '%s\n' 'cd ~/magic-mesh-farm-arch008-output-parent-integrity-r192' 'python3 install-helpers/verify-browser-portable-boundary.py --self-test' 'exit' | MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch008-output-parent-integrity-r192 install-helpers/xcp-build.sh shell

migrate-browser-profile: self-test passed
browser portable boundary: PASS
verify-browser-portable-boundary.py: self-test passed
```

The new hostile fixture places the requested output below a symlinked parent,
requires migration refusal, and confirms the redirected target has no bundle.
The proof is disposable source-level validation; live legacy-profile discovery,
guest restore, package upgrade, and three-seat Browser VM quality/performance
proof remain unverified.
