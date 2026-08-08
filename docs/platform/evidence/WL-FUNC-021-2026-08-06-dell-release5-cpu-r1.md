# WL-FUNC-021 — Dell release-5 install and CPU proof (2026-08-06)

## Artifact and package boundary

- Native Fedora 44 builder: `172.20.0.131`.
- Artifact: `magic-mesh-12.1.6-5.x86_64.rpm`, 83.5 MiB.
- SHA-256: `c72b9de16f7e0cb9355f092c902f1b44eedd12c751f2e2be4b7246dc754c9ebe`.
- RPM metadata: version `12.1.6`, release `5`, architecture `x86_64`.
- Payload gate passed, including `/usr/bin/mackesd`, `/usr/bin/mde-shell-egui`,
  and the Fedora 44 media dependencies `libavcodec.so.62`,
  `libswresample.so.6`, `libswscale.so.9`, `libmpv.so.2`, and
  `libplacebo.so.360`.

The artifact hash matched after transfer to Dell. The separate RPM transaction
test passed before the authorized install. Dell was Fedora 44 on
`magic-mesh-12.1.6-4.x86_64` before installation and is now on release 5.

## Live verification

- `verify-music-live-seat.sh` on Dell passed: `mde-musicd.service` active with
  `NRestarts=0`, Music bus ping/state/album actions answered, payload entries
  are present, and `rpm -V magic-mesh` is clean.
- The first CPU sample after the file install was deliberately rejected as a
  release-5 claim because the already-running `mackesd` process was 10 hours
  old. It measured max `1149‰`, mean `1092‰` and exposed the stale-process
  boundary.
- After the authorized `systemctl restart mackesd.service`, the bounded
  read-only CPU proof ran for 30 seconds with 15 samples. Result: maximum
  `437‰`, mean `218‰`, and stable restarts `0→0`; the declared thresholds
  are max `850‰` and mean `500‰`. The proof passed.
- The observation-only provider-loss probe produced 15 healthy samples with
  service/provider/catalog/state all `ok`, then returned the expected refusal
  because no natural provider loss occurred. It requested no interruption or
  playback change.

## Scope boundary

This proves the release-5 package and common-seat CPU behavior on Dell only.
The second canonical seat still needs the same installation/proof. Physical
renderer acceptance, natural provider-loss recovery, and live two-seat
owner-yield/resume remain unproven.
