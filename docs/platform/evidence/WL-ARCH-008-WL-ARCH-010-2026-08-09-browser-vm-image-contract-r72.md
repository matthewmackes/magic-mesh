# Browser VM image/profile identity contract (R72)

Date: 2026-08-09
Worklist: `WL-ARCH-008`, `WL-ARCH-010`
Base commit: `16b37d2a83c4128aadcd378bb36b204b507c302d`

## S4 audit result

The existing Browser VM source profile and image recipe already enforce the
following production properties:

- exactly 4 vCPU, 8192 MiB memory, and a 64-GiB qcow2/raw virtual disk;
- Fedora bootc with guest-owned Chromium and Sway, with
  `BROWSER_VM_HOST_BROWSER=false`;
- PipeWire, PipeWire-Pulse, WirePlumber, ALSA support, and bounded media/runtime
  evidence;
- qemu-guest-agent plus the retained SPICE guest agent;
- xrdp as the implemented and preferred Browser VDI endpoint; and
- a pinned bootc base and bootc-image-builder container.

The resource verifier previously admitted values above the profile floors. It
now requires the governed values exactly, preventing nominally compatible but
unreviewed 5-vCPU, larger-memory, or alternate-size identities from sharing the
same profile ID.

The requested Sunshine alternate is not implemented. The image has no
Sunshine endpoint and Construct has no admitted Moonlight decoder. The current
`rdp,spice` profile remains honest: RDP is the preferred production path and
SPICE is a compatibility/recovery path. This slice does not rename SPICE to
Sunshine or claim S4 live closure.

## Production correction

Before R72, a built qcow2/raw image had an operator-computed SHA-256 but no
single bounded artifact that tied those bytes to the profile, image version,
resource shape, source revision, and exact runtime build inputs. A Workload
admitter would have needed to infer or duplicate that identity.

`verify-image-manifest.py` is now the sole Browser VM disk-artifact manifest
authority. `build-image.sh --disk ...` writes one canonical sidecar named
`<artifact>.mcnf-manifest.json`, then immediately checks it through both
`verify-profile.sh` and `verify-image.sh --artifact`.

The schema is capped at 64 KiB and has exact top-level/nested fields. It binds:

- complete artifact SHA-256, apparent byte count, format, filename, and
  qcow2/raw virtual size;
- `browser-vm-chromium-v1`, profile/image IDs, exact 4/8192/64 resources,
  source commit, profile SHA-256/bytes, `host_browser`, and transport policy;
- a fixed, ordered set of bounded source assets copied or compiled into the
  guest, each with its relative path, byte count, and SHA-256.

Creation and verification reject non-regular or symlinked inputs, writable
group/other inputs, oversized files/manifests, malformed UTF-8/JSON, duplicate
or unknown fields, noncanonical sidecar names, unsupported formats, wrong
virtual size, image truncation or byte mismatch, and stale profile/runtime
identity. The final image digest remains the complete runtime binding; the
asset records make its derivation independently reviewable without creating a
second lifecycle or profile authority.

An OCI image produced without `--disk` remains an intermediate builder input,
not a Workload-admissible Browser VM artifact.

## ISO output review correction

The first R72 draft treated `anaconda-iso` as if the pinned
bootc-image-builder wrote `$OUT/anaconda-iso/disk.iso`. No ISO build had proved
that path, and the pinned builder image was not cached on machine194 for
digest-specific inspection. The authoritative Red Hat image-mode instructions
instead show the boot ISO at `output/bootiso/install.iso`; the upstream
bootc-image-builder project also identifies `anaconda-iso` as an output type
without establishing the draft's `anaconda-iso/disk.iso` layout.

References:

