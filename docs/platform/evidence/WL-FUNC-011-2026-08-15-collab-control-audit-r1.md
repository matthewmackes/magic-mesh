# WL-FUNC-011 collaboration control audit r1

Date: 2026-08-15

This audit reconciles the stale blocker wording in the active worklist against
the production implementation. It does not claim live provider or package
proof.

## Controls confirmed in source

- `crates/mesh/mackesd/src/workers/transfers/mod.rs` registers the typed V2
  executor families: Mesh, Rsync, Sftp, Http, Scrape, Multipart, Recurring,
  and Clipboard, in addition to Local.
- `crates/mesh/mackesd/src/workers/transfers/v2.rs` admits the Mesh executor
  and rejects an unsupported kind before provider effects.
- `crates/services/mde-collab-core/src/import.rs` contains the bounded,
  idempotent legacy migration importer.
- `crates/services/mde-collab-core/src/projection.rs` exposes the canonical
  global `AlertInbox` projection and folds alert lifecycle events.

## Still genuinely blocked

- Native office editing still requires a packaged, sandboxed out-of-process
  LibreOfficeKit adapter; the existing admission boundary correctly refuses
  unsafe fallback paths.
- Calls still require an operator-installed governed SIP account for live
  activation. No live provider artifact is fabricated by this audit.

The result is an implementation reconciliation, not a release or live-device
claim.
