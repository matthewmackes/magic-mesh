# WL-FUNC-016 RDP image lease-bound Files/CAS descriptor evidence — 2026-08-13

- Scope: production guest-to-host RDP image ingestion in
  `mde-shell-egui::vdi` and the root-local `mackesd` Files authority.
- Result: the daemon now returns one typed staged descriptor containing the
  canonical SHA-256, byte count, Files reference, MIME, session, lease
  generation, lease ID, and lease expiry. The shell rejects any descriptor
  that does not exactly match the admitted image and current lease.
- Commit ordering: `Begin` binds the transaction to the current lease;
  bounded chunks remain private staging; permission is decided; `Commit`
  must re-present the exact session/generation/lease. Changed or expired
  authority removes staging and fails before CAS installation or Files
  projection publication.
- Fail-closed behavior: malformed metadata, out-of-order chunks, descriptor
  substitution, stale generations, expired leases, non-root endpoints, and
  digest/length disagreement cannot fabricate a Files identity.

## Farm gates

- `.170`, slot `func016-rdp-cas-tests`:
  `cargo test -p mackesd --features async-services guest_clipboard_ -- --nocapture`
  passed 3/3 exact Files/CAS transaction tests (4,995 filtered) after a cold
  15m19s compile.
- `.90`, slot `func016-rdp-cas-rustfmt`: direct farm `rustfmt --check
  --edition 2021` over the two owned production modules passed.
- Scoped `git diff --check` passed.

## Deferred gate debt

- The broader `mde-shell-egui --features live-vdi --all-targets` BigBoy build
  was stopped at operator direction once it was clear it duplicated broader
  coverage and was still in a cold dependency build. The exact shell test and
  broad Clippy remain deferred to the post-first-build acceptance wave.
- Live Windows guest proof remains post-release and non-blocking under the
  current release directive.
