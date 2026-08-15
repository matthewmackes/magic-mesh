# WL-REL-001 source-freeze evidence

Date: 2026-08-15

The clean source boundary is frozen at:

```text
revision: 01ea65dbce966238651f0dccf135cb35f97a1fde
epoch: 1786803392
version: 12.1.6
tag plan: magic-mesh-v12.1.6
git status: clean
git HEAD...upstream: 0 0
```

`install-helpers/source-revision-receipt.sh --repo .` returned the same
revision and epoch. Farm-routed Cargo metadata checks on `172.20.0.50`,
slots `release-version-metadata-farm1` and `release-version-final-farm4`,
confirmed the shipped workspace packages and aligned `mde-role-chooser`
resolve to version `12.1.6`. The three isolated browser helper manifests and
lockfiles were aligned and each returned `12.1.6` from farm metadata checks.

WL-REL-001 S1 and S2 are complete. S3 cannot begin: the required private
release-input argv file and its governed Maps, App VM, Cuttlefish, RPM signer,
and bootc receipts are absent. No artifact build was attempted.
