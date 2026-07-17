# Worklist Reconciliation Report - 2026-07-16

Authoritative active worklist: `docs/platform/WORKLIST.md`

Archive:

- `docs/worklist-archive/2026-07-16-platform-worklist-pre-reconcile.md`
- `docs/worklist-archive/2026-07-16-platform-worklist-marker-index.tsv`
- `docs/worklist-archive/2026-07-16-reconciliation-archive.md`

## Summary

This reconciliation replaced the 6,357-line mixed-status platform worklist with
a single active worklist containing only current work. The previous file is
preserved verbatim except for targeted redaction of credential-shaped values.

Primary source reviewed:

- 1,605 status-marked rows from the pre-reconciliation platform worklist.
- Additional evidence from `docs/NEEDS-OPERATOR.md`, `docs/COMPLIANCE.md`,
  `docs/RECONCILE-PLAN.md`, `docs/review/OPEN-LEDGER-2026-07-11.md`,
  `docs/review/PLATFORM-REVIEW-2026-07-10.md`, current code, docs, scripts,
  and TODO/FIXME/stub scans.

Active result:

| Active status | Count |
| --- | ---: |
| Remaining | 34 |
| Blocked | 8 |
| Needs clarification | 0 |
| Total active worklist items | 42 |

Archived source-marker disposition:

| Final classification bucket | Count |
| --- | ---: |
| Completed | 1,384 |
| Superseded or obsolete | 39 |
| Explicit merged markers | 15 |
| Blocked or partial-blocked source markers | 38 |
| Open or in-progress candidates consolidated | 127 |
| Partial marker rows consolidated | 2 |
| Total status-marked rows reviewed | 1,605 |

The `127` open/in-progress source markers and `2` partial rows were not copied
forward one-for-one. They were corrected, deduplicated, and consolidated into
43 `WL-*` items during reconciliation. One of those items, WL-TEST-003, was
completed in the follow-up hygiene pass and archived, leaving 42 active items.

## Required Counts

| Requested count | Result |
| --- | ---: |
| Total original status-marked items reviewed | 1,605 |
| Remaining active items | 34 |
| Completed source rows archived | 1,384 |
| Superseded source rows archived | 39 |
| Duplicated source rows | 0 primary rows; duplicate descriptions were folded under `Merged` |
| Merged source rows/candidates | 144 |
| Obsolete or invalid source rows | 39, counted with superseded/obsolete because old rows usually combined both conditions |
| Blocked active items | 8 |
| Needs clarification active items | 0 |
| Missing items discovered and added | 6, including 1 now completed and archived |

## Missing Work Added

| New item | Evidence |
| --- | --- |
| WL-CRIT-003 Browser geometry and idle media regression | User bug report on 2026-07-16 plus current Browser layout/repaint architecture. |
| WL-FUNC-006 Bottom navigation session entries and file-operation progress | User design directive that file-operation progress belongs in the bottom navigation bar and should be reused platform-wide. |
| WL-ARCH-005 Browser worker crypto seam and mde-seal emitter completion | TODO scan found production placeholder returns in `crates/mesh/mde-seal/src/lib.rs`; open ledger left browser passkeys dependent on shared crypto. |
| WL-BUILD-003 Promotion rollback, version matrix, and secret-scan gates | Platform review credential finding plus archive redaction pass confirmed worklist/docs can carry secret-shaped values if not gated. |
| WL-TEST-003 Worklist and farm hygiene guardrails | The old worklist itself exceeded usable size and user explicitly forbade retesting only to refill a host. Completed and archived after the hygiene gate landed. |
| WL-DOC-002 Re-key operator queue to reconciled IDs | `docs/NEEDS-OPERATOR.md` remains useful but still points at old IDs and would otherwise become a parallel tracker. |

## Contradictions Found

