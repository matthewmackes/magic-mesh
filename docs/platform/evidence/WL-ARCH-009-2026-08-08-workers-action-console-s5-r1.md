# WL-ARCH-009 Workers Action Console S5 slice — 2026-08-08

`Surface::Workers` now exposes a bounded Action Console backed by live worker
contracts and runtime generations. Operators select only advertised typed
actions, publish Preview before Commit or Cancel, and see typed audit and
per-item partial-failure results. A generation change invalidates a staged
preview before commit.

The request contract now defines canonical node-scoped action/result topics, a
stable intent digest, and a bounded 4 KiB short-lived capability field minted by
the existing root shell action authority. No raw command or arbitrary path is
accepted.

## Verification

- `.50`, explicit farm slot: Action Console model/publication tests passed 3/3.
- `.50`, explicit farm slot: final contract and authentication test passed 1/1.
- Scoped rustfmt and `git diff --check` passed.
- The broader shell rerun reached concurrent Android fixture construction
  errors before this slice's tests; those fixtures are owned by the Android UI
  tranche and are not counted as Action Console evidence.
- No operational tests were removed.

## Remaining acceptance gap

S5 remains open until an installed live daemon round trip proves
Preview/Commit/Cancel and audit projection, and wide, narrow, and largest-text
captures pass. Six-service isolation, legacy route removal, and fleet
convergence also remain for the parent epic.
