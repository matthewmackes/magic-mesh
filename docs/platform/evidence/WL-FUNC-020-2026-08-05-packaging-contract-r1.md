# WL-FUNC-020 — Android/Cuttlefish packaging contract wiring (2026-08-05)

The Android packaging path now has one executable contract entrypoint that
checks the immutable nine-app manifest verifier, the nested-host tool-readiness
receipt, and the Cuttlefish placement-readiness verifier. Readiness self-tests
therefore exercise the real manifest/provenance path and reject incomplete
package sets and image-digest mismatches instead of relying on a parallel
permissive parser.

## Verification

- Farm `.90`, slot `wl-func020-cuttlefish-contract-r1`: packaging contract
  self-tests passed.
- Farm ended at `0/9` heavy slots active with `4/4` nodes up.
- This is packaging/provenance readiness only; no booted Cuttlefish guest,
  installed package, display, input, audio, or app-launch proof is claimed.
