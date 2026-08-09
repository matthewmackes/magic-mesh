# WL-CRIT-006 current candidate r7 — 2026-08-09

This is candidate package-path evidence only. It does not claim a signed,
deployed, attested, or accepted release, and the package bytes become stale when
the source revision changes.

## Immutable BigBoy cut

A clean detached scratch worktree at
`ee5f356794f2042edef13fec663a709b1be68291` was synced to BigBoy
`172.20.0.130` in slot `crit006-candidate-r7`. The shared checkout's dirty
`crates/shared/mde-egui/src/drm.rs` was not present in that worktree. The real
governed command was:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=crit006-candidate-r7 \
MCNF_BUILD_ARTIFACTS=<scratch>/artifacts ./install-helpers/xcp-build.sh rpm
```

The locked workspace release build passed in 15m10s, the required
`drm,live-vdi,media-mpv` shell relink passed in 6m48s, both RPM payload-size
guards passed, and the final bytes were pulled from the farm:

- workstation `magic-mesh-12.1.6-23.x86_64.rpm`: 89,548,425 bytes; SHA-256
  `7198095d386dd44e3090011128ea5a5558528dc5b00fc8d89f8d77cc2303bcdc`
- lighthouse `magic-mesh-lighthouse-12.1.6-9.x86_64.rpm`: 13,754,774 bytes;
  SHA-256 `5340446c7ccf1494c89bd53e6a5a1f165d929970302de579e9005abb06e728cf`

RPM metadata reports both packages as `x86_64` and unsigned. No signing,
credential fabrication, deployment, or live-seat mutation was performed.

## Manifest blocker

The governed writer was then invoked against those exact final bytes and the
exact revision:

```text
./install-helpers/write-candidate-manifest.py \
  --repo <clean-detached-worktree> \
  --revision ee5f356794f2042edef13fec663a709b1be68291 \
  --workstation-rpm <scratch>/artifacts/magic-mesh-12.1.6-23.x86_64.rpm \
  --lighthouse-rpm <scratch>/artifacts/magic-mesh-lighthouse-12.1.6-9.x86_64.rpm \
  --out-dir <scratch>/candidate
```

It refused before emitting either candidate document:

```text
write-candidate-manifest: BLOCKED: cpio --quiet --to-stdout ./usr/bin/mackesd failed: cpio: You must specify one of -oipt options.
Try 'cpio --help' or 'cpio --usage' for more information.
```

The RPM extraction command lacks an explicit cpio extract mode. No wrapper or
bypass was used. Consequently there is no candidate-manifest or source-receipt
hash, and `collect-six-node-topology.py --validate-candidate-manifest` could not
run because no manifest existed. The writer must be corrected and this exact
immutable package/manifest exercise repeated before S1-S3 can claim a governed
candidate document. Publisher credential attestation, signing, deployment, and
final release acceptance remain separate unmet gates.

The explicit BigBoy slot and detached scratch worktree were removed after this
evidence was captured.
