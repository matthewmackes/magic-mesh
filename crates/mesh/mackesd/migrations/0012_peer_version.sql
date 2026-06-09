-- PEERVER-4 (v2.7, 2026-05-29) — per-peer mde-core RPM version.
--
-- Strictly additive per the migration policy: one nullable column on
-- `nodes`, populated by the health-reconciler tick mirroring the
-- GFS-replicated peer-files (docs/design/v2.7-peer-data-convergence.md).
-- NULL until a peer's `<hostname>.json` record has been seen. The
-- installer tools (mde-update / mde-install) read the peer-files
-- directly; this column is the nodes-table cache for mackesd's own
-- consumers (Workbench mesh view, health).

ALTER TABLE nodes ADD COLUMN mde_version TEXT;