- [Red Hat RHEL image-mode documentation](https://docs.redhat.com/en/documentation/red_hat_enterprise_linux/10/html-single/using_image_mode_for_rhel_to_build_deploy_and_manage_operating_systems/using_image_mode_for_rhel_to_build_deploy_and_manage_operating_systems)
- [Upstream bootc-image-builder repository](https://github.com/osbuild/bootc-image-builder)

R72 therefore does not guess a replacement path. `build-image.sh` now admits
only its proven qcow2/raw artifact lanes and rejects `--disk anaconda-iso` with
status 2 before Podman resolution or any build mutation. The focused aggregate
contract gate executes that early rejection. This removes the unsupported ISO
claim while retaining the existing bounded qcow2/raw manifest authority.

## Focused machine194 verification

Host: machine194 (`172.20.0.170`)
Slot: `browser-vm-image-contract-r72`

The explicit helper sync was:

```text
MCNF_BUILD_HOST=172.20.0.170 \
MCNF_BUILD_SLOT=browser-vm-image-contract-r72 \
install-helpers/xcp-build.sh sync
```

The completed prior `node-grade-bus-r66` farm workspace was removed first
because machine194 was below the helper's 8-GiB sync safety floor. It contained
only a prior-task farm copy/cache; no source of record or unrelated slot was
removed.

Focused hostile verifier self-tests:

```text
packaging/browser-vm/verify-profile.sh --self-test
# Browser VM profile/manifest self-tests passed

packaging/browser-vm/verify-image.sh --self-test
# Browser VM image provenance/manifest self-tests passed
```

These execute valid creation/verification, then reject a truncated image, an
unknown manifest field, stale profile identity, truncated JSON, and a symlinked
manifest. The existing profile contract gate additionally rejects malformed,
missing, duplicate and unknown profile fields plus symlinked profile input.

A sparse qcow2 fixture exercised the real qemu metadata path without building
an operating-system image:

```text
qemu-img create -q -f qcow2 "$fixture/disk.qcow2" 64G
packaging/browser-vm/verify-image-manifest.py create \
  --repo-root "$PWD" --profile packaging/browser-vm/profile.env \
  --image "$fixture/disk.qcow2" --format qcow2 \
  --manifest "$fixture/disk.qcow2.mcnf-manifest.json"
packaging/browser-vm/verify-profile.sh --source \
  --manifest "$fixture/disk.qcow2.mcnf-manifest.json" \
  --image "$fixture/disk.qcow2" packaging/browser-vm/profile.env
packaging/browser-vm/verify-image.sh --artifact \
  "$fixture/disk.qcow2" "$fixture/disk.qcow2.mcnf-manifest.json"
```

Result: passed. A copied noncanonical manifest name and a subsequently
truncated qcow2 were both rejected.

The focused aggregate source/contract gate also passed:

```text
packaging/browser-vm/verify-contract.sh
# Browser VM contract checks passed
```

This gate includes shell syntax checks, Python compilation, exact profile
admission, manifest/profile/image self-tests, activation/session/runtime input
contracts, unsupported-ISO early rejection, and existing bounded evidence
verifier self-tests. The direct rejection result was:

```text
packaging/browser-vm/build-image.sh --disk anaconda-iso
# FATAL: unsupported disk output type: anaconda-iso
# status=2
```

No full image, RPM, container, broad workspace, or live-seat test was run.

## Remaining image/live gap

No cached promoted Browser VM image existed in this slot, so this slice did not
perform a bootc OS image build. Closure still requires one real qcow2 produced
by the updated builder, retention of its matching manifest, catalog/Workload
admission of that exact pair, VM boot with the 4/8/64 shape, and the existing
composite live acceptance bundle for frame, input, reconnect, GPU decode,
playback, and capture. Sunshine/Moonlight additionally requires both guest and
Construct implementations before it can replace SPICE as the alternate.
An install/boot ISO is not an R72 build output; adding one later requires a
pinned-builder output-layout test and a separate review of its admission and
size semantics.

## Source hashes

```text
8abbed8185c6f084bad8a3bcb5437ea157754c449a7f3776b8917765b05dc460  packaging/browser-vm/README.md
fbf1bff5c0851fd73c4001db269f5103bc9cdb34b354fd4d0d8327ffdc1de137  packaging/browser-vm/build-image.sh
b507d0c802f6b2a1fa5e72aad5e8d98b908452be4fa0ecffe95f6fb00ec92a64  packaging/browser-vm/verify-contract.sh
e348c65118f1271d4a3b254fcbdf56df45c8553449554c2f6d5e98df05c17461  packaging/browser-vm/verify-image.sh
cce0cdfe4b30d3cd32859d49e196680a93f5398eb9ec23a078fec6561ee9c56c  packaging/browser-vm/verify-image-manifest.py
fba02774dc2d64a115e143d522c9efc4c63a791f6f02cc281590951544df1150  packaging/browser-vm/verify-profile.sh
```

No WORKLIST edit, commit, push, or unrelated local-file change was made.
