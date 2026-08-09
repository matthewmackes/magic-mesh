# WL-CRIT-006 current candidate r8 — 2026-08-09

This is immutable candidate package and manifest evidence only. It does not
claim a signed, deployed, attested, target-compatible, or accepted release.
Any later source commit makes these bytes stale.

## Immutable BigBoy cut

A clean detached scratch worktree at revision
`832726b0e011b067ec756ee8600cb406a985cd9b` was synced to BigBoy
`172.20.0.130` in slot `crit006-candidate-r8`. The shared checkout's dirty files
were not present. The real governed command was:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=crit006-candidate-r8 \
MCNF_BUILD_ARTIFACTS=<scratch>/artifacts ./install-helpers/xcp-build.sh rpm
```

The locked workspace release build passed in 15m09s, the required
`drm,live-vdi,media-mpv` shell relink passed in 6m48s, both RPM size guards
passed, and the final Fedora 42 bytes were pulled from BigBoy:

- workstation `magic-mesh-12.1.6-23.x86_64.rpm`: 89,550,043 bytes; SHA-256
  `377a5ff48aec431cf4dffaca37f5a6e32adca1afe6ddbba590a730290961f2f6`
- lighthouse `magic-mesh-lighthouse-12.1.6-9.x86_64.rpm`: 13,756,769 bytes;
  SHA-256 `c716751a8ac3c7ad1fce34f527564c9749952a69e7d369aee2f30b0af3ff3946`

`rpm -Kv` accepted each header and payload digest but reported no signature;
both RPMs are unsigned. The packaging log was 267,932 bytes with SHA-256
`b128cf4d7403113bff5080855bc528ef4da38a18a1ef288a8a543a267c8ab849`.

## Governed manifest and receipt

`write-candidate-manifest.py` was run against the clean checkout and those exact
pulled RPMs. Its first attempt correctly emitted no document when local `/tmp`
lacked room for both immutable RPM snapshots. The unchanged command was rerun
with `TMPDIR` on a bounded local tmpfs; it then emitted:

- `candidate-manifest.json`: 697 bytes; SHA-256
  `a564b6c3c1b17a36df2e983c2abf00141ac88bfb7b0abbeba9a90d10c5358125`
- `candidate-source-receipt.json`: 1,190 bytes; SHA-256
  `9dcb7162ef4830c20ebc6db75ea591eecd674aa38d37fb74f6a963348e2a4a41`

The receipt binds both different role releases to the same compatible
`12.1.6` version and `x86_64` architecture. The manifest binds:

- lighthouse payload SHA-256
  `59433f906c9082d7308bd1827b6795641ac92d7339188b6260b6f299ab8832d0`
- workstation payload SHA-256
  `3abe22467120ddfb5fc084385376a444a6f45e78cca7f2e9f560d4d37a74ef44`
- shared `mackesd` binary SHA-256
  `f8b4151021315f596e26afe7e75a4842427ae08bfd5409cc265df4ef4b534f66`
- workstation `mde-shell-egui` binary SHA-256
  `3b5f4118e778afea31bd907d2494399b5550818f8e5b020d4c88f497f2c39d8e`

The requested independent manifest-only collector validation passed:

```text
collect-six-node-topology.py: PASS — exact candidate manifest covers
lighthouse, workstation at 832726b0e011b067ec756ee8600cb406a985cd9b
```

The canonical 139-byte PASS log is preserved at
`/tmp/magic-mesh-candidate-r8-artifacts/collector-validation.log` with SHA-256
`b0528b1eac3265a1a2fbb766e4382981d027652a80993076e5682ab9ee445230`.
Its adjacent restored manifest is byte-identical to the writer output above.

## Remaining release blockers

This run did not sign or deploy anything and did not fabricate a publisher
HMAC or attestation. It does not provide GitHub required-check authority, a
signed release-evidence bundle, Fedora 44 workstation compatibility, live
six-node topology/recovery evidence, hardware acceptance, publisher credential
attestation, or operator signing. Those gates remain required before production
promotion. The explicit farm slot, tmpfs, local artifacts, and detached scratch
worktree were removed after recording this evidence, except for the exact
manifest and canonical PASS log retained at the audit path above.
