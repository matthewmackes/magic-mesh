# WL-REL-003 S4 dest-cut derivatives — r4

Date: 2026-09-22
Classification: source leftover S4 attempt; **not** six-role plan, publication,
live enroll, or dest invention
`published: false`
`production_admitted: false`

S6 is not done. Output path
`/home/mm/mcnf-private-s4/derivatives-42035dcbd` remains absent. The helper
trap removed the staging directory on refuse.

## Invocation

Admitted BigBoy slot 1 (`172.20.0.130`). Freeze SHA `42035dcbd` /
epoch `1788153988`. Helper tree `2fdd56412` (App VM `cpio` install).
App and Browser bases both
`registry.fedoraproject.org/fedora-bootc@sha256:3a5e74e6…`.
Did not start `.131`. Exit `2`. Elapsed about 36 minutes.

## What passed

App VM image verified:

`sha256:b733413e1a0163c867d7a00c7fb3b92b2c5abc45c27cea331329a984abc4373c`

Browser VM osbuild qcow2 finished, resized to 64 GiB, and the source
profile contract passed twice. The builder wrote
`disk.qcow2.mcnf-manifest.json`, which matches that image name.

## Exact failed gate

```
verify-browser-vm-image-manifest: manifest does not use the canonical image sidecar name
release-derivative-images: REFUSED: published Browser VM profile/manifest re-verification failed
```

The publisher copied that sidecar to
`browser-vm-chromium.mcnf-manifest.json` and then verified it against
`browser-vm-chromium.qcow2`. The verifier requires the sidecar name
`{image.name}.mcnf-manifest.json`, so the published name must be
`browser-vm-chromium.qcow2.mcnf-manifest.json`.

Hostile suite on the control host after that publisher fix:
`test-build-release-derivative-images: hostile orchestration suite passed`.
