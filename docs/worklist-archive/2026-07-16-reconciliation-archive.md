# Worklist Archive - 2026-07-16 Reconciliation

This archive records the non-active disposition of the pre-reconciliation
platform worklist. The preserved source is:

- `docs/worklist-archive/2026-07-16-platform-worklist-pre-reconcile.md`
- `docs/worklist-archive/2026-07-16-platform-worklist-marker-index.tsv`

The active replacement is `docs/platform/WORKLIST.md`. The reconciliation report
is `docs/platform/WORKLIST-RECONCILIATION-2026-07-16.md`.

## Redaction Note

The historical worklist previously carried secret-adjacent live-operation notes.
The archive keeps the historical text but redacts credential-shaped access-key
IDs and the already-redacted seat-login token shape. Do not add live secrets to
worklist, archive, report, or acceptance notes.

## Bulk Disposition Rules

The marker index is the line-level archive for every status-marked source row.
It contains the original source line number, original marker, reconciled
disposition class, and title text.

Rows not copied into the active worklist use these final classifications:

| Original marker | Final classification | Reason |
| --- | --- | --- |
| `[✓]` | Completed | Source row claimed completion and either later repo evidence supported it or the work was no longer active after newer slices. |
| `[x]` | Completed | Historical alternate completed marker normalized to `Completed`. |
| `[X]` | Completed | Historical alternate completed marker normalized to `Completed`. |
| `[✗]` | Superseded or Obsolete | Source row explicitly dropped or referred to retired architecture. |
| `[->]` or `[→]` | Merged | Source row was rehomed into another item or later design. |
| `[!]` | Blocked | Source row names an operator, live-infra, hardware, external account, or release gate. |
| `[~]` | Blocked | Source row is partial and live/operator-gated. |
| `[ ]` | Merged into active worklist or archived by report | Open candidates were reviewed, corrected, and consolidated into `WL-*` items. |
| `[>]` | Merged into active worklist or archived by report | In-progress candidates were reviewed, corrected, and consolidated into `WL-*` items. |
| `[◐]` | Merged or Needs clarification | Partial rows were folded into Quasar Cloud resource-work items. |

## Marker Totals From The Preserved Source

| Classification bucket | Count |
| --- | ---: |
| Completed markers (`[✓]`, `[x]`) | 1,384 |
| Superseded or obsolete markers (`[✗]` plus XCP reclassification) | 39 |
| Explicit merged markers (`[→]`) | 15 |
| Blocked/partial-blocked markers (`[!]`, `[~]`) | 38 |
| Open or in-progress candidate markers (`[ ]`, `[>]`) | 127 |
| Partial marker rows (`[◐]`) | 2 |
| Total status-marked rows archived | 1,605 |

## Archived Source Groups

