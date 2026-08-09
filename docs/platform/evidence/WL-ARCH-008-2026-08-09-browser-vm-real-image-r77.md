# WL-ARCH-008 Browser VM real image proof (R77)

Date: 2026-08-09
Host: machine194 (`172.20.0.170`)
Farm slot: `browser-vm-real-image-r77`

## Outcome

A real Browser VM qcow2 was built with the pinned bootc-image-builder, passed
the in-container production image checks, was resized to the governed 64-GiB
virtual size, and passed the canonical profile/manifest/artifact admission
chain. `qemu-img check` reported zero errors.

This is image-build and static admission evidence only. The artifact was not
promoted to a catalog or booted on a seat. It is not Sunshine/Moonlight,
frame/audio/input, reconnect, or other live-seat proof. The truthful transport
identity remains `rdp,spice` with RDP preferred and `host_browser=false`.

## Required packaging correction

The initial pull of the checked-in Fedora bootc base failed before any build:

```text
sudo podman pull quay.io/fedora/fedora-bootc@sha256:4d3600c36e461f77af144750fb7c199f8c80f1c987bad492def4215dcf1bbf7f
Error: ... reading manifest ...: manifest unknown
```

The official Fedora 44 image index resolved amd64 to
`sha256:b7539a4cf967c22ef076922e03e7264816ffd4d8dfbac47ee252e64e6a1d6d08`.
That immutable image pulled successfully and identified itself as bootc
`44.20260809.0`, kernel `7.1.7-200.fc44.x86_64`. `Containerfile` now pins that
digest. `verify-contract.sh` additionally fails unless the default base is an
immutable `quay.io/fedora/fedora-bootc@sha256:<64-hex>` reference.

## Pinned build inputs

```text
bootc-image-builder:
  reference: quay.io/centos-bootc/bootc-image-builder@sha256:2b52843ea2bfda73b0a08d97e76b734393b1d3a804681b9fabb26723bd3a2f0b
  local image id: ba9dec3a9a314dad88bfcdd2299a8d2b441ced1a833b5cc4a335f5ee315a3922

Fedora bootc base:
  reference: quay.io/fedora/fedora-bootc@sha256:b7539a4cf967c22ef076922e03e7264816ffd4d8dfbac47ee252e64e6a1d6d08
  local image id: 3707a998d87434072816b53aacb383532085a96ece785aaeacd565e81df31562

Browser OCI image:
  id: 5a2eb46bed6b96d84352252c3451263889ecc0a132e68a898533f9ee83c77b6d
  digest: sha256:23243c0a44ab0528a9c3fc4e8fcb5a0d8b7c4de3506527c5b1b79bef21a266cc
  source commit: af3348bcfa350c6e2ed0d4f283e3e8d7da4c9ba6
```

The OCI verifier found Chromium, Sway, PipeWire/WirePlumber, xrdp plus its
SELinux policy and glamor driver, qemu/spice guest agents, the thin
`magic-mesh-lighthouse` package, the bounded production-control service, and
all image-owned runtime assets. It also proved the workstation/browser RPMs
and controller secret absent.

## Exact farm commands and results

The source was synchronized through the required helper:

```text
MCNF_BUILD_HOST=172.20.0.170 \
MCNF_BUILD_SLOT=browser-vm-real-image-r77 \
install-helpers/xcp-build.sh sync
# route: MCNF_BUILD_HOST pinned -> 172.20.0.170
```

Focused source contract:

```text
packaging/browser-vm/verify-contract.sh
# Browser VM contract checks passed
```

Real image build, run rootful because bootc-image-builder requires privileged
container/storage and loop-device access:

```text
sudo -n env MCNF_PULL_TIMEOUT=300 \
  packaging/browser-vm/build-image.sh \
  --disk qcow2 --out /var/tmp/browser-vm-real-image-r77
# Build complete!
# Browser VM disk resized to 64 GiB
# Browser VM image manifest written
# Browser VM source profile contract passed (twice: profile and artifact entrypoint)
```

Build log:

```text
32f4ac883815ad9fb611791057c2d50f5ae0147fe239aae22be0a6eed0b37b03  /tmp/browser-vm-real-image-r77-build.log
bytes: 165172
```

The builder emitted non-fatal overlay-unmount and SELinux compiled-regex
version warnings. The pipeline nevertheless completed, unmounted its image
filesystems, and the resulting qcow2 passed both the independent integrity
check and admission checks below.

Explicit identity re-admission:

```text
packaging/browser-vm/verify-profile.sh --source \
  --manifest /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2.mcnf-manifest.json \
  --image /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2 \
  packaging/browser-vm/profile.env
# Browser VM source profile contract passed: browser-vm-chromium

packaging/browser-vm/verify-image.sh --artifact \
  /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2 \
  /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2.mcnf-manifest.json
# Browser VM source profile contract passed: browser-vm-chromium
```

## Artifact and qemu metadata

```text
artifact: /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2
sha256: 96d319d8faddb9a9f406aaee5121d3f18c50f6afd96855b337a77685b78438f2
apparent bytes: 1878852608
allocated bytes reported by qemu-img: 1871978496
format: qcow2
compat: 1.1
cluster size: 65536
virtual bytes: 68719476736 (64 GiB)
dirty flag: false
corrupt: false

qemu-img check --output=json:
  check-errors: 0
  total-clusters: 1048576
  allocated-clusters: 55379
  compressed-clusters: 47513

qemu-img map summary:
  extents: 3520
  data bytes: 3629318144
  zero bytes: 65090158592

manifest: /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2.mcnf-manifest.json
manifest bytes: 3838
manifest sha256: 99654ef81ef604c22af44e6c2c72663240acbb70ea48589a607b74f55aea4478
```

The manifest's artifact digest and byte count exactly match the file. Its
profile record binds image version `browser-vm-chromium-v1`, 4 vCPU, 8192 MiB,
64 GiB, source commit `af3348bcfa350c6e2ed0d4f283e3e8d7da4c9ba6`,
`host_browser=false`, default `rdp`, and transports `rdp,spice`.

## Reproducibility and remaining operational gap

This run proves that the pinned source inputs can produce an admissible real
artifact on machine194. It does not claim byte-for-byte reproducibility from a
single build; bootc-image-builder generated filesystem/partition identifiers,
and a second clean build was not an operation-impacting requirement here.

The artifact and sidecar remain in machine194's `/var/tmp` and are not a
promoted release. The next operator action is to copy the pair into the
authenticated image catalog while preserving both hashes, admit that exact
pair through Workload, and then boot it for the existing RDP/frame/input/audio/
reconnect live acceptance. Sunshine/Moonlight still requires real guest and
Construct implementations; no evidence here satisfies that gap.

## Source hashes

```text
1a41958284e5f0acdacc9d71f0a58ac32b6cf26c408980bf329bc90c6a5bec26  packaging/browser-vm/Containerfile
8abbed8185c6f084bad8a3bcb5437ea157754c449a7f3776b8917765b05dc460  packaging/browser-vm/README.md
fbf1bff5c0851fd73c4001db269f5103bc9cdb34b354fd4d0d8327ffdc1de137  packaging/browser-vm/build-image.sh
52621b6937a945c3958a651f7e79c6af87e47f280e196fea3bfb142d9a0343d0  packaging/browser-vm/verify-contract.sh
e348c65118f1271d4a3b254fcbdf56df45c8553449554c2f6d5e98df05c17461  packaging/browser-vm/verify-image.sh
cce0cdfe4b30d3cd32859d49e196680a93f5398eb9ec23a078fec6561ee9c56c  packaging/browser-vm/verify-image-manifest.py
fba02774dc2d64a115e143d522c9efc4c63a791f6f02cc281590951544df1150  packaging/browser-vm/verify-profile.sh
57c18ac8b8641bd00b0ba8ae83f96c559cbdf319917ba547c354c4f624bcdc5b  packaging/browser-vm/profile.env
```

No WORKLIST edit, commit, push, catalog promotion, VM boot, or live-seat claim
was made.