| Contradiction | Correction |
| --- | --- |
| The old worklist claimed to be the durable tracker while also warning it was too large for humans and agents. | The old file was archived; `docs/platform/WORKLIST.md` is now concise and active-only. |
| Status legend listed only `[ ]`, `[>]`, and `[✓]`, while the file used `[x]`, `[✗]`, `[!]`, `[~]`, `[→]`, and `[◐]`. | New active file uses explicit words: `Remaining`, `Blocked`, `Needs clarification`. |
| E12/OW text still mentioned cloud-hypervisor while governance and Quazar Cloud docs say Nova/libvirt/QEMU-KVM. | Active items reference the Nova/libvirt/QEMU-KVM stack; old cloud-hypervisor text is historical. |
| Browser C0-C5 still said "continue extraction" although code now contains `web/chrome_ui/`, internal options page, icons, and vertical-tabs default tests. | Browser chrome work was reclassified as visual/live-audit residual and current regression work: WL-CRIT-003 and WL-UX-002. |
| Platform review accessibility findings said the production DRM seat had no AccessKit path, but current code has `A11yBridge` and `MDE_A11Y`. | Accessibility was corrected to residual consumer/app-sweep work: WL-UX-003. |
| CEF resource callbacks were reviewed as unbounded, but current code has caps and fail-closed behavior. | The old review row is archived as completed/residual-only; no active item was created for that exact issue. |
| `docs/NEEDS-OPERATOR.md` and the old worklist both acted as active queues. | Active blocked work is in `docs/platform/WORKLIST.md`; NEEDS-OPERATOR needs re-keying under WL-DOC-002. |
| Several docs still describe historical Carbon/COSMIC/mde-workbench/cloud-hypervisor designs without clear current/historical labeling. | WL-DOC-001 tracks current-doc cleanup and supersession banners. |

## Important Corrections Made

- Preserved the previous platform worklist in `docs/worklist-archive/`.
- Redacted credential-shaped access-key IDs in the permanent archive and marker
  index.
- Replaced status markers with plain status words.
- Consolidated Browser daily-driver work into specific current items instead of
  keeping one giant browser epic.
- Removed completed Browser options/chrome implementation work from the active
  list while preserving live visual and regression work.
- Reframed cloud-hypervisor-era items to current Nova/libvirt/QEMU-KVM language.
- Separated operator/live-gated blocked work from coding-agent-drainable work.
- Added active work for the user-reported Browser layout/video bugs and the
  bottom-navigation file-operation progress requirement.

## Areas Not Fully Verified

These need live hardware, external accounts, a farm dev cloud, or operator action
before they can be closed:

- Live DR export/restore using off-fleet target and CA holder.
- Live substrate-v2 cutover on deployed lighthouses.
- Fresh-node enrollment against current public lighthouse endpoint.
- Live Quazar Cloud resource create/delete on farm/dev cloud.
- Protected media/Widevine and third-party DRM playback.
- Hardware passkey/CTAP2 and phone-as-authenticator.
- YouTube or equivalent long-running browser media playback on the DRM seat.
- Two Lighthouse_Media failover plus upload/rescan/fresh-node browse.
- Bootc/ISO/headless Workstation release boot proof.

## Commands And Evidence Used

The initial reconciliation was documentation-focused. The follow-up hygiene pass
added an executable lint gate and used the build farm for targeted verification
only; idle hosts were not refilled with duplicate work.

Commands used included:

- `rg --files`
- `find docs -maxdepth 3 ...`
- `wc -l docs/WORKLIST.md docs/platform/WORKLIST.md`
- `awk` extraction of status-marked rows into the archive marker index.
- `rg` scans for TODO/FIXME/HACK/stub/placeholder/unimplemented markers.
- `rg` scans for retired architecture terms and worklist references.
- `rg` and `sed` inspection of Browser, AccessKit, CEF, OpenStack, VDI, and
  review-ledger evidence.
- `rg` checks for credential-shaped archive content followed by targeted
  redaction.
- `install-helpers/lint-worklist.sh --self-test`
- `install-helpers/lint-worklist.sh`
- `automation/lib/farm-jobs.sh --self-test`
- `install-helpers/drain-coordinator.sh next 5`
- `automation/promotion/mcnf-promotion-cycle.sh status`
- Farm `.50`: synced hygiene lane with worklist lint, farm-job parser self-test,
  drain next-candidate check, and promotion status check.
- Farm `.130`: `cargo test -p mde-shell-egui browser -- --nocapture`, 212 passed.
- Farm `.90`: `cargo test -p mde-shell-egui tab_strip -- --nocapture`, 6 passed.
- Farm `.170`: `cargo test -p mde-shell-egui vertical_tab -- --nocapture`, 5 passed.

## Mapping Table

The line-level mapping for all 1,605 source rows is
`docs/worklist-archive/2026-07-16-platform-worklist-marker-index.tsv`. The table
below maps the major original source groups and residual IDs to active or
archived disposition.

