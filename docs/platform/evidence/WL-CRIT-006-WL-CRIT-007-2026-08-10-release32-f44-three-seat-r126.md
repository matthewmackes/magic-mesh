# Release 32 native-F44 three-seat corrected-forward checkpoint

Date: 2026-08-10  
Revision: `08bc13aa92bdefcbe3488f17904a5f685743d517`  
Physical test seats: exactly Dell, Basement seat 15, and Surface

## Rejected candidate

The first release-32 candidate was built on the ordinary Fedora 42 BigBoy VM.
It was signed and staged for a transaction test, but never installed. Dell's
F44 dependency solver rejected its FFmpeg-7 requirements, including
`libavcodec.so.61`, `libswresample.so.5`, and `libswscale.so.8`. No dependency
was filtered or forced. That artifact remains quarantined under
`/root/mcnf-release-artifacts/release32-d2535bdd` as negative evidence.

The correction is durable in commit `bdac550b`: native `xcp-build.sh rpm` now
requires an explicit numeric `MCNF_RPM_TARGET_FEDORA`, probes the builder before
source synchronization, and rejects omitted or mismatched releases.

## Corrected native cut and integrity

The ordinary BigBoy F42 VM `.130` was halted only after its job census was
empty. The dedicated BigBoy F44 VM `.131` then built the exact revision with:

```text
MCNF_BUILD_HOST=172.20.0.131 MCNF_RPM_TARGET_FEDORA=44 \
  install-helpers/xcp-build.sh rpm
```

The guard reported Fedora 44 matching target 44. The base payload gate passed
at 86.7 MiB and the lighthouse payload gate passed at 13.9 MiB. The preserved,
signed artifacts are in
`/root/mcnf-release-artifacts/release32-f44-08bc13aa`:

```text
1499e1c960a4383d763a9ad3e9649ac4b8df6cc7b2ef856ccb6bbb19a65ea20c  magic-mesh-12.1.6-32.x86_64.rpm
0215e2bad9afefcdd4a61df8d492bf5dd80ab0076c42978d52f8d787563c8155  magic-mesh-lighthouse-12.1.6-11.x86_64.rpm
```

`sha256sum -c SHA256SUMS` passed and GPG verified the checksum bundle with
release key `7D1DEAB08928CED418D69ACDE8EAC651D0921C73`. `rpm -K` on every selected
seat reported `digests signatures OK`. The corrected base RPM requires F44
sonames `libavcodec.so.62`, `libavformat.so.62`, `libavutil.so.60`,
`libswresample.so.6`, `libswscale.so.9`, `libplacebo.so.360`, and
`libmpv.so.2`. The complete RPM payload verifier passed.

## Three-seat rollout

Each seat received an `AI-GENERATED-ALERT` and five-second warning before
staging, installation, and temporary-file cleanup. Before installation, each
seat independently matched the signed SHA-256 and passed a DNF test transaction
from release 31 to release 32. The same bytes were then installed sequentially:

| Seat | Address | Fedora | Installed result | Runtime result |
|---|---|---:|---|---|
| Dell (`DELL-LAPTOP`) | `172.20.146.225` | 44 | `magic-mesh-12.1.6-32.x86_64`; `rpm -V` clean | Nebula, Construct, `mackesd.target`, and all six groups active |
| Basement seat 15 | `172.20.0.15` | 44 | `magic-mesh-12.1.6-32.x86_64`; `rpm -V` clean | Nebula, Construct, `mackesd.target`, and all six groups active |
| Surface (`SURFACE`) | `172.20.146.79` | 44 | `magic-mesh-12.1.6-32.x86_64`; `rpm -V` clean | Nebula, Construct, `mackesd.target`, and all six groups active |

The installed hashes agreed on all three seats:

```text
a7110d73c6c935a209ca7cb692c42d3cecc4fedb9f5bc8e406ccd872878af75b  /usr/bin/mackesd
0edcb3ab5ab87fe5fd8c5fc7b09efaba2a45d330a566eca31b7232a1e1b8f189  /usr/bin/mde-shell-egui
```

Surface had no failed units. Dell and seat 15 each retained only the unrelated
`fwupd-refresh.service` failure; no core Magic Mesh unit failed. Surface root
key access was proved directly with `root@172.20.146.79` returning UID 0, so
the previous `mm` sudo-password limitation no longer blocks governed work.

Dell's pre/post Browser VM identity remained
`a1100a2f-5b65-4064-ac9f-925e1affa1fb`, running, persistent, and
autostart-enabled. Its exact disks remained
`browser-vm-r13-af3348bc-overlay.qcow2` and
`browser-vm-r13-control-seed/seed.iso`.

## Process isolation and farm restoration

On Dell, a bounded attempt to start a second control-group owner exited 1 with
`SQLite writer socket /run/mackesd/store-writer.sock is already live`; the
installed `mackesd-control.service` remained active. This is live kernel/process
ownership evidence, not merely a fixture.

After artifact preservation, `.131` had no Cargo, rustc, or xcp-build job. The
F44 VM was halted, the ordinary BigBoy VM `mcnf-build-52` was restored, and
`.130` returned online reporting Fedora 42. The disposable detached release
worktree and both temporary staged RPMs on the three seats were removed; the
signed artifact directories remain recoverable.

## Honest boundary

This checkpoint proves a signed native-F44 package cut, package integrity,
corrected-forward installation, grouped runtime health, Dell VM preservation,
and duplicate-owner refusal on exactly three physical seats. It does not claim
the full CRIT-006 promotion matrix, lighthouse release-32 deployment, reboot or
sleep recovery, GUI capture, audio acceptance, or live RDP-only/CF_HTML
interoperability. Those rows remain open. The later shell CF_HTML integration
commit `5ac104db` is not part of this exact release-32 artifact.
