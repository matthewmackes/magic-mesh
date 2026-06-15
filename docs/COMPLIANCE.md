# Magic Mesh — Compliance & Integrity Sweep

**Date:** 2026-06-12 · **Scope:** 22 crates · **Rulebook:** `AI_GOVERNANCE.md` (E11 "Magic Mesh" pivot) · **Lens:** post-hardening integrity — does the code shipped by the AUD-1..21 + EFF drain hold to §7 "marked done = reachable + correct"?

Verdicts are binary: **FINISH** (make it real / wire it / fix the doc) or **REMOVE** (delete the dead surface). Report-only — nothing was modified by this sweep. Three parallel sub-audits (unreachable+stubs · mockups+conventions · doc-drift+packaging).

> Supersedes the 2026-06-11 sweep. **Every headline item from that sweep is resolved:** mesh file transfer is real (FileXfer over the LizardFS volume, AUD-1/7, now share-root-confined per EFF-2), KDC outbound drains (AUD-2), the cosmic-applet + role-chooser GUI are packaged (AUD-4/6), the runtime disclaimer accept gate exists (AUD-5), both missing lint gates run in CI (AUD-21) and pass clean, the §2 private-bus-name and §4 parallel-token violations are gone, and all 2026-06-11 REMOVE items were deleted. This sweep audits the *new* code those fixes added plus the standing surface.

## Headline

The platform passes the previous sweep's failure modes cleanly: **zero `todo!()`/`unimplemented!()`**, all three governance lint gates green (Carbon §4, bus-names §2, mesh-boundary §6), substrate Nebula-clean, crypto at or above the §3 floor (the one RSA-2048 reference is ring's *verify-range* constant for KDE Connect interop — own keys are 4096), no production-path mockups, and every new EFF module (metrics_exporter, proc, ca/expiry, body-cap guards, from_store) verified reachable from a real entrypoint.

