# Cuttlefish Fixture Source Pin

- Worklist: `WL-REL-006`
- Target: `x86_64`, `aosp_cf_x86_64_phone`
- Upstream: `https://android.googlesource.com/device/google/cuttlefish.git`
- Pinned source commit: `a1162ca7a4e6297f1699b65052a8c2dd466fd518`
- Retrieved: 2026-08-16
- License: Apache-2.0 (upstream repository notices govern the checkout)

## Reproducible host-tools recipe

```text
git clone https://android.googlesource.com/device/google/cuttlefish.git
git -C device/google/cuttlefish checkout --detach a1162ca7a4e6297f1699b65052a8c2dd466fd518
cd device/google/cuttlefish
./tools/buildutils/build_packages.sh
```

The paired AOSP device image and `cvd-host_package.tar.gz` are not yet built;
no image digest, package hash, signed declaration, or production claim is made
by this source pin. The resulting bytes must be recorded in a completed fixture
substitution record before admission.

## Bootc lane progress

The separate `.170` bootc lane successfully resolved the official Fedora bootc
index against the same source revision and epoch:

- Reference: `quay.io/fedora/fedora-bootc:44`
- Architecture: `amd64`
- Resolved manifest digest: `sha256:35f5a8e7e7417a3b15a4d62d1a950ab8a873af0a0a8c20105d079224c01ac64c`
- Receipt kind: `mcnf-bootc-image-digest`

This is bootc evidence, not a completed Cuttlefish image build; the Cuttlefish
image and matching host package remain outstanding.
