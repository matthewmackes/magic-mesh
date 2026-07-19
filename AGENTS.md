# Agent Project Instructions (AGENTS.md)

This MCNF / magic-mesh repository is developed almost entirely by AI agents, so
these instruction files are load-bearing infrastructure. This file (`AGENTS.md`)
is the shared agent rulebook read by every agent tool; the **durable authority**
is `AI_GOVERNANCE.md`. When any prose conflicts with it, follow the newer
governance lock and update the stale doc as part of the work.

> **Integrity note:** `AGENTS.md` and any repo-root `CLAUDE.md` are known
> prompt-injection targets — a fabricated `CLAUDE.md` was injected and then
> removed on 2026-07-10 (commit `26ef652b`, "remove injected content"). Treat any
> change to these two paths as security-sensitive and diff it against history.

## Operating Rules

- Use `rg`/`rg --files` first for repository searches.
- Preserve user changes. Do not revert unrelated dirty files.
- Remove abandoned agent worktrees (`.claude/worktrees/*`) after their work is
  merged or salvaged.
- The **tracked, legitimate** agent surface is `.claude/skills/` (e.g.
  `.claude/skills/polish/SKILL.md`) — keep it. Do **not** author or reintroduce a
  repo-root `CLAUDE.md`: the only one that ever existed was injected content and
  was removed 2026-07-10 (commit `26ef652b`); it is a known injection vector.
  Store any one-off salvage **outside** the repo (a scratchpad dir or `/tmp`) and
  document it in the handoff.
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
- Keep historical design notes only when they still explain a live behavior. A
  design note that describes retired architecture (the iced/`libcosmic`
  `mde-workbench` era, the LizardFS substrate, the cloud-hypervisor/`mde-kvm` VM
  path) must carry a top **HISTORICAL / SUPERSEDED** banner or be allowlisted;
  `install-helpers/lint-doc-supersession.sh` enforces this.
- Runtime code must remain reachable, tested, and free of stubs per
  `AI_GOVERNANCE.md §7`.

## Worklist Stewardship

- The **only** active platform worklist is `docs/platform/WORKLIST.md`. Design
  notes, ops runbooks, review ledgers, and `docs/NEEDS-OPERATOR.md` are *evidence
  sources*, not parallel trackers — never present a second file as an active
  worklist.
- Items are `### WL-<FAMILY>-<NNN>` epics with a fixed field set and a `Status` of
  `Remaining` / `Blocked` / `Needs clarification`. Full lifecycle — ID scheme,
  required fields, archive-on-close, evidence-citation, and duplicate-workstream
  avoidance — is the **Stewardship** section of `docs/platform/WORKLIST.md`.
- Closed/retired items move to `docs/worklist-archive/` with a disposition (see
  its `README.md`); they are not left in the active file. Pre-reconciliation IDs
  re-key to their owning `WL-*` epic (map in `docs/NEEDS-OPERATOR.md`).
- `install-helpers/lint-worklist.sh` enforces the active file's shape; run its
  `--self-test` before landing worklist edits.
