# WL-FUNC-021 Jellyfin zero-byte cache refusal — 2026-08-06

Offline cache availability now requires a regular, non-empty file whose size
matches the retained manifest. Empty, truncated, missing, path-escaping, or
symlinked entries cannot become playable fallback media, and atomic writes do
not leave temporary files behind.

Verification:

- BigBoy `.130`, slot `func021-jellyfin-outage-zero-byte-20260806-r1`:
  `cargo test -p mde-jellyfin --test outage -- --nocapture` passed **2/2**.
- `git diff --check` passed.
- Source SHA-256: `cache.rs`
  `3fac744dd865ab8f4e0014db01cfd542e8ff9b6058c841e73b58f14e312073d`,
  `outage.rs`
  `1b61ff3393910956cadab34ee2745cffcba80987667d7c704aa2296f54bd590a`.

Live provider outage, reconnect, decoder, rendered Media, package, and seat
proof remain open. Dell runtime was not modified.
