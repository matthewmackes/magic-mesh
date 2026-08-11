# WL-FUNC-016 RDP guest image admission evidence — 2026-08-11

- Scope: the production CLIPRDR client now negotiates guest CF_DIBV5
  preferentially, then CF_DIB, after existing Unicode/HTML paths.
- Boundary: each response is bound to the exact negotiated format and validated
  for byte cap, header, geometry, bitfields, and declared pixel size. Values use
  a private-construction typed wrapper, are consumed once, and stale replacement
  or unsolicited replay cannot replace an admitted value.
- Production truth: `RdpConnection` exposes the validated typed image and the
  live shell consumes it once. Because the current daemon socket only resolves
  existing Files references and cannot mint a governed descriptor/CAS identity,
  guest images currently produce visible non-fatal
  `FilesProviderUnavailable`; raw DIB bytes are dropped and never published,
  written through a guessed path, or assigned a fabricated Files identity.
- Intended farm command: `cargo test -p mde-vdi-rdp --features live-connect guest_ -- --nocapture` plus the shell's exact refusal regression.
- Result: **NOT RUN**. `.90` was unavailable and every reachable free slot was
  below the 8 GiB reserve. `git diff --check` passed.
- Remaining work: the guest-to-host image gap is not closed. It requires a
  daemon-owned bounded descriptor-ingest/CAS authority followed by the existing
  one-use permission and typed rich-message publication.
