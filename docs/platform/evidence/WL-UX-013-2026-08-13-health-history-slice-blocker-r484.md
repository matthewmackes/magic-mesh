# WL-UX-013 health-history/model slice audit — 2026-08-13

## Scope

The requested slice was limited to one bounded health-history/model/provider
file under `crates/desktop/mde-shell-egui` or the shared health module, plus
this evidence file. Music, `mackesd` broad paths, the worklist, and unrelated
agent files were excluded.

## Audit result

No safe substantive implementation gap remains in that scope. The current
health authority already provides the relevant bounded behavior:

- `fold_snapshot_with_availability` distinguishes expected absence, missed
  return, critical missed return, and unknown publisher absence without
  fabricating a fresh node observation.
- `SystemMeshHealthSnapshot` bounds freshness to admitted source expiry and
  the publication TTL.
- `health_modal` keeps active conditions separate from resolved history,
  filters before recurrence aggregation, applies the 24-hour window, and
  hard-bounds the materialized history page to eight rows.
- Existing focused evidence covers expected-state transitions, recurrence,
  history windows/severity, active-condition continuity, privacy, and capacity
  bounds (`WL-UX-013-2026-08-11-health-history-capacity-r460.md` and the
  related evidence cited by `docs/platform/WORKLIST.md`).

Adding another local fold, history query, or capacity constant would duplicate
the established authority or change semantics without a missing acceptance
case to justify it.

## External/live blocker

The remaining acceptance is outside this bounded code scope: S5 requires
render/package and direct transition captures across boot, sleep, network,
maintenance, outage, and rejoin on the approved physical seats/lighthouses,
plus provider-fed live evidence. The current farm has all five build VMs up,
but no live transition fixture or provider authority is supplied by this slice.
This is an external/live-proof blocker, not a failing code gate.

No source file was changed and no filler cargo gate was run. Unrelated working
tree state was preserved.