| Original item or group | Final status | New or replacement item | Evidence |
| --- | --- | --- | --- |
| `docs/WORKLIST.md` root pointer | Completed | `docs/platform/WORKLIST.md` | Root file already pointed to platform worklist. |
| Old `docs/platform/WORKLIST.md` giant tracker | Merged/Archived | This report plus new active worklist | 6,357 lines, 1,605 marked rows, mega-lines up to 78K chars. |
| E12-5 Remote desktop over mesh | Merged | WL-CRIT-001 | VDI/desktop source/session broker evidence. |
| E12-8 Session roaming/persistence | Merged | WL-CRIT-002 | Review reconnect findings and VDI state. |
| E12-9 Bridges | Merged | WL-ARCH-001 and future audio work under architecture/runtime scope | `docs/design/e12-9-10-libvirt-rescope.md`; cloud-hypervisor retired. |
| E12-10 Advanced VDI | Merged | WL-PERF-001, WL-CRIT-001 | VDI display and live hardware gates. |
| E12-13 Packaging bootc Workstation | Blocked | WL-BUILD-001 | Old packaging gate and bootc/ISO paths. |
| OW-4 Invite/Join residual | Merged | WL-SEC-001 | `docs/NEEDS-OPERATOR.md` final bootstrap blocker. |
| OW-7 Spawn Lighthouse | Merged | WL-RUN-003, WL-SEC-001 | Lighthouse add/retire and bootstrap paths. |
| OW-8 Workstation provisioning/first desktop | Merged | WL-CRIT-001, WL-ARCH-001 | Nova/libvirt correction in NEEDS-OPERATOR. |
| OW-11 Services flow | Merged/Blocked | WL-RUN-004, WL-FUNC-008 | Media/SIP/service flow residuals. |
| OW-12 ISO/RPM/bootc | Blocked | WL-BUILD-001 | Live boot/release gate. |
| MEDIA-1 through MEDIA-10 | Merged/Blocked | WL-RUN-004, WL-FUNC-007 | Media rows and operator queue. |
| DR #4 / DATACENTER-23 | Blocked | WL-CRIT-004 | DR code done but off-fleet CA/secret export is operator-run. |
| LH-JOIN-QNM and OPROG wedge rows | Blocked | WL-CRIT-005 | Incident rows and substrate-v2 runbook. |
| FARM-AUTO-PROD and DAR-34/35/36 | Merged | WL-BUILD-002 | Farm bootstrap/cache rows. |
| XCP-6 / MV-7 | Obsolete | None | Operator decision 2026-07-16: archive XCP-ng adoption under the Nova/libvirt architecture. |
| DATACENTER-3 / DS-8 secrets | Blocked | WL-SEC-003 | Secret-store live multi-node gate. |
| DATACENTER-14 Gateway tab | Merged | WL-RUN-006 | Router/firewall control residual. |
| QC-13, QC-15, QC-16, QC-18, QC-21, QC-22, QC-23 | Merged | WL-ARCH-001, WL-ARCH-002, WL-TEST-001 | OpenStack worker files and QC docs. |
| IAC partial rows `[◐]` | Merged | WL-ARCH-002 | Cloud resource create/update/delete forms omitted, not faked. |
| NODE-GRADE-4 | Blocked | WL-RUN-005 and live-smoke evidence under runtime | Live test-bed proof required. |
| BROWSER-DD-1 CEF integration | Completed | None | CEF helper/runtime installer and live smoke evidence in old rows and code. |
| BROWSER-DD-2 tabs/omnibox/chrome | Merged | WL-CRIT-003, WL-UX-002 | Current code has options page/vertical tabs; user bug remains. |
| BROWSER-DD-4 Widevine DRM | Remaining | WL-FUNC-001 | Protected media not fully live-proven. |
| BROWSER-DD-5 WebExtensions/LastPass | Superseded for v1 | WL-FUNC-004 only if revived | Old row says v1 no longer depends on extensions. |
| BROWSER-DD-6 WebAuthn/passkeys | Merged | WL-FUNC-002 | Consent landed; hardware/phone/live proof remains. |
| BROWSER-DD-7 sync/follow-me | Merged | WL-FUNC-003 | Mesh sync/bookmark integration remains. |
| BROWSER-DD-8 power/downloader/scraper | Merged | WL-FUNC-004 | Command-tool residual. |
| BROWSER-DD-9 media/conferencing | Merged | WL-FUNC-001 | Media/PiP/background/HW decode remain. |
| BROWSER-DD-10 chrome/UX | Merged | WL-CRIT-003, WL-UX-002, WL-FUNC-004 | Chrome code landed; live bug and tool residuals remain. |
| BROWSER-DD-11 accessibility/TTS | Merged | WL-UX-003 | Runtime bridge exists; consumer/app sweep remains. |
| BROWSER-DD-12 platform long tail | Merged | WL-FUNC-004 | Protocol/cache/notifications residual. |
| C0-C5 Browser chrome rebuild | Merged | WL-CRIT-003, WL-UX-002 | `mde://browser/options`, `chrome_ui`, vertical-tabs tests exist. |
| Device Manager open bullets | Merged | WL-RUN-005 | Multi-source host rail and notification residuals. |
| ROUTER rows | Merged | WL-RUN-006 | Router-control design and open bullets. |
| SEARCH-omnibox | Merged | WL-FUNC-005 | Full search/indexing deferred epic. |
| NAVBAR-U3 | Merged | WL-FUNC-006 | Active sessions as bottom-bar entries. |
| User bottom-nav progress directive | Remaining | WL-FUNC-006 | Direct user requirement. |
| BUG-VIDEO-1 / MEDIA-VIDEO | Blocked | WL-FUNC-007 | mpv/live seat proof required. |
| B5-rest / Win10 tray | Blocked | WL-UX-001 | Live visual proof remains. |
| Platform review `arch-11` | Remaining | WL-ARCH-003 | Open ledger still lists shared Bus seam absent. |
| Platform review `mackesd-03` | Remaining | WL-RUN-001 | Observe-only auto-repair gap. |
| Platform review `test-obs-9` | Remaining | WL-RUN-002 | Counter registry gap. |
| Platform review `perf-7` | Remaining | WL-PERF-001 | Dirty-rectangle residual partial. |
| Platform review `test-obs-5` | Remaining | WL-TEST-001 | OpenStack live/contract test gap. |
| Platform review `test-obs-6` | Remaining | WL-TEST-002 | Real etcd/Nebula harness gap. |
| Platform review a11y cluster | Merged | WL-UX-003 | Current AccessKit bridge changes original status; residual remains. |
| Platform review docs consistency rows | Merged | WL-DOC-001, WL-DOC-002; WL-TEST-003 archived Completed | Repo docs scan found stale terms and tracker drift, and the hygiene gate now exists. |
| `docs/NEEDS-OPERATOR.md` | Merged/Blocked | WL-DOC-002 and blocked active items | Operator queue remains but must not be a parallel active worklist. |
| `docs/COMPLIANCE.md` | Archived evidence | WL-BUILD-003 or WL-DOC-001 where residual applies | Historical compliance cycles mostly resolved. |
| `docs/RECONCILE-PLAN.md` | Superseded | None, except evidence for archive discipline | Old master/farm-autoscale branch plan. |

