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
- Result: **TARGET PASS / SUITE MIXED**. Farm `.90`, slot `func016-rdp-image`,
  ran the guest-image suite: 6 passed, 1 failed. The targeted
  `guest_dib_and_dibv5_are_admitted_as_typed_one_use_images` regression passed;
  the unrelated `bridge_bounds_host_text_and_decodes_guest_unicode` test
  failed. Farm `.90`, slot `func016-clippy`, ran
  `cargo clippy -p mde-vdi-rdp --features live-connect --lib` to completion with
  warnings only (97 warnings).
- Remaining work: the guest-to-host image gap is not closed. It requires a
  daemon-owned bounded descriptor-ingest/CAS authority followed by the existing
  one-use permission and typed rich-message publication. The unrelated host-text
  decoding failure remains a separate follow-up.
