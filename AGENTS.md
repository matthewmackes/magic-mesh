# Codex Project Instructions

This repository is a Codex-owned MCNF project. The durable rulebook is
`AI_GOVERNANCE.md`; when it conflicts with older prose, follow the newer
governance lock and update stale docs as part of the work.

## Operating Rules

- Use `rg`/`rg --files` first for repository searches.
- Preserve user changes. Do not revert unrelated dirty files.
- Remove abandoned agent worktrees after their work is merged or salvaged.
- Do not recreate `.claude`; that surface is retired. Store any one-off salvage
  outside the repo, preferably under `/tmp`, and document it in the handoff.
- **All AI agents must use the build farm for build/test/gate work** unless the
  command is only a tiny local syntax/probe check. Parallelize independent gates
  across the farm with explicit `MCNF_BUILD_HOST` and `MCNF_BUILD_SLOT`, put the
  longest job on BigBoy, avoid containers when direct farm-host fixtures work,
  and treat farm/test hosts as safe for destructive reboot/recovery unless a task
  explicitly says otherwise. See `AI_GOVERNANCE.md §10` and
  `docs/BUILD-ENVIRONMENT.md`.

## Build And Test

- Prefer the build farm for heavy work: `install-helpers/xcp-build.sh`.
- The current farm inventory lives in `docs/BUILD-ENVIRONMENT.md` and
  `install-helpers/farm.sh`; keep scripts and docs in sync.
- Local builds on the Rocky dev host need the gold linker override:
  `RUSTFLAGS="-C link-arg=-fuse-ld=gold"`.
- GUI/runtime claims need either farm verification or an explicit note that the
  live hardware was unavailable.

## Cleanup Doctrine

- Delete dead workflow glue instead of carrying compatibility shims for retired
  agent systems.
- Keep historical design notes only when they still explain a live behavior.
- Runtime code must remain reachable, tested, and free of stubs per
  `AI_GOVERNANCE.md §7`.