## Recommended Execution Order

1. WL-CRIT-003 - Fix Browser geometry and idle media regression. It is current
   user-visible breakage and also validates the Browser chrome work.
2. WL-SEC-001 and WL-CRIT-005 - Stabilize fresh-node join and substrate-v2
   cutover blockers before broader live fleet work.
3. WL-CRIT-001 and WL-CRIT-002 - Complete the flagship VDI console broker and
   reconnect UX together; reconnect tests need a real transport.
4. WL-BUILD-001 and WL-BUILD-003 - Make release/rollback/secret-scan gates
   honest before cutting or promoting more live artifacts.
5. WL-ARCH-001, WL-ARCH-002, and WL-TEST-001 - Finish Quazar Cloud hard cutover,
   resource verbs, and the live/contract test lane as one cloud program.
6. WL-CRIT-004 and WL-SEC-003 - Complete DR and secret-store distribution after
   substrate and cloud foundations are stable.
7. WL-RUN-001 and WL-RUN-002 - Close the self-healing/observability say-do gaps.
8. WL-RUN-003 and WL-RUN-004 - Finish lighthouse turnkey operations and media
   lighthouse production behavior once live maintenance windows are available.
9. WL-FUNC-001 through WL-FUNC-004 - Continue Browser daily-driver tails in
   bounded slices after the current regression is fixed.
10. WL-FUNC-005, WL-FUNC-006, WL-RUN-005, and WL-RUN-006 - Build user-facing
    completeness work that depends on stable runtime records and bottom-bar UI.
11. WL-UX-001 through WL-UX-003 - Visual/accessibility polish. WL-UX-004 closed
    on 2026-07-17 after the canonical `Quazar` source/docs sweep and drift guard.
12. WL-PERF-001 and WL-PERF-002 - Performance work after correctness paths have
    stable tests and live probes.
13. WL-TEST-002 - Harness work; keep the completed hygiene lint in the regular
    gate path, and do not run farm jobs merely to fill idle hosts.
14. WL-DOC-001 through WL-DOC-003 - Documentation cleanup and worklist lifecycle
    hardening after active IDs land.
