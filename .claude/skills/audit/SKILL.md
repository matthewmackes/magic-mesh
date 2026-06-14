---
name: audit
description: >-
  Integrity sweep of the Magic Mesh Rust mesh workspace: find dead/unreachable
  code, stubs, mockups passing as features, convention violations (raw hex,
  scattered metrics, mesh/desktop boundary breaks), and stale docs — each
  finding gets a FINISH-or-REMOVE verdict. TRIGGER when the user asks to
  "audit", "evaluate compliance", "check for dead code/stubs", or "find what's
  not really done" in the workspace. Produces a findings table / report; it
  does NOT fix things unless asked.
---

# audit — compliance & integrity sweep (Magic Mesh)

Catches the gap between "marked done" and "actually reachable + correct", and
checks compliance with the root `AI_GOVERNANCE.md` (the operational rulebook — this
repo has **no `CLAUDE.md`**). Output is a findings **table**
(`Location | Category | Evidence | Confidence | Verdict`) plus a short summary;
verdict is binary **FINISH** (wire it up / make it real) or **REMOVE** (delete
the dead surface). Don't fix unless asked — report first.

## Passes (run in parallel where possible)

1. **Unreachable code** — `pub mod`/`mod` with no external `<mod>::` ref; `pub fn`
   never called; dead `match` arms; a feature with no path to it from a real
   entrypoint (an app binary — `magic-fleet`, `mde-files`, `mde-workbench`,
   `mde-voice-hud`, `mde-music`, `mde-musicd`, `mde-bus` — a `mackesd` worker, or
   an `mde-bus` subscription).
2. **Stubs** — `todo!()`, `unimplemented!()`, `panic!("not …")`, stub arms,
   `pub mod foo;` with zero refs, "wiring in a follow-up" commit bodies.
   This is the §7 Definition of Done line: code existing is never "done".
3. **Mockups** — `demo_data`/placeholder constants, "coming soon"/"placeholder"
   strings, panels/surfaces that render but do nothing.
4. **Convention violations** (`AI_GOVERNANCE.md`):
   - raw hex/RGB literal or scattered metric literal anywhere outside the
     `mde-theme` token modules (§4 — Carbon tokens single-sourced in
     `crates/shared/mde-theme`, lint-gated):
     `rg -n '#[0-9a-fA-F]{6}|from_rgb8?\(' crates/**/src` minus
     `crates/shared/mde-theme`;
   - a Carbon token / metric value changed without a matching `mde-theme` test
     assertion to back it (§4 — change a value only with a reference);
   - **mesh/desktop boundary break** (§6): a mesh-side crate depending on a
     deleted desktop-shell crate. Run `./install-helpers/lint-mesh-boundary.sh`;
     any hit is a finding.
   - **substrate locks** (§1–§3): non-Nebula transport (Tailscale/Headscale/DERP),
     Gluster instead of LizardFS, a new MDE-private D-Bus name (only FDO
     `org.freedesktop.*` interop is allowed, §2), or crypto below the pinned
     values (Ed25519 / AES-256-GCM / ChaCha20-Poly1305 / RSA-4096 KDC, §3).
5. **Doc drift** — prose claiming facts the code contradicts. Check prose against
   the *current* reality: the E11 "Magic Mesh" pivot — **Cosmic owns the desktop**,
   the GUI is **strictly IBM Carbon** (Gray 10 / 90 / 100, Gray 100 default dark),
   iced 0.14 + cosmic-text + rustls. Flag any prose still describing the labwc/Win-
   era desktop shell, a `mde <subcommand>` dispatcher, four era-themes
   (Win2000/Windows10/BeOS), Gluster, OpenSSL, or a `crates/shell/`-rooted path.
   Each stale claim is a FINISH (fix the doc).
6. **Packaging reachability** — once packaging is wired (§5: ONE RPM with the
   install-time deployment-role chooser, Lighthouse ⊂ Server ⊂ Workstation, plus a
   signed COPR and a Magic-on-Cosmic ISO), flag assets/symbols the package ships
   but nothing uses. Confirm the `DISCLAIMER.md` pre-flight gate exists + is
   non-empty. **Note:** packaging IS wired — `[package.metadata.generate-rpm]`
   lives in `crates/mesh/mackesd/Cargo.toml` (one `magic-mesh` RPM; assets =
   every workspace binary + `packaging/` units/launchers + docs). Audit asset
   source paths for existence and binaries against real bin targets (explicit
   `[[bin]]` OR auto-discovered `src/bin/*.rs`). The GPG key + COPR + ISO
   remain operator-gated.
7. **Unprovisioned infrastructure preconditions** (ONBOARD-6 lesson, 2026-06-14)
   — the gap §7 + the passes above CANNOT see: a feature whose code is fully
   reachable + unit-tested yet **silently no-ops in the mesh because its
   deployed substrate was never provisioned**. QNM-Shared hid for the whole
   project this way — the leader/directory/fleet code is real + reachable, but
   the tests bind `QNM_SHARED_ROOT` to a **tempdir**, and the code behaves
   identically against a local dir vs a real LizardFS mount, so nothing failed
   while the mesh showed NO LEADER / node_count 0. Flag any feature that reads a
   shared path / mount / external daemon (`default_qnm_shared_root`,
   `QNM_SHARED_ROOT`, a `*mount*`, a `Command::new("<daemon>")`) whose ONLY
   coverage is a single-instance tempdir/mock — it needs a **multi-node /
   real-substrate assertion** (cross-node visibility, leader contention) and a
   **fail-loud runtime check** that the substrate is real, else "works locally"
   masquerades as "works in the mesh". Guardrails:
   `./install-helpers/lint-shared-substrate.sh` (the watchdog mount-assert +
   the multi-node test must stay), `tests/mesh_shared_state.rs` (the gate),
   `install-helpers/setup-qnm-shared.sh` (the provisioning). §7 = necessary,
   not sufficient.

## Safeguards (avoid false positives)

Framework lifecycle callbacks (`iced` `update`/`view`/`subscription`, `Default`,
`Drop`, serde derives), `#[test]`/`#[cfg(test)]` helpers, and declaratively-wired
handlers are **reachable** even with no direct textual caller — don't flag them.
Confirm a "dead" symbol with `rg` across the whole workspace before the verdict.
All 22 crates are workspace members (none are excluded from the build), so a symbol
is not dead merely because a sibling crate is the only caller — check the whole
graph. `mackesd` is a **lib + bins** crate: the daemon binary lives at
`src/bin/mackesd.rs` (plus `meshctl`); workers are spawned from its `run_serve`.
**Cargo auto-discovers `src/bin/*.rs` as bin targets** — a binary is NOT missing
merely because no explicit `[[bin]]` block names it (AUD2-7 false-positive
lesson, 2026-06-12); confirm with `cargo build -p <crate> --bin <name>` before
flagging a packaging asset as unbuildable.

## Output

A markdown findings table + counts by category, written to `docs/COMPLIANCE.md`
(create `docs/` on first use), or returned inline for a quick check. Lift every
FINISH into `docs/WORKLIST.md` (the single durable tracker, created when execution
begins) so the sweep produces actionable work, not just a report.
