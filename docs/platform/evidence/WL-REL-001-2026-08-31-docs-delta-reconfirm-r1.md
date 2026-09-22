# WL-REL-001 dest-cut reconfirm — docs-only HEAD delta — 2026-08-31

Classification: dest-cut identity reconfirm. Not freeze. Not
`production_admitted`. No dest rebound. No dest invented.

Dest-cut: `42035dcbd76b03b8323399892052b21a96e2e233` epoch `1788153988`.
S7 `release-input-preflight` passed at that revision
(`WL-REL-007-2026-08-31-s7-preflight-42035dcbd-r1.md`). Private argv
`/root/mcnf-private/release-preflight-42035dcbd.json` stays bound to
those bytes.

## Delta from dest-cut to then-HEAD `fcf199c6e`

```
fcf199c6e docs: draft magic-mesh-v13.0.0 release notes without declaring freeze
969f12eb8 docs: reconfirm S7 preflight at HEAD 42035dcbd
```

Paths: `CHANGELOG.md`, `docs/platform/WORKLIST.md`,
`docs/platform/evidence/WL-REL-001-2026-08-31-s4-notes-draft-r1.md`,
`docs/platform/evidence/WL-REL-007-2026-08-31-s7-preflight-42035dcbd-r1.md`,
`docs/releases/magic-mesh-v13.0.0.md`.

No crate, packaging, helper, or Cargo change. A non-docs helper would
invalidate dest-cut receipts and force rebind; it was not landed.

## Freeze leftover

Promote the dest-cut SHA on the protected default branch. Do not rebind
already-selected dests for documentation. Surface `bootc_base` stays
null. Do not grind `cargo test --workspace`.
