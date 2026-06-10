---
name: release
description: >-
  Cut a Magic Mesh RPM (one spec, install-time role chooser): pre-flight gates
  incl. the DISCLAIMER.md gate, version check, build/test/lint gates, then
  commit/tag. TRIGGER ONLY when the operator explicitly types "cut release" /
  "build the RPM" / "release it" for this repo. NEVER auto-trigger from a /ship
  run — releasing is always operator-gated, and the package is HELD until every
  feature is §7-complete.
---

# release — RPM cut (Magic Mesh)

Operator-triggered only. Pushing tags and publishing are outward-facing — confirm
before anything leaves the machine. Push to a **single** remote `origin`, branch
`master` (`git push origin master` / `git push origin <tag>`); there is no
dual-remote. The rulebook is the root `AI_GOVERNANCE.md` (no `CLAUDE.md` in this
repo).

> **Packaging is not yet wired.** There is no RPM spec and no
> `[package.metadata.generate-rpm]` in any `Cargo.toml` today. The intended target
> (`AI_GOVERNANCE.md` §5) is **ONE RPM with an install-time deployment-role chooser**
> — Lighthouse ⊂ Server ⊂ Workstation, each a strict superset — plus a signed COPR
> and a Magic-on-Cosmic ISO. Until the spec/metadata land, a real cut is blocked;
> the build steps below describe the target mechanism, not something runnable today.

> **The package is HELD.** Per §5/§7, it does not cut until **every feature is
> §7-complete** (runtime-reachable, no stubs). If the operator asks for a cut before
> that gate, surface it and confirm a scoped "cut for testing" before proceeding.

## Pre-flight gates (all must hold)

1. **DISCLAIMER gate.** `DISCLAIMER.md` (repo root) **must exist and be non-empty**
   before any RPM build. No disclaimer → no RPM. (Hard pre-flight.) *Present today.*
2. Clean git tree on `master`; nothing un-committed that belongs in the cut.
3. `docs/WORKLIST.md` (the single durable tracker, created when execution begins)
   has no open `[ ]`/`[>]` blocking the release scope — or the operator explicitly
   scoped a partial "cut for testing". (If the worklist doesn't exist yet, the §7
   hold above already blocks a real cut.)
4. `cargo build --workspace --release` clean; `cargo test` green; `cargo clippy
   --all-targets` and `cargo fmt --all --check` clean (run from the repo root).
5. **Boundary gate.** `./install-helpers/lint-mesh-boundary.sh` clean — no mesh-side
   crate depends on a deleted desktop-shell crate (§6).
6. **Visual verification.** For any shipped iced/Cosmic GUI, confirm the render
   against the Carbon reference (see `/preview`) — launch the app binary
   (`cargo run -p mde-workbench` / `mde-files` / etc.) and inspect; don't trust a
   green `cargo test`.

## Steps

1. **Version — single-sourced, no per-crate bump.** The version is
   `[workspace.package] version = "10.0.0"` at the **repo-root `Cargo.toml`** (one
   version, all 20 crates inherit via `version.workspace = true`). Do NOT edit a
   per-crate `version` — they inherit. Bump the workspace version on shipped
   changes; if/when packaging metadata exists, bump only the packaging `release`
   field for asset-only changes so `dnf upgrade` sees a newer NEVRA.
2. **Update** release notes if present.
3. **Stage assets** (once the packaging layout is defined): bundle only what §5/the
   locked asset decision permits; verify any `NOTICE.md` / license coverage before a
   public RPM. *No asset-staging script exists in the repo yet.*
4. **Build** (target mechanism, once metadata lands): `cargo generate-rpm` is the
   intended mechanism — never raw `rpmbuild`. The layout is **ONE spec** with an
   install-time deployment-role chooser (Lighthouse ⊂ Server ⊂ Workstation).
5. **Smoke test:** install in a throwaway env and confirm the role chooser deploys
   the right workers/surfaces for each role.
6. **Commit** the version bump (named pathspecs, `Co-Authored-By` trailer).
   **Tag + push only after explicit operator go-ahead** (committing and pushing are
   separate authorizations). Release tag: **`magic-mesh-v10.0.0`**.

## Failure modes

`cargo generate-rpm` missing → `cargo install cargo-generate-rpm`. Empty/missing
`DISCLAIMER.md` → the build is gated; do not proceed. No RPM spec / packaging
metadata present → packaging is not yet wired; a real cut is blocked until it lands
(flag it, don't fabricate a build).

> The live skill set is exactly five: plan, ship, release, audit, preview.
> `release` is operator-gated and is never auto-triggered from a `/ship` run.
