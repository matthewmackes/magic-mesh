# Decision Log (ADR-style, append-only)

Every change to a governance lock (`AI_GOVERNANCE.md` §0–§10) requires an entry here:
the **symptom** that justified reopening the lock, the **superseding decision**, and the
**date**. Newest wins (§10). Append only — never edit or delete a prior entry; supersede
it with a newer one.

---

## ADR-0002 — Visual-confirmation gate re-instated as automated pixel-diff (2026-06-21)

- **Supersedes:** §7 "Visual-confirmation gate lifted" (2026-06-11 operator directive).
- **Symptom:** The 2026-06-11 lift was explicitly conditioned on visual confirmation
  being *hardware-gated and manual* (`/preview` could not run on a headless host, so it
  could not be a blocker). With the §10 move to **self-hosted CI on the XCP-ng fleet**,
  deterministic headless capture of the preview gallery is now feasible — so UI
  regressions that currently land undetected can be gated automatically without holding a
  feature on hardware.
- **Decision:** Re-instate a visual gate as **§10 B8** — an **automated pixel-diff visual
  regression** on the headless preview gallery, made deterministic via pinned fonts + a
  software rasterizer + fixed resolution. Golden references are **re-blessed in the same
  commit** as an intentional UI change (diff visible in git). Manual `/preview` is *not*
  restored as a blocker; the token discipline (§4) + palette tests remain underneath.
- **Scope:** GUI crates (`mde-*` surfaces). Phases in with the infra epic (the gate
  activates once self-hosted CI + the capture harness land).

## ADR-0003 — EFF-18 reopened + coverage model changed (2026-06-21)

- **Supersedes:** EFF-18 "keep the serial mackesd suite" (WON'T-DO, 2026-06-12) and
  EFF-31 "whole-denominator 80% hard floor" (2026-06-12).
- **Symptom:** §10 introduces a **blocking pre-commit hook that runs the FULL gate on
  every commit** (B-phase). That condition did not exist when EFF-18 was closed — the
  serial `--test-threads=1` suite is now the per-commit bottleneck, not just a CI-speed
  nicety. Separately, a whole-repo floor lets new code under-test as long as the legacy
  average holds.
- **Decision:** Reopen EFF-18 → **fix the env-race at the root** (inject config instead of
  `std::env::set_var`) and restore parallel mackesd tests (§10 B4; tracked as PROCESS-3).
  Change coverage to a **90% floor on changed lines** (diff-coverage), not whole-repo
  (§10 B7; PROCESS-4).
- **Scope:** Test/CI process. Activates with the PROCESS epic.

## ADR-0001 — §10 Process Locks established; agents may edit governance (2026-06-21)

- **Supersedes:** the implicit prior norm that process lived scattered across
  `CONTRIBUTING.md`, `ci.yml`, and the skills, and that `AI_GOVERNANCE.md` was edited
  ad hoc.
- **Symptom:** Effort + tokens were repeatedly spent solving problems that didn't need
  solving — speculative work, re-litigated decisions, reactive (catch-after-the-fact)
  remediation — because the design→build→verify→remediate process was neither
  single-sourced nor enforceable.
- **Decision:** Add **§10 — Process Locks** to `AI_GOVERNANCE.md`: numbered binary gates
  (D/B/V/R) with a single executable definition (`verify-gates.sh`), a tracked +
  symptom-backed entry filter, and a fix-forward / no-detour remediation loop. The
  **agent is source of truth** for design forks and **may edit `AI_GOVERNANCE.md`**, but
  every § change requires an entry in this log under the reopen rule (new symptom + dated
  superseding decision). Full lock map: `docs/design/process-governance.md`.
- **Scope:** Whole platform. Active immediately; infra-dependent gates phase in as the
  first §10 epic (the DevOps substrate) lands.
