# WL-FUNC-021 — cross-seat CPU and process-provenance verifier (2026-08-06)

## Scope

This slice audits and strengthens the approved live-seat CPU/package verifier
only. It does not install packages, restart services, change playback, stop a
provider, or otherwise mutate a seat. Rust source and the active worklist were
not edited.

`install-helpers/verify-music-cpu-proof.sh` now accepts
`MUSIC_CPU_PROOF_HOSTS=host-a,host-b` while retaining the existing
`MUSIC_LIVE_HOST` single-seat default. Every host is checked independently and
the final status reports the number of seats only when all requested checks
pass. A refusal on one seat does not hide a threshold failure on another.

For each eligible seat the read-only proof now records and checks:

- exact `magic-mesh` version, RPM release, architecture, and RPM install epoch;
- the `mackesd` executable path from `/proc/<MainPID>/exe`, or the exact
  systemd `ExecStart` path when root-owned procfs hides the symlink from the
  SSH user;
- RPM ownership and the installed RPM file digest for `/usr/bin/mackesd`;
- process-start epoch derived from `/proc/<MainPID>/stat` and `/proc/stat`,
  refusing a daemon that predates the installed RPM;
- stable `MainPID` and `NRestarts` across the bounded CPU sample window.

The systemd fallback is deliberately narrow: deleted, alternate, or
unavailable executable paths still refuse. It makes the proof usable on
root-owned system services without weakening the stale-process boundary that
previously allowed a release-4 process to be mistaken for release 5.

## Verification

The final helper was checked with:

```text
install-helpers/verify-music-cpu-proof.sh --self-test
verify-music-cpu-proof: self-test passed (no SSH attempted)

bash -n install-helpers/verify-music-cpu-proof.sh — passed
shellcheck install-helpers/verify-music-cpu-proof.sh — passed locally
git diff --check -- install-helpers/verify-music-cpu-proof.sh — passed
```

Farm `.90`, slot `music-cpu-proof-syntax-r2`, received the final helper and
passed both `bash -n` and the no-SSH self-test. The farm image does not contain
ShellCheck (`shellcheck: command not found`), so no farm ShellCheck pass is
claimed.

## Bounded two-seat read-only observation

Command shape:

```text
MUSIC_CPU_PROOF_HOSTS=172.20.146.225,172.20.0.15
MUSIC_CPU_PROOF_OBSERVE_SECONDS=10
MUSIC_CPU_PROOF_SAMPLE_INTERVAL_SECONDS=2
MUSIC_CPU_PROOF_SSH_TIMEOUT_SECONDS=45
./install-helpers/verify-music-cpu-proof.sh
```

Results:

- Dell `172.20.146.225` matched `magic-mesh-12.1.6-5.x86_64`. Its root-owned
  process path was proven through systemd `ExecStart` because user `mm` cannot
  read the procfs executable symlink; RPM ownership and digest checks passed.
  The process start epoch (`1786064400`) was after package install epoch
  (`1786064190`), and the five samples held `NRestarts=0→0` and PID
  `1993170`. CPU was **max 1106‰, mean 1096‰**, exceeding the declared
  `850‰`/`500‰` thresholds, so this seat correctly returned a threshold
  failure.
- Seat 15 `172.20.0.15` refused before sampling because it still has
  `magic-mesh-12.1.6-4.x86_64` while the checked-out RPM identity is release 5.
  No installation or other mutation was attempted.
- The aggregate command returned refusal/failure rather than a false pass.

This observation does not claim that the CPU-spike source mitigation is
complete. It is stronger provenance evidence: the current Dell process was
verified as the release-5 daemon before the high CPU result was reported, and
the second seat remained explicitly unproven instead of being conflated with
the Dell result. The cross-seat proof remains incomplete until seat 15 has an
authorized release-5 installation and both seats pass the bounded CPU gate.

## Artifact identity

```text
sha256  install-helpers/verify-music-cpu-proof.sh
        61b7295da738984ccf8602b967349e099acd1e9b99e55b542329e50dd3295c42
```
