# WL-ARCH-008 / WL-ARCH-010 Browser VM catalog and boot boundary (R80)

Date: 2026-08-09
Host: machine194 (`172.20.0.170`)
Farm slot: `browser-vm-catalog-boot-r80`

## Outcome

The retained R77 Browser VM qcow2 and its canonical identity manifest were
promoted without conversion into a new isolated farm catalog. Their hashes are
unchanged, the canonical Workload artifact resolves as
`browser-vm-chromium:r77`, and the promoted artifact again passed `qemu-img`
metadata and integrity checks.

Machine194 cannot run the requested bounded guest boot. It has no `/dev/kvm`,
`qemu-system-x86_64`, `virsh`/libvirt, or OVMF firmware. It also exposes only
four CPUs and about 9.5 GiB MemAvailable, below the existing 9-GiB host
headroom plus 8-GiB guest requirement. TCG was not installed or used.

No production catalog, VM, live seat, rendered frame, input path, or audio path
was mutated or claimed.

## Catalog authority and directly necessary contract

The canonical image catalog is `crates/mesh/mackesd/src/image_catalog.rs` plus
the armed `image-build` promotion verb. Its replicated VM layout is:

```text
<workgroup>/images/<name>/<version>/manifest.toml
<workgroup>/images/<name>/<version>/image.sha256
<workgroup>/images/<name>/<version>/<name>.img
<workgroup>/images/<name>/PROMOTED
```

The live authenticated path consumes an armed token and rehashes the artifact
before replacing `PROMOTED`. Machine194 has no installed `mackesd` binary or
live armed-token service, and this task explicitly prohibited touching a
production catalog. The prior packaging had no bounded offline importer that
could bind the richer Browser VM identity manifest into an isolated instance
of this canonical layout.

`packaging/browser-vm/promote-catalog-image.py` closes that directly necessary
farm/offline boundary. It:

- requires an absolute, previously nonexistent catalog root and refuses any
  update/overwrite path;
- rejects symlinked paths and group/other-writable source leaves;
- executes the existing complete Browser VM profile/artifact/manifest verifier;
- independently requires an exact 64-GiB qcow2 and a clean `qemu-img check`;
- preserves the exact source-named qcow2 and identity manifest;
- exposes `<name>.img` as a hard link to those same bytes, never a conversion;
- records the canonical TOML, SHA sidecar, promotion marker, and a bounded JSON
  admission binding; and
- fsyncs staged leaves/directories before one atomic new-root rename.

This is local root operator authority for a new isolated catalog, not a claim
that the unavailable live Bus armed-token flow ran. Production promotion still
requires that authenticated service and must not reuse the offline helper to
replace an existing catalog.

The first operational promotion review found the temporary root retained
`mkdtemp` mode `0700`. The helper was corrected to publish mode `0755`; only the
new isolated `/var/tmp/browser-vm-catalog-r80` was removed and recreated. The
R77 source pair was not removed or rewritten.

Final reviewer hardening rejects symlinks in both source parent paths and uses
Linux `renameat2(RENAME_NOREPLACE)` for publication. This closes the race where
another actor could create an empty catalog root after the early existence
check and have a plain `rename` replace it. A focused collision gate preserved
both stage and destination, then the exact isolated R80 catalog was removed and
successfully recreated from the retained pair with the no-replace path.

## Retained source and isolated catalog

Retained R77 source:

```text
/var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2
  sha256 96d319d8faddb9a9f406aaee5121d3f18c50f6afd96855b337a77685b78438f2
/var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2.mcnf-manifest.json
  sha256 99654ef81ef604c22af44e6c2c72663240acbb70ea48589a607b74f55aea4478
```

Isolated R80 catalog:

```text
/var/tmp/browser-vm-catalog-r80/
└── images/browser-vm-chromium/
    ├── PROMOTED                         # r77
    └── r77/
        ├── browser-vm-chromium.img      # same inode/bytes as disk.qcow2
        ├── disk.qcow2                   # exact R77 artifact
        ├── disk.qcow2.mcnf-manifest.json
        ├── manifest.toml
        ├── image.sha256
        └── catalog-admission.json
```

The source artifact, preserved catalog artifact, and Workload artifact all had
inode `303954`, link count 3, and SHA-256
`96d319d8faddb9a9f406aaee5121d3f18c50f6afd96855b337a77685b78438f2`.
The source and preserved identity manifest had inode `303970`, link count 2,
and SHA-256
`99654ef81ef604c22af44e6c2c72663240acbb70ea48589a607b74f55aea4478`.

