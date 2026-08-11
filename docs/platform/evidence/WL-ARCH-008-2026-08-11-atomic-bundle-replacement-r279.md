# Browser migration atomic bundle replacement evidence — 2026-08-11

- Scope: `--replace` verifies that the existing output is an admitted migration
  bundle and binds replacement to its exact filesystem identity.
- Publication: Linux `renameat2(RENAME_EXCHANGE)` atomically swaps the staged
  and retained directories. Unsupported or failed exchange refuses replacement
  without deleting the last complete bundle; no non-atomic fallback exists.
- Hostile fixtures: unrelated output is preserved and refused; an injected
  publication failure retains the exact old manifest and payload, after which a
  normal corrected-forward replacement succeeds.
- Farm gate: BigBoy `.130`, slot 3: `migrate-browser-profile: self-test passed`.
- Python syntax check and scoped `git diff --check`: passed.
