# WL-CRIT-007 corrected-forward payload binding — 2026-08-13

- Scope: bind corrected-forward recovery evidence to the exact SHA-256 RPM
  payload identities authorized by the signed release, rather than trusting
  package names alone.
- Production change: preflight and post-reboot snapshots now record the
  installed package's RPM `PAYLOADDIGEST`, require algorithm 8/SHA-256, and
  reject null or malformed values. `verify-forward` requires exact previous
  and forward payload digests and rejects an unchanged payload, retained old
  payload, or substituted payload even when package names look correct.
- Hostile boundary: malformed signed digest authority, same-payload rollback,
  retained old bytes, and a substituted forward digest all fail closed.
- Files: `install-helpers/verify-corrected-forward-recovery.py` and this record.

## Farm gates

Capacity was checked immediately before use. Five of five farm nodes were
online. `.196` had its sole lane free and 4.8 GiB available. Because this was a
small script-only sync with no Cargo target or generated release artifacts, the
generic sync reserve was explicitly lowered to 1 GiB for this invocation only.

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=1 \
MCNF_BUILD_MIN_SYNC_FREE_KIB=1048576 \
  install-helpers/xcp-build.sh sync
ssh ... 172.20.0.196 \
  'cd /home/mm/magic-mesh-farm-1 && \
   install-helpers/verify-corrected-forward-recovery.py self-test && \
   python3 -m py_compile install-helpers/verify-corrected-forward-recovery.py'
```

Result: **PASS**, hostile self-test 19/19 and Python compilation.

```text
ssh ... 172.20.0.196 \
  'cd /home/mm/magic-mesh-farm-1 && \
   install-helpers/verify-rpm-payload.sh overlay-claims-package'
```

Result: **PASS**, all three RPM variant identity/recovery payload shapes and
runtime/activation/recovery contracts passed.

Local `git diff --check` is the tiny source-tree whitespace gate because farm
sync workspaces intentionally omit `.git`.

## Remaining acceptance

- Cut and sign the first complete seven-role release and feed its exact old/new
  workstation RPM payload digests into `verify-forward`.
- Execute one-node corrected-forward reboot/network-return evidence after that
  release; live acceptance remains deferred and non-blocking until then.
- Physical sleep/resume and any required lighthouse recovery rows remain
  post-release evidence, not pre-release coding blockers.