Catalog record hashes:

```text
289b408b475ade7c9d341cfc7f323147cc3e05ea92026b7fe37d1fd06277628c  manifest.toml
3e97f1b50404b0b644eb616e81b71e20e26982edded254161ced0d4f25b79694  image.sha256
a245a1e89fda71dbdaa26ab67af8eb3738ab525688810d060a3070e787686d66  catalog-admission.json
0c4a478d95f08aba74620f926eec82cf59db82046d54fa58c88e7c17aece0592  PROMOTED
```

The catalog root and directories are `0755`; files are root-owned `0644`.
`qemu-img info` reported qcow2, compat 1.1, 64-GiB virtual size, no backing
file, `dirty-flag=false`, and `corrupt=false`. `qemu-img check --output=json`
reported `check-errors: 0`.

## Exact operation-impact verification

Farm sync and package contract:

```text
MCNF_BUILD_HOST=172.20.0.170 \
MCNF_BUILD_SLOT=browser-vm-catalog-boot-r80 \
install-helpers/xcp-build.sh sync
# MCNF_BUILD_HOST pinned -> 172.20.0.170

packaging/browser-vm/verify-contract.sh
# Browser VM contract checks passed
```

Exact-pair promotion:

```text
sudo packaging/browser-vm/promote-catalog-image.py \
  --catalog-root /var/tmp/browser-vm-catalog-r80 \
  --image /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2 \
  --manifest /var/tmp/browser-vm-real-image-r77/qcow2/disk.qcow2.mcnf-manifest.json \
  --name browser-vm-chromium --version r77
# Browser VM source profile contract passed: browser-vm-chromium
# catalog promotion passed: browser-vm-chromium:r77
# sha256:96d319...438f2
# identity-manifest-sha256:99654e...4478
```

Running the same command against the now-existing root failed before hashing or
mutation:

```text
promote-catalog-image: refusing to replace an existing catalog root:
/var/tmp/browser-vm-catalog-r80
rc=1
```

A canonical sidecar with a modified artifact digest was rejected and no target
catalog was created:

```text
verify-browser-vm-image-manifest: manifest is stale or does not match the profile, image, or runtime assets
promote-catalog-image: artifact identity verification failed; catalog remains unchanged
rc=1
```

## Exact boot boundary

```text
packaging/browser-vm/prepare-ephemeral-nocloud.sh preflight \
  --image /var/tmp/browser-vm-catalog-r80/images/browser-vm-chromium/r77/browser-vm-chromium.img \
  --run-dir /var/tmp/browser-vm-r80-run
# prepare-ephemeral-nocloud.sh: required command is unavailable: qemu-system-x86_64
# rc=1
```

Independent probes:

```text
/dev/kvm=absent
qemu-system-x86_64=absent
virsh=absent
OVMF_CODE.fd=absent
nproc=4
MemAvailable: 9964480 kB
```

The exact external remedy is a farm/proof host with nested virtualization
enabled and readable `/dev/kvm`, qemu-system-x86_64, libvirt/virsh, OVMF, at
least 4 vCPU available to the guest, and enough host memory to satisfy the
existing 9-GiB safety floor while assigning 8192 MiB. On that host, use the
same catalog artifact hash, create a disposable overlay/NoCloud seed, assign
exactly 4 vCPU and 8192 MiB, and prove only bounded guest readiness and the RDP
endpoint before any rendered-seat claim.

## Farm storage housekeeping

To restore the helper's mandatory 8-GiB sync safety floor, only recoverable R77
rootful Podman build images were removed by exact image ID: the Browser OCI,
pinned builder, pinned Fedora base, and the untagged controller-build stage.
The retained qcow2+manifest were rehashed afterward and remained unchanged.
Machine194 finished with approximately 9.2 GiB free.

## Source hashes

```text
286e3458aa61c117dd465d231f051c95d6a5ba43c2a8c57f30a71a8bddcfcf65  packaging/browser-vm/README.md
b3cd2f38b03fd1738c8b49fcd0675e5c94bc7189c543a0e4898e411fcaeba5f9  packaging/browser-vm/promote-catalog-image.py
c800cc79e0e045bc5816065c28ed9a05a4858faf90a46a03b632d032435767aa  packaging/browser-vm/verify-contract.sh
```

No WORKLIST edit, commit, push, production-catalog mutation, VM definition, VM
boot, or live-seat assertion was made.
