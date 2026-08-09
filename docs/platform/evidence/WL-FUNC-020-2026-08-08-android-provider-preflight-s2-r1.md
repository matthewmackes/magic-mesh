# WL-FUNC-020 Android provider/image preflight — 2026-08-08

The Cuttlefish provider now registers only after consuming an admitted signed
catalog and verifying its package/image/catalog bindings against the configured
immutable image. Production preflight hashes that image, validates its stable
filesystem identity, requires real `/dev/kvm`, enabled vendor nested
virtualization, sufficient CPU/memory/disk, and healthy libvirt, then publishes
typed readiness or exact refusal through the existing cloud state authority.

Provider registration is removed when preflight fails. Outer-VM health never
becomes fabricated Android guest readiness, and image hash reuse is bound to
path, device, inode, size, and modification time.

## Verification

- BigBoy provider/image preflight passed 3/3.
- BigBoy additive cloud mirror contract passed 1/1.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

The production key, signed catalog/package manifest/image artifact, real nested
Cuttlefish placement, and S3 guest lifecycle remain. FUNC-020 stays `Remaining`.
