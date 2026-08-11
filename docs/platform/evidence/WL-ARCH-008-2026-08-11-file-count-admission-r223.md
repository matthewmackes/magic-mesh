# WL-ARCH-008 file-count admission — 2026-08-11

- Scope: portable Browser profile migration.
- Change: source traversal now carries the remaining `MAX_FILES` budget into the iterator and refuses the next entry before retaining more candidates.
- Focused gate: `python3 install-helpers/migrate-browser-profile.py --self-test` after farm sync on `.50` slot `arch008-early-file-cap-r223`.
- Result: PASS — normal migration remains deterministic and an oversized source is refused with the bounded file-count error.
- Local confirmation: same self-test and `git diff --check` passed.