| Original source group | Final classification | Reason for archival | Evidence | Replacement or active item |
| --- | --- | --- | --- | --- |
| Pre-E12, E1-E11, and old platform survey rows | Completed, Superseded, or Obsolete | These rows describe shipped older epochs, retired COSMIC/iced/Carbon directions, or work already absorbed by the 12.x egui/Quasar architecture. | `AI_GOVERNANCE.md` current architecture; repo scan for egui-native shell and retired cloud-hypervisor guidance. | WL-DOC-001 for stale-doc banners only. |
| E12 mesh VDI rows | Merged | The valid remaining work is brokered console attach, reconnect, local audio, packaging, and live verification, not the original monolithic E12 story text. | `desktop_sources.rs`, `session_broker.rs`, `vdi.rs`, platform review VDI findings. | WL-CRIT-001, WL-CRIT-002, WL-BUILD-001, WL-ARCH-001. |
| Onboarding Wizard rows | Merged or Blocked | GUI and service flows have many completed slices; residuals are fresh-node enrollment, remote push/live verification, packaging, and services. | `docs/NEEDS-OPERATOR.md`, old OW rows, onboarding modules. | WL-SEC-001, WL-BUILD-001, WL-RUN-004, WL-DOC-002. |
| Media lighthouse rows | Merged or Blocked | Bucket/role/Navidrome pieces are partly proven; remaining work is live production account, upload/rescan, failover, and fresh-node browse. | Old MEDIA rows, `docs/ops/media-ingestion.md`, media worker evidence. | WL-RUN-004, WL-FUNC-007. |
| DR, DAR, and control VM rows | Merged or Blocked | Many code slices are complete; live/off-fleet DR and final bootstrap enrollment remain. | Old DR/DAR rows; `automation/dr/`; `docs/NEEDS-OPERATOR.md`. | WL-CRIT-004, WL-SEC-001, WL-BUILD-002. |
| Quasar Cloud/QC/IAC rows | Merged | The active work is current Nova/libvirt cloud cutover, resource verbs/forms, live cloud gate, SELinux, and QC-23 fast path. | `crates/mesh/mackesd/src/workers/openstack/`, cloud docs, old QC rows. | WL-ARCH-001, WL-ARCH-002, WL-TEST-001. |
| XCP-ng adoption/provider rows | Obsolete | Operator decision 2026-07-16: archive XCP-ng adoption as obsolete under the Nova/libvirt architecture. | MV-7 and XCP-6 source rows; Quasar Cloud supersession. | None. |
| Browser base CEF/Servo integration rows | Completed or Merged | The CEF helper, runtime installer, browser options page, vertical tabs default, and first-party chrome extraction now exist in code. Remaining work is regressions and daily-driver tails. | `rg mde://browser/options`, `BrowserInternalPage::Options`, CEF helper files, browser tests. | WL-CRIT-003, WL-UX-002, WL-FUNC-001 through WL-FUNC-004. |
| Browser WebExtensions/LastPass rows | Merged or Superseded | v1 path moved to native adblock/password/passkey/download tooling; WebExtensions remain a future Chrome-runtime path, not a v1 blocker. | Old BROWSER-DD-5 notes, CEF Alloy gate notes. | WL-FUNC-004 if revived; otherwise archived as Superseded for v1. |
| Browser passkey rows | Merged | Consent and bridge slices landed; hardware/phone/real-site proof remains. | Browser passkey code and old passkey notes. | WL-FUNC-002. |
| Browser Chrome C0-C5 rows | Merged | Most implementation slices landed; remaining work is live visual parity and the current geometry/idle-media bug. | `crates/desktop/mde-shell-egui/src/web/chrome_ui/`, browser tests. | WL-CRIT-003, WL-UX-002. |
| Device Manager rows | Merged | The active remainder is multi-source inventory and debounced fault notifications. | Old Device Manager bullets and device manager module. | WL-RUN-005. |
| Router rows | Merged | Discovery/control requirements remain valid but need current shell alignment and live router proof. | `docs/design/router-control.md`, old ROUTER rows. | WL-RUN-006. |
| Search/omnibox rows | Merged | Search remains a valid epic but now needs a cleaner data/indexing definition. | Browser omnibox and front-door search rows. | WL-FUNC-005. |
| Win10 hybrid/taskbar rows | Merged or Blocked | Many slices are done; live tray visual proof and bottom-bar progress integration remain. | Old B5-rest and NAVBAR rows. | WL-UX-001, WL-FUNC-006. |
| Accessibility rows | Merged | Runtime AccessKit bridge exists now; remaining work is consumer, app sweep, live regions, and companion apps. | `crates/shared/mde-egui/src/a11y.rs`, `drm.rs`, platform review a11y findings. | WL-UX-003. |
| Platform review completed rows | Completed | Open ledger shows many findings fixed after the review; repo evidence confirms several. | `docs/review/OPEN-LEDGER-2026-07-11.md` and code scan. | None. |
| Platform review residual rows | Merged | Remaining review-backed gaps are now captured as specific active WL items. | `docs/review/PLATFORM-REVIEW-2026-07-10.md`. | WL-ARCH-003, WL-ARCH-004, WL-RUN-001, WL-RUN-002, WL-PERF-001, WL-TEST-001, WL-TEST-002, WL-DOC-001. |
| NEEDS-OPERATOR rows | Merged or Blocked | Operator queue remains useful but must reference new IDs rather than act as a second active worklist. | `docs/NEEDS-OPERATOR.md`. | WL-DOC-002 and blocked items in the active list. |
| Compliance/reconcile-plan rows | Completed, Superseded, or Merged | Compliance cycles are historical evidence; branch reconciliation plan targeted old branches and versions. | `docs/COMPLIANCE.md`, `docs/RECONCILE-PLAN.md`. | WL-DOC-001, WL-BUILD-003 where residual applies. |

## Closed After Reconciliation

| Original ID and title | Final classification | Reason for archival | Evidence | Replacement or merged item |
| --- | --- | --- | --- | --- |
| WL-TEST-003 Worklist and farm hygiene guardrails | Completed | The active worklist now has an executable lint gate, the farm job parser understands reconciled status-word items, status/drain consumers report the new active counts, and placeholder `@farm` payloads fail planted tests. | `install-helpers/lint-worklist.sh`, `automation/lib/farm-jobs.sh`, `automation/promotion/mcnf-promotion-cycle.sh`, `install-helpers/drain-coordinator.sh`; local `lint-worklist.sh --self-test`, `farm-jobs.sh --self-test`, `lint-worklist.sh`; farm `.50` hygiene lane passed on 2026-07-16. | None. |
| WL-UX-004 Brand spelling and product identity sweep | Completed | Current source, generated-user-facing metadata, install helpers, packaging, and non-archive docs now use canonical `Quazar` spelling; lower-case package/asset paths remain intentionally unchanged, and only two explicit historical S-spelling decision lines are allowed. | Commits `8a5935d9`, `2df459a1`, `5f610b26`, and `d842bc1f`; `install-helpers/lint-brand-identity.sh` scans the current docs tree plus source/install/package roots and falls back to `grep` on farm images without `rg`; local brand/worklist/diff checks and farm `.50` `brand-docs-lint` passed on 2026-07-17. | None. |

## Archive Policy Going Forward

When an active `WL-*` item completes, move a short archive row here or into a
new dated archive file with:

- Original ID and title.
- Final classification.
- Reason for archival.
- Evidence.
- Replacement or merged ID, if any.
- Reconciliation date.

Do not append completed implementation logs to `docs/platform/WORKLIST.md`.
