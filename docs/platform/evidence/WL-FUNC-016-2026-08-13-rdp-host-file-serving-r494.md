# WL-FUNC-016 — governed RDP host-file serving (r494)

Date: 2026-08-13

## Implemented boundary

The live RDP adapter now completes the host-to-guest delayed-rendering path for
a permission-approved Files-backed image offered as a native file. The shell
continues to obtain bytes from the existing root-local Files materializer only
after the one-use clipboard permission transition; no host path or second file
store crosses into the RDP crate. The guest sees a deterministic digest-derived
file name and native `FileGroupDescriptorW` metadata.

The CLIPRDR backend now explicitly negotiates file streaming, path suppression,
and lock snapshots. It refuses serving unless streaming and locking were both
negotiated, caps the retained payload at 32 MiB, caps every range response at
256 KiB, binds SIZE/RANGE requests to file index, stream lock, offset, and the
advertised generation, and limits queued replies. Replacement preserves only
an explicitly locked snapshot. Unlock, rejected format advertisement, the
60-second serving deadline, and connection teardown revoke retained authority.

The implementation reuses the existing daemon Files descriptor authority in
`workers/transfers/clipboard_materializer.rs` and the existing shell permission
controller. `ipc/files.rs` remains the sole guest-to-host Files ingest authority;
no parallel storage or path-based serving endpoint was introduced.

## Farm evidence

- `.130`, slot `func016-host-file-rdp-final-r494`: focused native file-serving
  regression passed 1/1, covering negotiated admission, SIZE, bounded RANGE,
  locked replacement, and unlock cancellation.
- `.90`, slot `func016-host-file-authority-r494`: focused daemon Files
  descriptor regression passed 1/1, proving exact-command validation,
  digest/length re-attestation, one read-only descriptor, and replay refusal.
- `.196`, slot `func016-host-file-shell-r494`: focused shell metadata/path
  boundary regression passed 1/1, proving the native offer uses governed
  digest-derived metadata rather than disclosing a host path.
- `.170`, slot `func016-host-file-clippy-final-r494`: strict RDP library clippy
  passed with `-D warnings`.
- `.170`, slot `func016-host-file-shell-clippy-r494`: strict live-VDI shell
  binary clippy passed with `-D warnings`.
- `.90`, slot `func016-host-file-authority-r494`: strict daemon library clippy
  with `async-services` passed with `-D warnings`.
- `.90`, slot `func016-host-file-fmt-final-r494`: touched-file rustfmt passed
  for all four authorized source paths after the strengthened unbound-request
  assertion. The earlier `.196` check correctly failed on mechanical drift;
  the files were formatted before this final green check.

## Remaining acceptance

Post-release live Windows proof remains deferred by operator direction. It must
exercise Explorer paste, replacement during an active locked transfer, explicit
cancellation, expiry, and reconnect while confirming bounded memory and no
host-path disclosure. Non-image arbitrary Files objects still require a shared
typed descriptor request contract in the canonical Files materializer; this
slice does not bypass that authority or claim unsupported MIME coverage.
