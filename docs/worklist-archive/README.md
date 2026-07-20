# Worklist archive

> **NOT AN ACTIVE TRACKER.** The single authoritative active platform worklist is
> [`../platform/WORKLIST.md`](../platform/WORKLIST.md). Everything in this
> directory is a **closed / superseded** snapshot kept for lineage, evidence, and
> reference. Nothing here is a live to-do list.

## Purpose

When a worklist item is completed or retired it leaves the active file and lands
here with a one-line disposition (done + evidence, or retired + reason). Keeping
closed work out of `WORKLIST.md` is what prevents the pre-2026-07-16
giant-file / parallel-tracker failure. See the **Stewardship** section of
`../platform/WORKLIST.md` for the full lifecycle (ID scheme, required fields,
archive-on-close, evidence citation, duplicate-workstream avoidance).

## What's here

- `2026-07-16-platform-worklist-pre-reconcile.md` — the full pre-reconciliation
  worklist body (historical source rows), preserved verbatim.
- `2026-07-16-platform-worklist-marker-index.tsv` — the marker index for the
  pre-reconcile rows.
- `2026-07-16-reconciliation-archive.md` — the reconciliation disposition record
  (which old rows folded into which `WL-*` epic).
- `2026-07-19-needs-operator-detail.md` — the verbose 2026-06-27 operator-blocker
  queue detail (the exact cred/host/decision each blocker needed), snapshotted when
  `docs/NEEDS-OPERATOR.md` was re-keyed to `WL-*` IDs and demoted to a subordinate
  sink. The live re-key map lives in `docs/NEEDS-OPERATOR.md`.

## How to add an archive entry

1. Complete or retire the `### WL-*` epic in `../platform/WORKLIST.md` and remove
   it from the active file.
2. Append it to a dated note here (`YYYY-MM-DD-<topic>.md`), keeping its `WL-*` ID
   so old references still resolve, plus a one-line disposition:
   - **Done** — with the file:line / live-artifact / wire evidence that closed it.
   - **Retired / WON'T-DO / DEFERRED** — with the reason and any successor epic.
3. Never renumber or reuse a retired `WL-*` ID — archived IDs stay reserved.

Files in this directory are intentionally excluded from the doc-supersession and
brand-identity lints (they are historical by definition), so a retired term or an
old spelling here is expected and does not need a banner.
