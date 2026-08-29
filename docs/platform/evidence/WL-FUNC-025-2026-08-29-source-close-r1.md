# WL-FUNC-025 source close — 2026-08-29

Classification: source/cargo close. Live Construct menubar click and
lock-11 mesh-mounted tree remain `WL-TEST-003` after a testing Beta.

Source revision: `5f9685408` on `agent/drain-worklist-20260725`.
`production_admitted: false`. No dest invented. No seat mutation.

## Why this closes

S1–S3 are in-tree: New File, Duplicate, Compress, Extract Here/To,
Symlink, and Hard Link are reachable from the Files menubar and context
menu and execute through the existing FileOps/OpQueue engine with
exists, read-only, traversal, destination-symlink, cross-device, and
mesh-escape refusal. Dest-cut `bc14a22d7` matches that wiring.

Live leftovers already transferred to `WL-TEST-003` S3 (operator
2026-08-27). Fixtures do not satisfy that leftover.

## Farm (current HEAD, not re-run)

Reuse of the already-fresh focused gates at `5f9685408`:

| command | job | node | ended | result |
|---|---|---|---|---|
| `cargo test -p mde-files-egui` | `143b09b89c4d` | `.50` d1 | 2026-08-29T00:52:09Z | 223 passed, 0 failed |
| `cargo test -p mde-files` | `edd4ab8d84fb` | (same HEAD) | 2026-08-29T00:52:27Z | 159 + 5 passed, 0 failed |

Prior surface/fixture notes remain factual and do not reopen source:

- `WL-FUNC-025-2026-08-20-files-posix-closure-r2.md`
- `WL-FUNC-025-2026-08-23-mesh-tree-archive-queue-r1.md`
- `WL-FUNC-025-2026-08-26-surface-files-posix-r1.md`