What remains is **second-order**: one observability seam left half-plumbed by the EFF work itself (the router histogram is observed but never exported), three small dead surfaces, and a cluster of stale doc/packaging claims — including one **release-blocking** packaging defect (an RPM asset pointing at a binary that doesn't exist; `cargo generate-rpm` would fail at cut time).

## Findings

| # | Location | Category | Evidence | Conf. | Verdict |
|---|----------|----------|----------|:---:|:---:|
| **1** | `mackesd/src/metrics.rs:96` (`percentile_estimate`), `workers/mesh_router.rs:411` (histogram observed), `workers/metrics_exporter.rs` (`write_textfile(…, &[])`) | Half-plumbed | The KDC2 router-decision histogram is built + observed per tick in `mesh_router`, but the EFF-9 exporter always passes an **empty histograms slice** — observed, never exported; `percentile_estimate` has zero production callers. The SLO instrumentation exists and is silently dropped at the export seam. | High | **FINISH** — share the router histogram (Arc) into `MetricsExporterWorker`, or `#[cfg(test)]`-gate the percentile API |
| **2** | `mackesd/src/ipc/files.rs:143–199` (free fns `inbox_reply`, `outbox_reply`, `file_ops_reply`) | Unreachable | The pre-FileXfer "honest empty" free functions are no longer wired anywhere — `bin/mackesd.rs` always constructs `FileXfer` and uses its methods; only the free `downloads_reply` is still wired (line 6156). Dead degraded-path code that can silently drift from the live one. | High | **REMOVE** (the three dead free fns; keep `downloads_reply`) |
| **3** | `crates/shared/mde-theme/src/elevation.rs` (`Elevation` enum + `radius()`/`shadow()`) | Unreachable | Zero callers outside the module's own tests; no `Elevation` import anywhere in the workspace. | High | **FINISH** (adopt in panel/dialog chrome) or **REMOVE** |
| **4** | `crates/shared/mde-theme/src/brand.rs` (`Brand`/`BrandSlot`/`BrandAsset`/`BrandFormat`/`BrandSource`) | Unreachable | Re-exported from `lib.rs` but no workspace crate instantiates any of it in production. | Med | **FINISH** (wire into app-header/role-chooser logo resolution) or **REMOVE** |
| **5** | `mackesd/src/events.rs:75` (`dispatch_alerts`, `AlertHook`) | Unreachable | Confirmed still dead — zero non-test callers; no `AlertHook` constructed anywhere. This is the already-tracked **EFF-25**; the sweep confirms it remains open (EFF-39 routed its `eprintln!` through tracing, but the dispatch layer itself is unwired). | High | **FINISH or REMOVE** (= EFF-25, no new item) |
| **6** | `crates/kdc/mde-kdc-proto/src/crypto.rs:209–224` module/struct docs | Doc drift (§3) | Prose says "RSA-2048 keypair holder" / "generating fresh RSA-2048 keypairs" — misleading vs §3. Reality: own keygen is pinned 4096 (`RSA_MODULUS_BITS = 4096` in keygen.rs); `RSA_PKCS1_2048_8192_SHA256` is ring's **verify range** accepting stock KDE Connect peers ≥2048. Code correct, prose wrong. | High | **FINISH (doc)** — state "verify accepts 2048–8192 for KDE Connect interop; own keys always 4096" |
| **7** | `mackesd/Cargo.toml:321` (`[package.metadata.generate-rpm]` asset `target/release/mde-mesh-wallpaper`) | Packaging — **release-blocking** | The RPM declares an asset for a binary **no workspace crate builds** (`mde-mesh-wallpaper` is PD-10's *planned* layer-shell bin). `cargo generate-rpm` fails at cut time on the missing file. All other 11 binary assets match real `[[bin]]` targets. | High | **FINISH** (build the PD-10 bin) or **REMOVE** (drop the asset line until it exists) |
| **8** | `packaging/repo/magic-mesh.repo` (`gpgkey=…/RPM-GPG-KEY-magic-mesh`) | Packaging | The referenced GPG key file is committed nowhere; `gpgcheck=1` would fail every install. Already-tracked **EFF-17**; confirmed still open. | High | **FINISH** (= EFF-17, no new item) |
| **9** | `.claude/skills/release/SKILL.md:24–29, 53` | Doc drift | Still claims *"Packaging is not yet wired… no `[package.metadata.generate-rpm]` in any Cargo.toml… a real cut is blocked"* and "all 20 crates" — both false (metadata exists at `mackesd/Cargo.toml:307`; 22 members). Already-tracked **EFF-41**; confirmed still open, now with the crate-count error noted. | High | **FINISH** (= EFF-41) |
| **10** | `.claude/skills/audit/SKILL.md:70` | Doc drift | "All 20 crates are workspace members" — count is 22 (mde-role-chooser, mde-disclaimer added post-prose). | High | **FINISH (doc)** |
| **11** | `README.md` crate table (platform group) | Doc drift | Omits `mde-cosmic-applet` and `mde-role-chooser` — both ship real RPM-packaged binaries. | Med | **FINISH (doc)** |

## Counts

| Category | FINISH | REMOVE-or-FINISH | Already-tracked (confirmed open) |
|---|:---:|:---:|:---:|
| Half-plumbed observability (#1) | 1 | — | — |
| Unreachable (#2–#4) | — | 3 | #5 = EFF-25 |
| Packaging (#7) | 1 | — | #8 = EFF-17 |
| Doc drift (#6, #10, #11) | 3 | — | #9 = EFF-41 |
| **New actionable** | **5** | **3** | **3 confirmed-open cross-refs** |

## Cleared (verified clean this sweep)

- **All three governance lint gates pass:** `lint-carbon-tokens.sh`, `lint-bus-names.sh`, `lint-mesh-boundary.sh` — clean output verbatim.
- **Every new EFF module reachable:** `metrics_exporter` (spawned in run_serve), `proc` (8 worker callers), `ca/expiry` (exporter), `body_within_cap`/`body_too_large_reply` (10 responder sites), `HealthReport::from_store` (3 callers), `alert_relay` (spawned).
- **Zero `todo!()`/`unimplemented!()`** anywhere; no production mockups (`DemoBackend`/`demo_data` strictly test-contained).
- **Substrate:** no live Tailscale/Headscale/DERP/Gluster code (comments/heritage analogies only); `mesh_services` test asserts `tailscaled` absent.
- **Crypto:** Ed25519/AES-256-GCM/ChaCha20-Poly1305/rustls pinned; own RSA pinned 4096; MD5 sightings are protocol-mandated interop (Subsonic token auth, RFC 2617 SIP digest fallback — both sanctioned in §3).
- **Carbon:** `cargo test -p mde-theme` green; all raw-literal sightings are tests, data-model CSS strings (tag colors), or the annotated dynamic album-art path.
- **2026-06-11 REMOVE items:** confirmed deleted (tag_predicate/window_rules/workspace_overrides, dead mde-iced-components widgets, MeshShuntWorker struct).
- **Packaging assets:** 11 of 12 binaries + every static asset path verified present; DISCLAIMER.md non-empty + embedded via `include_str!`.

## Verdict

The 2026-06-11 → 2026-06-12 delta is what a §7-disciplined hardening pass should look like: every prior headline gap closed *and reachable*, no new stubs introduced, and the governance gates that were missing are now both present and passing. The remaining work is small and precisely nameable — **fix the RPM wallpaper asset before any release cut (#7)**, plumb or prune the router-histogram seam (#1), delete three dead surfaces (#2–#4), and clear four doc-drift items (#6, #9–#11). Nothing found this sweep contradicts production-readiness; #7 is the only item that would actually break a release.

---

## Fix-cycle resolution (2026-06-12, same day)

All 11 rows were resolved the same day and **independently re-verified by a fresh agent pass** (verification table: 20/20 YES):

- **#7 (the "release blocker") was a FALSE POSITIVE** — `mde-mesh-wallpaper` is a real PD-10 bin (`mde-workbench/src/bin/`), auto-discovered by Cargo without an explicit `[[bin]]` block; `cargo build --bin mde-mesh-wallpaper` verified green. The audit skill now carries an auto-discovery safeguard so this class of false positive doesn't recur.
- **#1** — worse than reported: production never even attached the histogram (`with_metrics` had zero bin callers). run_serve now shares one `RouterMetrics` Arc between `MeshRouterWorker` and the exporter; mackesd.prom carries the full `_bucket`/`_sum`/`_count` series (tested).
- **#2** — dead free fns deleted (`downloads_reply` kept; tests folded onto the live `FileXfer` methods).
- **#3** — `Elevation` REMOVED (shell-era Q29/Q30 tiers; `shadows` stays live via `Theme::modal_shadow`).
- **#4** — `brand` WIRED: role-chooser renders the wordmark via the Brand loader; the RPM ships the swappable pack to `/usr/share/mde/brand/`.
- **#5 (EFF-25)** — alert layer WIRED: `[[alert_hooks]]` in mackesd.toml → `dispatch_alerts` post-commit from the reconcile tick (typo'd kinds drop with a warn; rolled-back events can never alert). Remainder: only `Reconcile` is emitted today; new emission sites inherit the dispatch automatically.
- **#6, #9–#11** — all doc drift fixed (KDC RSA prose now states own-keys-4096/verify-range-2048–8192 everywhere incl. three sites the sweep missed; both skills updated; README platform row complete).
- **#8 (EFF-17)** — project Ed25519 signing key generated (private in operator `~/.gnupg`); public key committed at `packaging/repo/RPM-GPG-KEY-magic-mesh`; the one RPM ships the `.repo` + key so one-shot installs get a gpgcheck'd upgrade channel. The `magic-mesh-release` sub-package concept retired from `packaging/README.md` + `docs/help/install.md` (the re-audit's F-3).

Re-audit residuals F-1/F-2/F-3 (stale RSA-2048 prose in kdc-host, the sub-package install docs) fixed in the same cycle; F-4 (`percentile_estimate` test-only) accepted as a legitimate analysis API; F-5 cleared as correct interop prose.

**Cycle status: CLEAN.** Gates at close: workspace build green, `cargo test --workspace` 63/63 suites green, mackesd 1447 serial tests green, all three governance lints clean, zero `todo!()`/`unimplemented!()`.

---

## Cycle 3 (2026-06-12, same day) — fresh sweep + resolution

A third, deeper sweep (previously-skimmed crates, off-scale metrics, substrate gates, doc command accuracy) found **12 findings; all closed same day** (commit `a2b3c5a`) and **independently re-verified clean (10/10)**:

- **S-2/S-5 (the substantive pair):** the KDC2-1.9 scorer and KDC2-1.12 audit feed — scaffolded since v2.1 with comments promising "follow-up" — are now genuinely in the production path: `tick_once` → `select_paths` → `scorer::select` per peer, and every primary flip appends a hash-chained `PathSwitchEvent` (kind=`lifecycle`, so the EFF-25 alert hooks fire on path switches).
- **CV-1:** the `encryption_kind` field stopped being decorative — `score()` now filters content classes (Clipboard/FileBulk/Notification) below the AES-256-class floor (`Policy.min_content_encryption`, operator-tunable, typo = hard error); the stale WireGuard-AES-128 default corrected to the Nebula AES-256 reality.
- **S-3:** `peer_join` (PC-3) REMOVED rather than wired — it spawned `mde-peer-card`, deleted in the E11 pivot; the audit's FINISH verdict would have wired a dead modal.
- **S-4:** false positive (mdns_relay's inbound half is implemented; stale module doc fixed).
- **EFF-43 closed:** `kamailio-mde.service` + `rtpengine-mde.service` shipped disabled with render-config ExecStartPre, in the RPM assets.
- Plus: stale "Phase G stubs" comment, `mackesd --help` XFCE string, 3 wrong doc command examples, off-scale/density-blind spacing literals.

**Cycle-3 status: CLEAN (verified).** Three consecutive audit cycles have now converged same-day; the remaining open worklist is feature backlog (EFF P2/P3, PEERS epic), not integrity findings.

---

## Cycle 4 (2026-06-12) — fresh sweep + resolution

Fourth full sweep over the heavily-hardened tree (unreachable/stubs · conventions/mockups · docs/packaging). Docs/packaging pass: **NO FINDINGS** (every cited command/flag/path/metric/asset in the new ADMIN.md/CONTRIBUTING.md/architecture.md verified against the tree). Two findings, both fixed same day:

- **AUD4-1 (Unreachable, FINISH):** the VV-4 voice-routing heuristic (`voice::best_path`/`pick_relay`/`score`) was built + tested but had **only test callers** — the VV-2.a writer (`voice::materialize`) emitted `priority: 0` for every dispatcher row with an "until VV-4 ships" comment, and `Candidate::loss_pct` was a permanently-unconsumed field. **Fixed:** added `voice::dispatcher_priority` (best_path → inverted score → u8 band; non-direct floors to 0) and wired it into `build_voice_desired` via a per-peer latency-lookup closure that reads the mesh-latency cache (generic JSON parse — the worker type is async-gated, this path isn't). Healthy direct paths now rank high (faster ⇒ higher); unmeasured peers stay neutral; over-budget/unreachable floor to the transit tier. 2 new tests + the doc corrected.
- **AUD4-2 (Supply-chain, FINISH):** `cargo deny` advisories FAILED — `swash 0.2.8` (transitive via cosmic-text→iced) was yanked. **Fixed:** `cargo update -p swash` → 0.2.9; advisories green again.

**Cycle-4 status: CLEAN.** Four consecutive same-day audit cycles converged. Gates: mackesd 1450 serial green, three governance lints clean, cargo deny all-ok.

---

## Cycle 5 (2026-06-12) — verify + convergence

Verify-and-light-sweep pass. Cycle-4 fixes re-verified PASS (VV-4 dispatcher_priority reachable end-to-end from build_voice_desired; best_path/pick_relay/score now have a non-test caller; swash 0.2.8 gone, cargo deny all-ok). Fresh sweep otherwise CLEAN (zero todo!/unimplemented!, three lints clean, no new dead surface, latency-cache parse panic-safe). It surfaced **two real test-integrity bugs** the per-crate runs had masked, both fixed:

- **AUD5-1 (stale insta snapshot):** EFF-24 (`e0a337b`) added 4 fields to `HealthReport` but committed only the `.snap.new` — the accepted `library_contracts__health_report_empty.snap` was never promoted, so `snapshot_health_report_shape` failed at workspace scope. Promoted the snapshot (verified the new shape matches `HealthReport::empty()`).
- **AUD5-2 (audit-chain hash flake — latent, security-relevant):** `events::append_event` (EFF-25) and the original `worker::apply_repair_rows` computed each row's hash from `now_ms()` but stored `created_at` from a *separate* `Utc::now()` call. `load_audit_rows` reparses `created_at` → epoch-millis to recompute the chain hash, so a sub-millisecond drift between the two clock reads makes `audit::verify` spuriously report a `Break` (a false tamper alarm). Both now derive `created_at` from the single `now_ms` instant via `from_timestamp_millis`. Passed in isolation, failed under load — caught only by the workspace-scope run.

**Cycle-5 status: CLEAN — CONVERGED.** Five consecutive same-day audit cycles; the last two found only second-order test-integrity issues, now resolved. Gates: full workspace test green, mackesd 9/9 targets green, three governance lints clean, cargo deny all-ok.

---

## EFF-31 resolved (2026-06-12) — coverage gate made hard

Operator authorized package installs, unblocking the measurement EFF-31 needed. Installed cargo-llvm-cov 0.8.7 and measured the **library-logic** denominator — the 7 iced/libcosmic GUI crates excluded (render code; correctness rests on mde-theme tokens + the lifted visual gate) and the binary entrypoints (`src/bin/*.rs`, `src/main.rs`; ~5.7k integration-level lines in mackesd.rs alone) ignored: **84.84% lines**. Flipped the CI coverage job from advisory (`continue-on-error: true`) to a **hard `--fail-under-lines 80`** gate over that denominator (~5% margin), mackesd at `--test-threads=1`. Coverage can no longer silently rot. EFF-18 remains the sole CI-decision item parked for an operator call.

---

## Cycle 6 (2026-06-13) — "fit for purpose" sweep

Operator-directed lens: *would the product actually work for an arriving operator, end to end?* Four parallel passes (unreachable/stubs/mockups · substrate/crypto/secrets locks · doc drift · fit-for-purpose journeys) + the three governance lints (all clean) + green workspace suite.

**Journeys: all six PASS** — install→first-boot (RPM assets map 1:1 to the 12 workspace binaries; DISCLAIMER gate live), enroll (all 5 documented verbs exist with the quoted flags), daily driving (Front Door = Peers, directory responder live), failure honesty (EFF-45 pattern verified in peers/mesh_storage/profiles/home), systemd units, CLI smoke. **No blocking finding for a real operator.**

### Findings

| # | Location | Category | Evidence | Confidence | Verdict |
|---|---|---|---|---|---|
| AUD6-1 | `crates/workbench/mde-workbench/src/panels/mesh_join.rs:165–176` | Secrets (EFF-21 regression) | `run_mackesd_enroll` passes the 16-char passcode as `--passcode <value>` on argv (visible in `/proc/<pid>/cmdline`); `--passcode-stdin` exists and is ignored by this caller | High | FINISH — pipe via `--passcode-stdin` |
| AUD6-2 | `crates/mesh/mackesd/src/orchestrator.rs` | Unreachable | 522-line Send-To state machine; zero production callers workspace-wide (only a "future epic" comment in `ipc/files.rs`); internal tests only | High | REMOVE |
| AUD6-3 | `crates/mesh/mackesd/src/ipc/mod.rs:53` + doc table | Dead constant + stale doc | `MACKESD_BUS_NAME = "org.mackes.mackesd"` never referenced; module doc table still lists retired `org.mackes.*` D-Bus services as active (retired to mde-bus, E0.3.5) | High | REMOVE constant + FINISH doc |
| AUD6-4 | `crates/mesh/mackesd/src/ipc/notifications.rs` | Possibly-dead scaffolding | zbus `#[interface]` for FDO Notifications with no `serve_at` in `run_serve` — verify wiring; REMOVE if dead | Medium | FINISH (verify) |
| AUD6-5 | `crates/services/mde-voice-hud/src/sip.rs:1249` | Crypto preference | SIP digest default falls back to MD5 (RFC-mandated interop §3 exception) — verify negotiation prefers SHA-256 when the registrar offers it | Medium | FINISH (verify) |
| AUD6-6 | `packaging/` RPM metadata | Packaging gap | `kamailio-mde.service`/`rtpengine-mde.service` ExecStart external binaries not in `Recommends`; enabling the voice units without them = silent ExecStart failure | High | FINISH — add weak `Recommends` |
| AUD6-7 | `crates/mesh/mackesd/src/bin/mackesd.rs` | CLI polish | `role-workers` subcommand renders an empty description in `--help` | High | FINISH — add doc comment |
| AUD6-8 | `crates/workbench/mde-workbench/src/panels/compute.rs` | Failure honesty | first-paint "Loading instances…" has no timeout/error arm (panel probes local hypervisors, not mackesd — EFF-45 N/A but a hung probe shows Loading forever) | Medium | FINISH — error/timeout arm |
| AUD6-9 | `crates/mesh/mackesd/src/topology/mod.rs:58` | Doc drift (substrate) | `NebulaLighthouseRelay` doc says "Tailscale's relayed-via-DERP transport" — Tailscale copy on a Nebula-native variant | High | FINISH — fix doc |
| AUD6-10 | README.md (4 spots) | Doc drift | `gtk3-devel` prereq (no GTK dep exists); "sshd overlay-bind" (actual worker is additive, public kept — [[ssh-always-public]]); `mesh-wallpaper` in the crate diagram (bin lives in mde-workbench); "~30 workers" (52 today) | High | FINISH — 4 line fixes |
| AUD6-11 | Crate headers (5 crates) | Doc drift (pre-pivot heritage) | `mackesd/src/lib.rs` ("linked into mackes-panel", `PROJECT_WORKLIST.md`), `mde-workbench` lib.rs+Cargo.toml ("GTK3 Python Workbench", "mded", `dev.mackes.MDE.Shell.*`), `mackes-config` lib.rs+Cargo.toml ("mackes-panel"), `mackes-mesh-types` lib.rs ("mackes-panel") | High | FINISH — rewrite headers |
| AUD6-12 | docs/design (4 files) | Doc drift (COPR) | enterprise-readiness.md ("signed COPR", "no unit exists to start any of them at boot" — units ship now), platform-survey-answers.md ("signed COPR" ×2), planes.md ("magic-mesh COPR (W61)"), platform-survey.md Q79 reads as current | High | FINISH — GitHub-repo wording + annotate Q79 |
| AUD6-13 | `packaging/ENROLLMENT.md` | Doc gap | doesn't state the token embeds the lighthouse address (it does) — operator may hand-carry the IP needlessly | Medium | FINISH — one line |
| AUD6-14 | `packaging/kickstart/magic-on-cosmic.ks` | Packaging gap | `services --enabled=libvirtd` but libvirt not a Recommends — bare-Fedora Workstation spin may lack it | Low | FINISH — Recommends or drop |

**Counts:** 14 findings — 1 REMOVE, 13 FINISH (1 secrets-hygiene regression, 2 packaging, 2 verify-and-close, 8 doc/polish). Zero stubs, zero mockups, zero substrate/crypto/boundary violations, zero dead workers/panels. Lints clean ×3; workspace suite green.

**Cycle-6 status: FIT FOR PURPOSE with a short punch list.** The one security-relevant item is AUD6-1 (passcode on argv — a caller-side regression against the EFF-21 stdin path). Findings lifted into `docs/WORKLIST.md` as AUD6-*.

**Cycle-6 resolution (2026-06-13, same day):** all 14 findings closed in `f539430`. Highlights: AUD6-1 (passcode now rides `--passcode-stdin`, argv pinned secret-free by test) and AUD6-5's verify, which surfaced a REAL interop bug — rsip 0.4 can't parse the RFC 7616 `SHA-256` token, so an RFC-compliant registrar's challenge broke SIP registration outright; fixed with a manual fallback challenge parser + test. 847 dead lines removed (orchestrator.rs, ipc/notifications.rs, MACKESD_BUS_NAME). Gates: full workspace suite green, three lints clean. **Worklist: zero open items again.**

---

## Cycle 7 (2026-06-13) — post-NET-INTROSPECT + post-cutover sweep

Fresh integrity sweep after the NET-INTROSPECT feature (Nebula debug-SSH relay/direct classification) + the full libcosmic cutover. Inline: 5 governance/iced lints clean, zero stubs, zero raw hex. Three parallel deep passes (unreachable/stubs/mockups · substrate/crypto/secrets locks · doc-drift/packaging).

**NET-INTROSPECT security verdict: CLEAN.** The new Nebula debug-SSH is loopback-only (`render_sshd_block` hardcodes `listen: 127.0.0.1:{port}`, test asserts no `0.0.0.0`), Ed25519 keys 0600 under `/etc/nebula` (not a replicated path), no secret on argv (`-i <path>` only), needs no firewall rule (loopback). One zero-cost hardening noted (not a violation): host-key pinning over `StrictHostKeyChecking=no` — loopback has no MITM surface, so deferred.

### Findings

| # | Location | Category | Evidence | Confidence | Verdict |
|---|---|---|---|---|---|
| AUD7-8 | `crates/mesh/mackesd/Cargo.toml` generate-rpm | Packaging | NET-INTROSPECT shells `ssh-keygen`/`ssh`; `openssh-clients` not in Requires/Recommends → feature silently no-ops ("overlay") on minimal installs | High | **FIXED 2026-06-13** — added `openssh-clients` to weak `recommends` |
| AUD7-4 | `transport_probe.rs:11` doc | Doc drift | comment said direct/relay "needs the Nebula admin socket (not yet provisioned)" — NET-INTROSPECT provides it now | High | **FIXED** — points at `nebula_admin`; NAT-class noted as the remaining gap |
| AUD7-5 | `docs/WORKLIST.md` PD-6/PD-7 body prose | Doc drift | descriptions still said "pending Nebula admin introspection" / "until the Nebula admin socket is provisioned" (acceptance bullets were already flipped) | High | **FIXED** — prose refreshed to NET-INTROSPECT |
| AUD7-6 | `AI_GOVERNANCE.md §4` | Doc drift | "iced 0.14 (wgpu) + cosmic-text" — post-cutover it's libcosmic's vendored iced fork (carries a11y) | High | **FIXED** |
| AUD7-7 | `AI_GOVERNANCE.md §3` | Doc/security-model | §3 omitted the new loopback debug-SSH listener mackesd now enables | Med | **FIXED** — added the listener note (loopback-only, key-auth, graceful degradation) |
| AUD7-2 | `nebula_admin.rs` | Over-exposed API | `ensure_sshd_keys`/`render_sshd_block`/`parse_hostmap` were `pub` but consumed only within the mackesd lib + tests | Med | **FIXED** — demoted to `pub(crate)` |
| AUD7-1 | `workers/nebula_supervisor.rs:623` `check_leader` | Stub/dead params (PRE-EXISTING) | private async fn takes `_store`/`_node_id` (never read) + comment references a non-existent `leader::current_holder`; actually just a marker-file check | High | **OPEN — FINISH** (lifted) |
| AUD7-3 | `crates/services/mde-voice-hud/src/main.rs:1` module doc | Doc drift (PRE-EXISTING) | claims VOIP-28/29 "scaffold ships idle-state only" + a hardcoded `Registered · 127.0.0.1:5060` string; both shipped (live registration machine + call handlers) | High | **OPEN — FINISH** (lifted) |

**Counts:** 8 findings — 6 fixed same-day (the direct fallout of NET-INTROSPECT/cutover I'd just landed: 1 packaging, 4 doc-drift, 1 API-surface), 2 lifted OPEN (pre-existing, unrelated to this work: a dead-param stub + a stale voice-hud doc). Zero stubs, zero mockups, zero substrate/crypto/secrets violations, zero dead NET-INTROSPECT wiring (all 5 call chains verified reachable). Note: a few benign pre-existing unused-import/attribute warnings in unrelated workers (job_exec/mesh_shunt/mesh_dns/nebula_csr_watcher) — out of scope, candidate for a warnings-cleanup /ship.

**Cycle-7 status: CLEAN — NET-INTROSPECT fit for purpose, debug-SSH security verified.** The 2 open items are pre-existing and lifted to the worklist.

## AUDIT-MESH — workbench Mesh-section end-to-end data (2026-06-15, operator /audit)

Live audit: each Mesh-nav panel's data source tested against the running mesh
(host = healthy baseline w/ full mount; .13 = the "no data" node). Per the
finding→worklist+patch directive, each FINISH has an `AUDIT-MESH-*` worklist item.

| Panel | Data source | Live result | Finding | Verdict |
|---|---|---|---|---|
| Peers | `action/mesh/directory` | host=4 peers · .13=1 (self only) | `/mnt/mesh-storage` unmounted on .13 → local 1-record dir | **FINISH** (AUDIT-MESH-1; .13 live-patched) |
| Mesh DNS | `mackesd dns list` | host=4 fqdns · .13=1 | same mount root cause | **FINISH** (AUDIT-MESH-1) |
| Mesh Storage | `mackesd mesh-fs-status` | `master_reachable:true, peers:[], goal:0` | chunkservers/goal/usage not reported | **FINISH** (AUDIT-MESH-3) |
| Mesh Control | `action/nebula/self-node` | `overlay_ip:"", mesh_id:""` | self-node responder leaves overlay_ip/mesh_id blank | **FINISH** (AUDIT-MESH-2) |
| Routing | `mackesd validate status` | `{run_id:null}` | empty until a manual run | **FINISH** (AUDIT-MESH-5) |
| Discovered Hosts | `mackesd` probe (nmap) | nmap was absent → empty | nmap not a hard RPM dep | **FINISH** (AUDIT-MESH-6) |
| Music | `action/music/get-state` | empty | musicd serves no parseable state when idle/unconfigured | **FINISH** (AUDIT-MESH-4) |
| Connected Devices | `action/connect/devices` | `[]` | no paired devices (by-design unless KDC down) | OK (verify) |
| Message Bus | `mde-bus` (dnd + activity) | `dnd ok` | works | OK |
| nebula list-peers (mde-files) | `action/nebula/list-peers` | host=peers · .13 was `[]` | same mount root cause | **FINISH** (AUDIT-MESH-1) |
| Mesh Federation / Registration / Mesh Join / Mesh Pending | `mackesd`/`mde-bus` actions | action surfaces (not data reads) | n/a | OK |

**Root cause of "many panels return no data":** AUDIT-MESH-1 — the workgroup
mount (`/mnt/mesh-storage`) was missing on .13, so every directory/DNS/storage
read saw a local 1-record dir. Fixed live on .13 (directory now returns 4). The
durable fix (mount on every node, boot-durable, fail-loud watchdog) + the
code-level findings (2–6) are worklist items for the next release.
