# WL-ARCH-008 Browser VM release payload boundary (R499)

Date: 2026-08-13

The static RPM verifier now treats Browser delivery as an exact release
boundary instead of accepting generic VM or `/usr/libexec/mackesd` content.
It requires the immutable image recipe, typed profile, guest runtime service,
and image builder/bootstrap contracts in the source release. It also requires
the base RPM manifest (and, for a built RPM, `rpm -qlp`) to contain the exact
typed Browser workload request, sealed RDP credential provisioner, and matching
credential drop-in.

The new hostile self-test leaves unrelated App VM/cloud-init content and the
Browser credential files present while deleting
`/usr/libexec/mackesd/request-browser-vm-workload`. The Browser-specific check
fails and names that exact missing bootstrap, proving unrelated payload cannot
satisfy the boundary.

## Farm evidence

- `172.20.0.130`, slot `arch008-browser-payload-selftest-r499`:
  `install-helpers/verify-rpm-payload.sh --self-test` passed every assertion,
  including the hostile Browser omission fixture.
- `172.20.0.196`, slot `arch008-browser-payload-syntax-r499`:
  `bash -n install-helpers/verify-rpm-payload.sh` passed.
- `172.20.0.50`, slot `arch008-browser-payload-static-r499`:
  `install-helpers/verify-rpm-payload.sh payload` passed the complete static
  package verifier, including all exact Browser image/profile/runtime/bootstrap
  source and RPM-manifest assertions.

An additional `all` diagnostic reached the separate surface-reachability gate
and reported pre-existing `mde-bookmarks-egui` and `mde-panel-egui` catalog
failures. Those surfaces are outside this slice; the required static package
`payload` gate is green and no surface result is claimed here.

Installed-image and built-RPM execution proof remains part of the deferred,
post-release WL-ARCH-008 acceptance matrix. This change only closes the
release-artifact admission boundary and does not claim live Browser quality.
