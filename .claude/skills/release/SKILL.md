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

> **Packaging IS wired (EFF-41 update, 2026-06-12).** The
> `[package.metadata.generate-rpm]` block lives in `crates/mesh/mackesd/Cargo.toml`
> — ONE `magic-mesh` RPM (`AI_GOVERNANCE.md` §5) whose assets bundle every workspace
> binary (incl. the auto-discovered `mde-mesh-wallpaper`), the systemd units,
> `.desktop` launchers/autostarts (role-chooser first-run, cosmic-applet), icons,
> DISCLAIMER/LICENSE/NOTICE/SUPPORT, and `docs/help/`. Scriptlets (post_install /
> pre_uninstall / post_uninstall) are in the same block. The role split stays
> install-time (Lighthouse ⊂ Server ⊂ Workstation via `mackesd role pin` + the
> first-run chooser GUI). **Signing (EFF-17/EFF-30):** the public key is
> committed (`packaging/repo/RPM-GPG-KEY-magic-mesh`); sign at cut time with
> `./install-helpers/sign-release.sh <rpm> [iso…]` — rpmsign-embeds the RPM
> signature and emits `SHA256SUMS` + a detached `.asc` (run on the operator's
> machine holding the "Magic Mesh Release Signing" secret key). **Still
> operator-gated/open:** the signed COPR and the Magic-on-Cosmic ISO build.

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
6. **Visual verification — gate lifted (2026-06-11, operator directive).** A shipped
   iced/Cosmic GUI no longer needs operator/on-session (`/preview`) visual confirmation
   to release; render correctness rests on the `mde-theme` Carbon tokens (§4) + tests
   (§7). `/preview` stays available but is optional/best-effort and never holds the cut.

## Steps

1. **Version — single-sourced, no per-crate bump.** The version is
   `[workspace.package] version` at the **repo-root `Cargo.toml`** (one version,
   all 22 crates inherit via `version.workspace = true`). Do NOT edit a
   per-crate `version` — they inherit. Bump the workspace version on shipped
   changes; for asset-only changes bump only a packaging `release` field (add it
   to the generate-rpm block if absent — EFF-40) so `dnf upgrade` sees a newer
   NEVRA.
2. **Update** release notes if present.
3. **Stage assets:** the asset list IS the `[package.metadata.generate-rpm]`
   `assets` array in `crates/mesh/mackesd/Cargo.toml` — verify every `source`
   path exists after the release build (`cargo build --workspace --release`)
   and that LICENSE/NOTICE coverage is current before a public RPM.
4. **Build:** `cargo build --workspace --release` then
   `cargo generate-rpm -p crates/mesh/mackesd` — never raw `rpmbuild`. ONE
   `magic-mesh` RPM; role selection happens at install/first-run, not via
   per-role packages.
5. **Smoke test:** install in a throwaway env and confirm the role chooser deploys
   the right workers/surfaces for each role.
6. **Commit** the version bump (named pathspecs, `Co-Authored-By` trailer).
   **Tag + push only after explicit operator go-ahead** (committing and pushing are
   separate authorizations). Release tag: **`magic-mesh-v10.0.0`**.

## Failure modes

`cargo generate-rpm` missing → `cargo install cargo-generate-rpm`. Empty/missing
`DISCLAIMER.md` → the build is gated; do not proceed. A generate-rpm asset whose
`source` is missing after the release build → fix the asset list or the bin
target before cutting (don't fabricate a build). GPG signing without the
committed key (EFF-17) → the unsigned-RPM cut may proceed only as an explicitly
scoped "cut for testing"; a public release stays blocked on the key.

> The live skill set is exactly five: plan, ship, release, audit, preview.
> `release` is operator-gated and is never auto-triggered from a `/ship` run.
