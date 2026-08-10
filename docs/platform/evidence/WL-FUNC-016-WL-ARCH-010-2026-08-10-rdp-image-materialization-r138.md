# WL-FUNC-016 / WL-ARCH-010 — bounded Files-backed RDP image materialization (r138)

Date: 2026-08-10

Base revision: `630c0407805f`

## Result

The production host-to-guest RDP clipboard path now carries PNG and JPEG
offers through the existing typed clipboard lease and one-use permission gate.
The shell does not resolve a Files identity or read image bytes until the gate
reaches `Materialize`. It then asks the root-local Transfers authority for one
descriptor bound to the exact live lease, command, Files generation, byte
count, and SHA-256 digest.

The authority exposes no path, opens the source read-only with no symlink
following, rechecks the current Files record, hashes the opened inode, and
releases the descriptor once over a mode-0600 Unix `SEQPACKET` socket. Replay,
expiry, stale metadata, malformed identity, unsupported MIME, and payloads over
32 MiB fail closed. Silent root-local clients are nonblocking and expire after
one second, so they cannot pin the Transfers worker tick.

The shell verifies peer credentials, response metadata, descriptor count,
regular-file type, size, and digest before bounded PNG/JPEG decode. It emits a
top-down 32-bit CF_DIBV5 offer; the IronRDP bridge validates geometry and
advertises CF_DIBV5 plus CF_DIB compatibility.

## Focused farm proof

- `.50` (`172.20.0.50`):
  `files_materialization_request_is_exact_bounded_and_lease_bound` — 1 passed.
- `.90` (`172.20.0.90`, `live-connect`):
  `clipboard::tests::bounded_dibv5_negotiation_round_trips_and_rejects_hostile_geometry`
  — 1 passed, 97 filtered.
- `.170` (machine 194 build VM, `live-vdi`):
  `vdi::tests::rdp_png_expands_only_into_a_bounded_top_down_dibv5` — 1 passed,
  1,573 filtered.
- `.196` (`async-services`):
  `workers::transfers::clipboard_materializer::tests::` — the exact descriptor,
  replay, silent-client, and expiry gates passed.

Focused rustfmt and `git diff --check` passed. The broad workspace rustfmt gate
is independently red from pre-existing formatting drift outside this slice and
is not counted as this checkpoint's proof.

Source SHA-256:

- `2daa423690d45b368464e93d0674d151900e4013de65a5fc5c17386a9aab3b37`
  — `crates/desktop/mde-shell-egui/src/vdi/mod.rs`
- `85a0194050c574f365e2f7ae8465d6b5df43a88f1ff30c41d42ef72928ee6670`
  — `crates/desktop/mde-shell-egui/src/vdi/tests.rs`
- `cfa70f12ac52ba781355620e96b465a325319200592722d9212a488952ce14be`
  — `crates/desktop/mde-vdi-rdp/src/clipboard.rs`
- `8b38036d2fe6bcafa3464e8f077732edd378e56c900214442d9f0a3990c3332f`
  — `crates/desktop/mde-vdi-rdp/src/connect.rs`
- `f43ff8d811fb6b3709ad0ea40e54177c1828cbd7add9bce82a63bb4816ab3a0e`
  — `crates/mesh/mackes-mesh-types/src/vdi_clipboard.rs`
- `b52cead8eef34f0a1a374bc76397b20e5e20e75788c236e0fc9604f7302c3f4d`
  — `crates/mesh/mackesd/src/workers/transfers/mod.rs`
- `0e07da459589b540f936688e583b11e18dfa92b430206d0e22c215e471b4abc6`
  — `crates/mesh/mackesd/src/workers/transfers/v2.rs`
- `e49ed71603337d252dd81cdce8a8278f7a1014b7ac5701519a631564fb94128e`
  — `crates/mesh/mackesd/src/workers/transfers/clipboard_materializer.rs`

## Remaining boundary

This checkpoint proves host-to-guest image authority and conversion in focused
farm fixtures. Guest-to-host images remain intentionally refused until a Files
write authority exists. File-list and typed-metadata guest transport, live
Windows interoperability, and physical-seat proof remain, so neither epic is
closed.
