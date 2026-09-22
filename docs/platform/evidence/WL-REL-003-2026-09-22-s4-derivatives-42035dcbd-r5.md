# WL-REL-003 S4 dest-cut derivatives — r5

Date: 2026-09-22
Classification: source leftover S4 attempt; **not** six-role plan, publication,
live enroll, or dest invention
`published: false`
`production_admitted: false`

S6 is not done. Output path
`/home/mm/mcnf-private-s4/derivatives-42035dcbd` remains absent.

## Invocation

Admitted BigBoy slot 1. Helper tree `5ca88bdd0`. Freeze SHA `42035dcbd`.
Same dest-cut RPMs and F44 bootc bases as r4. Did not start `.131`.
Exit `2`. Elapsed about 18 minutes (image layers were already cached).

## Exact failed gate

The r4 sidecar-name check passed. Re-verification then refused:

```
verify-browser-vm-image-manifest: manifest is stale or does not match the profile, image, or runtime assets
release-derivative-images: REFUSED: published Browser VM profile/manifest re-verification failed
```

The builder writes the manifest for `disk.qcow2`. The publisher copies that
file beside `browser-vm-chromium.qcow2`. `artifact.filename` stayed
`disk.qcow2`, so the published object did not match a manifest built for the
published image. The publisher now rewrites that filename before re-verify.
Hostile suite after the rewrite:
`test-build-release-derivative-images: hostile orchestration suite passed`.
