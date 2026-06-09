# Magic Mesh — Worklist

The single durable tracker. Tasks lifted from `docs/COMPLIANCE.md` (sweeps 1 & 2, 2026-06-09).
Status: `[ ]` open · `[>]` in progress · `[✓]` done. Each task carries its finding id + verdict.
Ordered by priority — security lock first, then largest structural debt, then mechanical/doc cleanup.

**Operator decisions (2026-06-09):** A1/A2 → **DELETE** the labwc/sway surface · B1/H6/H3/H4 → **BUILD/WIRE** them · C1 → **implement Phase-G** · E1 → **retarget tests to Nebula**.

## P0 — Security / substrate lock (urgent)

- [✓] **H1 · RSA-2048 → RSA-4096 KDC device identity (§3)** — done (`a5186c5`); 49/0 green. — `mde-kdc-host/src/pairing.rs:236` `generate_pkcs8()` generates the live `identity.pkcs8` at 2048 bits via `PairingStore::open:101`. Rewire to the compliant 4096 generator that already exists (`keygen.rs:63`, `RSA_MODULUS_BITS=4096`, exported `lib.rs:41`); delete the duplicate 2048 `generate_pkcs8` in `pairing.rs`. Add/confirm a config test asserting 4096. **Do first — a max-crypto lock regression where the correct code already exists, just isn't called.**

## P1 — Retired labwc/sway desktop-shell surface (largest §5/§7 break)

- [✓] **A1 · DELETE the 13 sway/labwc workers** — done (`4fa070b`); cluster + role-table entries + census + `swayipc-async` dep removed. mackesd 1267/0.
- [✓] **A2 · DELETE the `window_manager` panel** — done (`4fa070b`); panel + 9 app.rs sites + nav + 2 role tests removed. workbench 760/0.

> **A1/A2 residuals (deferred, separate from the decided scope):** the `swaymsg exec` tag-launch CLI in `mackesd.rs:~1490` (a separate sway-dependent surface); `nebula_ca_backup.rs:37` "GlusterFS topology snapshot (GF-9.2)" doc (needs checking the snapshot code); `mackesd/src/ipc/nebula.rs:7` deleted-`crates/shell/` doc pointer. Low priority.

## P2 — §4 Carbon-token compliance (mechanical, high-value)

- [✓] **D1 · ~40 raw status-color literals → mde-theme tokens** (workbench) — done (`9d80767`). *Deferred: a `/preview` visual pass once a display is available (headless env here).*
- [✓] **H5 · mde-music maxi-view colors → palette** — done (`eddd2cc`).
- [✓] **H2 · voice-hud parallel palette → `mde_theme::carbon` ramp** — done (`35e7566`); added the single-sourced, test-pinned Carbon ramp to mde-theme. *Follow-up (low-pri): refactor `palette.rs` dark()/light() onto the ramp too, so mde-theme has one internal hex source.*

> The remaining `from_rgb`/struct-literal color sites the audit noted in `mde-iced-components/src/lib.rs` (tests) and `mde-files/src/widgets.rs:755` are either `#[cfg(test)]` (out of §4 scope) or a single hairline-blue in widgets — fold the widgets one into a later mde-files pass; not status colors.

## P3 — Substrate lock §1 (tests)

- [→] **E1 · retarget integration tests to Nebula** — **SPECCED** by the survey (Q87–90) → see **OBS-1**: retarget `integration_testcontainers.rs` to real `nebula-lighthouse` + peer containers (testcontainers), daemon-absent skip = hard fail.

## P4 — Unreachable pub surface (§7)

- [→] **H3 · `mde-card` dead pub surface** — **SPECCED** (Q38–40) → **GUI-4**: REMOVE all three (migration, RenderMode, TemplateSpec).
- [→] **H4 · `mde-iced-components` dead pub surface** — **SPECCED** (Q41–45) → **GUI-5**: REMOVE all five (motion, skeleton_shimmer, toast_chip, elevation_container, icon_fill_morph).

## P5 — Mockup / dead nav / stub surfaces (§7)

- [→] **B1 · `mesh_ssh` ("Mesh SSH")** — **SPECCED** (Q53–62) → **SVC-1**: fold a per-peer SSH status+launcher INTO the Remote Desktop panel ("Remote Access" = SSH+RDP+VNC); drop the standalone `mesh_ssh` nav entry.
- [→] **H6 · `mde-music` Radio card** — **SPECCED** (Q63–64) → **SVC-3**: BUILD `list-radio` (Airsonic `getInternetRadioStations` + verb + enqueue stream URL).
- [→] **C1 · Fleet Phase-G control plane** — **SPECCED** (Q1–18) → the **FLEET-PHASE-G** epic (FPG-1..8): the no-fixed-center revision plane.

## P6 — Doc drift (FINISH — fix docs)

- [✓] **F1 · `mde <subcommand>` dispatcher doc-drift** — done (`b6d74de`). Also fixed the mde-role NotPinned error + role.toml header (operator-facing strings pointing at the non-existent `mde setup`) and two extra `pre-mde-setup` comments in mackesd.
- [✓] **F2 · labwc-as-current doc-drift** — done (`b6d74de`). `repair.rs` reload action marked the legacy labwc path (code untouched, pending A1/A2). `mackesd/Cargo.toml:240` left — it's accurate heritage.
- [✓] **F3 · GlusterFS-lock doc-drift** — done (`b6d74de` + `38edbcf`); also caught mesh-types `tags.rs`/`peers.rs`. **Residual (defer):** `mackesd/workers/nebula_ca_backup.rs:37` "GlusterFS topology snapshot (GF-9.2)" describes a versioned backup payload field — needs checking the snapshot code before relabeling (don't guess). `mackesd/src/ipc/nebula.rs:7` + `window_manager.rs:8` cite deleted `crates/shell/` paths — the latter dies with A2.
- [✓] **H7 · `mde-music/src/library.rs:24–26` stale comment** — done (`b6d74de`); only Radio is unbacked.

## P7 — Vestigial model / soft seams

- [✓] **G1 · vestigial `derp` field** — done (`d8d79f7`); dropped the field + render fragment. mde-files 271/0.
- [→] **H8 · `SyntheticAnnounce`/`inject_synthetic` seam** — **SPECCED** (Q26–27) → **SEC-5**: BUILD the KDC2-4 mesh-shunt worker that consumes it (accept-any relayer; pinning is the gate).

---

# Platform epics (from the 100-question survey, 2026-06-09)

> Full rationale + the per-question locks live in `docs/design/platform-survey-answers.md`.
> 51 tasks across 6 epics. Each is `[ ]` Open; acceptance is runtime-observable per §7.
> The RPM is held until every feature is §7-complete; releasing is operator-gated (`/release`).

## FLEET-PHASE-G — the no-fixed-center fleet control plane (resolves C1)

Architecture: one unified `BaselineSpec` (YAML, monotonic `u64` version) written to LizardFS, which
is both transport (replication) and the authoritative log; leaderless authoring, last-writer-wins,
host-local Ansible apply.

- [ ] **FPG-1: unify the revision model** — one `BaselineSpec` (OS state + folded-in settings, Q9), YAML wire format (Q2), `u64` version id (Q1); retire the rowid + date-string schemes to display fields.
- [ ] **FPG-2: LizardFS revision log + store** — revisions written to LizardFS as the authoritative append-only log (Q3/Q8); replication is the transport; `mackesd` watches the path.
- [ ] **FPG-3: leaderless election** — any node mints+gossips; highest `version` wins (Q4/Q5); the leader lock only guards the local SQLite mirror write.
- [ ] **FPG-4: the Bus verbs** — implement `push`/`list`/`diff`/`rollback` (replace the `ipc/fleet.rs` stubs): rollback = mint a higher-version copy (Q6), flat top-level diff (Q7), `list` returns the full held set tagged with the winner (Q16).
- [ ] **FPG-5: apply-ack + signals** — nodes gossip an apply-ack advancing the author's FSM to Verified (Q14); emit `event/fleet/signals` {revision_id, peer, status} + a Workbench subscription (Q15).
- [ ] **FPG-6: cold-node convergence** — a joining/partitioned node applies the newest revision immediately, back-fills history lazily (Q18).
- [ ] **FPG-7: LizardFS mount ownership** — bind-mount the five XDG dirs (never `~/Local/`, Q13), default goal 2 (Q12), master pinned to Lighthouse nodes (Q11).
- [ ] **FPG-8: host-local Ansible apply** — `magic-fleet` reconciles the unified baseline host-local (Q10); revision auth rests on the Nebula transport, `author` advisory (Q17).

## SECURITY — CA lifecycle, enrollment, KDC (resolves H8)

- [ ] **SEC-1: non-expiring peer certs** — drop mid-epoch expiry; turnover via rotation/revocation (Q19).
- [ ] **SEC-2: passphrase-gated CA rotation** — `mackesd ca rotate` requires an operator passphrase, never auto-on-promotion (Q20).
- [ ] **SEC-3: QR/file 256-bit enrollment token** — replace the typed 16-char passcode with a delivered 256-bit token; keep auto-sign/TOFU (Q21/22).
- [ ] **SEC-4: outbound first-pair flow** — an operator-initiated KDC pairing flow that completes the handshake and writes the fingerprint pin (Q24/25); keep RSA-4096 (Q23).
- [ ] **SEC-5: KDC2-4 mesh-shunt worker** — consume `SyntheticAnnounce`/`inject_synthetic`, relay neighbors' `phones.json` mesh-wide; accept any relayer (Q26/27). *(resolves H8; SVC-6.)*
- [ ] **SEC-6: gossiped signed revocations** — a signed retract record gossips peer-to-peer (like fleet revisions) alongside the per-node ban files (Q28/29).
- [ ] **SEC-7: mandatory CA backup on lighthouse** — refuse-start / loud-warn without `MDE_BACKUP_PASSPHRASE`; one combined CA+topology bundle (Q31/32).
- [ ] **SEC-8: encrypt KDC session keys at rest** — persist session keys encrypted so links survive a daemon restart (Q34); keep AES-256-GCM (Q33).

## GUI — Carbon look + component cleanup (resolves H3, H4, H2-followup)

- [ ] **GUI-1: add Gray 90 theme** — `Theme::Gray90` + `Palette::gray_90()`, the full 3-theme set §4 names (Q35).
- [ ] **GUI-2: live theme switching** — thread the resolved `Palette` through `App` state so a theme change repaints live (Q36).
- [ ] **GUI-3: Carbon Themes-panel rewrite** — offer exactly Gray 10/90/100 via the mde-theme pref store; drop the retired presets + gsettings shell-out (Q37).
- [ ] **GUI-4: remove dead `mde-card` surfaces** — delete `migration`, `RenderMode`, `TemplateSpec`+`CardKind::Template` (Q38–40). *(resolves H3.)*
- [ ] **GUI-5: remove dead `mde-iced-components` widgets** — delete `motion`, `skeleton_shimmer`, `toast_chip`, `elevation_container`, `icon_fill_morph` (Q41–45). *(resolves H4.)*
- [ ] **GUI-6: build `mde-cosmic-applet`** — a libcosmic applet subscribing to `mde-bus`: health pip + quick actions (join/leave, DnD, transfers) + deep links into Workbench (Q46/47).
- [ ] **GUI-7: maximize-Cosmic-native cutover** — notifications via Cosmic's daemon, mde-files chrome reskinned to libcosmic, panel hosted by Cosmic (Q43/51).
- [ ] **GUI-8: density boot-apply** — read `theme.density` at boot and apply app-wide (Q50).
- [ ] **GUI-9: reduced-motion from Cosmic** — source the reduce-motion flag from Cosmic's a11y setting (Q49).
- [ ] **GUI-10: refactor `palette.rs` onto the carbon ramp** — `dark()/light()` reference `carbon::*` so the ramp is the sole hex source (Q52). *(closes the H2 follow-up.)*

## SERVICES — Remote Access, music, voice, files, KDC (resolves B1, H6)

- [ ] **SVC-1: Remote Access panel** — fold a per-peer SSH status+launcher into `remote_desktop` (SSH+RDP+VNC); drop the `mesh_ssh` nav entry; launch via `.remmina`, reuse remmina probes, hostname targets, show local+remote sshd state, no ACL (Q53–62). *(resolves B1.)*
- [ ] **SVC-2: SSH pubkey-gossip worker** — a `mackesd` worker gossips each peer's mesh ed25519 pubkey into every peer's `authorized_keys` (Q60).
- [ ] **SVC-3: build `list-radio`** — Airsonic `getInternetRadioStations` client + `list-radio` verb + `verb_for(Radio)`; play = enqueue the stream URL as a pseudo-track (Q63/64). *(resolves H6.)*
- [ ] **SVC-4: voice HUD promotion** — Cosmic autostart for `--agent` + Workbench presence; Bus-native presence (every peer publishes `state/voice/status`) (Q65/66).
- [ ] **SVC-5: document the 3 file bridges** — keep mesh / SMB / KDC co-equal in mde-files (Q67); no code change, just the lock.
- [ ] **SVC-6: KDC full phone hub** — land KDC2-4 (= SEC-5), keep all plugins, phone actions on the device card only (Q68/69).
- [ ] **SVC-7: Workstation-only service gating** — gate music/voice/files/KDC to Workstation rank; Servers/Lighthouses run plumbing only (Q70).

## PKG — one RPM, role chooser, COPR, ISO (the unbuilt §5)

- [ ] **PKG-1: monolithic RPM** — cargo-generate-rpm metadata → one `magic-mesh` RPM carrying all 8 bins (Q71/72/76).
- [ ] **PKG-2: `packaging/` dir** — a top-level non-crate dir for the spec/metadata, units, `.ks`, `.repo` (Q85).
- [ ] **PKG-3: self-gating `mackesd.service`** — one service that gates its in-process workers via `resolve_rank()`; the RPM enables nothing role-specific (Q75/86) + app surface units.
- [ ] **PKG-4: `mackesd role pin` subcommand** — the CLI front-end for `mde_role::pin` (Q74).
- [ ] **PKG-5: install-time role chooser** — a Cosmic first-run GUI chooser (Q73) + a kickstart `%post` inline path (Q81) + an "init-new-mesh vs join-existing" prompt (Q84).
- [ ] **PKG-6: DISCLAIMER gate** — build refuses to package without it (build.rs/release) AND a mandatory install-time accept screen (Q82).
- [ ] **PKG-7: upgrade-only enforcement** — refuse downgrade at both the RPM scriptlet and `mde_role::pin`; upgrade is unit-only re-pin + reload (Q77/78).
- [ ] **PKG-8: signed COPR** — COPR built-in per-project GPG; ship the pubkey + a `magic-mesh-release.rpm` (Q79).
- [ ] **PKG-9: Magic-on-Cosmic ISO** — a Fedora-Cosmic kickstart built with livemedia-creator (Q80).
- [ ] **PKG-10: post-install enrollment** — `mackesd enroll --token` documented as the post-install step (Q83).

## TEST-OBS — testing/CI + observability (resolves E1)

- [ ] **OBS-1: retarget integration tests to Nebula** — real `nebula-lighthouse` + 2 peer containers via testcontainers; assert overlay reachability + handshake; daemon-absent skip = hard fail (Q87–89). *(resolves E1.)*
- [ ] **OBS-2: multi-process convergence harness** — N real `mackesd` binaries over one QNM root assert newest-wins + single leader (Q91).
- [ ] **OBS-3: GitHub Actions CI** — hosted runners; the §7 gates (build/test/clippy/fmt + boundary/Carbon/Nebula lints) + a hard 80% line-coverage floor (Q90/93).
- [ ] **OBS-4: screenshot-artifact visual regression** — a scripted `/preview` capture posting screenshots as CI artifacts for human review (Q92).
- [ ] **OBS-5: mesh-replicated structured logging** — each peer writes a structured log into QNM-Shared; any peer can read any peer's recent trace (Q94).
- [ ] **OBS-6: Mesh Health Workbench panel** — union the per-peer reachability + alert signals (no central metrics aggregation; Netdata stays local, Q95/96).
- [ ] **OBS-7: upgrade-transition alerts** — `alert_relay` emits each upgrade-state transition as a desktop alert (Q97).
- [ ] **OBS-8: alerts via the cosmic-applet** — deliver through the mde-bus → cosmic-applet FDO Notifications path instead of `notify-send` (Q100).

## ENTERPRISE — operability + security-enforcement gaps (from the enterprise-readiness verification)

> Source: `docs/design/enterprise-readiness.md` (verdict: **prototype with enterprise direction**).
> These are the enterprise-specific gaps the survey epics did NOT capture. **ENT-1/2/3 are CRITICAL.**
> Overlaps: installation/packaging/role-chooser = the **PKG** epic; CI/observability = **OBS**;
> control plane = **FLEET-PHASE-G**. The minimum bar for honestly claiming "enterprise-grade" is
> PKG + ENT-1, 2, 3, 5, 6, 7, 8, 9, 12 + OBS CI.

- [ ] **ENT-1: enforce the enrollment bearer (CRITICAL — security)** — `sign_pending_csr` (`nebula_enroll.rs:571`) + `nebula_csr_watcher` sign any well-formed CSR and **never check the bearer/passcode** against an issued list, though the docs claim they do. Maintain an issued-but-unredeemed allow-list (single-use bearers); refuse a CSR whose bearer isn't pending-issued. **Acceptance:** an enroll with a wrong/replayed/absent bearer is refused (test); a valid single-use bearer signs once then can't be reused.
- [ ] **ENT-2: pin `role.toml` at provision (CRITICAL)** — `mde_role::pin_at` is lib-only; nothing writes `/var/lib/mde/role.toml`, so every box runs unpinned→Workstation. Add `mackesd role pin <role>` (= PKG-4) and have the installer/chooser call it. **Acceptance:** a Server install gates to rank-1 workers (`mackesd role-workers` matches); downgrade refused.
- [ ] **ENT-3: revocation evicts the data plane (CRITICAL — security)** — `ca revoke` only marks the DB + ban list + a bus event; the Nebula data plane keeps trusting the cert until expiry. Push a Nebula `pki.blocklist` (or equivalent) to running nebula + reload on revoke. **Acceptance:** a revoked node can no longer reach any peer within N seconds (integration test).
- [ ] **ENT-4: `mackesd mesh init`** — one-command Lighthouse bootstrap: mint CA + self-sign the lighthouse peer cert + overlay IP + pin role Lighthouse + start nebula + print a join token. **Acceptance:** on a clean box, one command yields a working CA-signing lighthouse + a token a peer enrolls with.
- [ ] **ENT-5: unify `mackesd leave` / decommission** — today `decommission` (DB soft-delete) and `ca revoke` (trust) are uncoordinated and neither tears down local state. One verb that revokes + bans + wipes local `/etc/nebula/`, keys, and role. **Acceptance:** after `leave`, the node holds no valid cert and is gone from the roster; re-enroll is a clean fresh join.
- [ ] **ENT-6: `mackesd.service` + supervisor hardening** — no systemd unit means nothing restarts mackesd on crash; the worker supervisor is a 250 ms fixed-retry stub (`workers/mod.rs:430`, no max-restarts/circuit-breaker). Ship the unit (Restart=on-failure) (= PKG-3) + bounded exponential back-off + circuit-breaker + max-restarts. **Acceptance:** `kill -9 mackesd` → restarted ≤ N s; a hot-looping worker trips the breaker instead of spinning at 250 ms forever.
- [ ] **ENT-7: `mackesd doctor`** — a unified self-test (identity present, role pinned, nebula up, peers reachable, storage mounted, services healthy) with clear pass/fail per check. **Acceptance:** `mackesd doctor` on a healthy node exits 0 with all-green; a broken node names the failed check.
- [ ] **ENT-8: `mackesd fleet status`** — a whole-fleet view any node can produce (peers + versions + leader + health), not just per-peer. **Acceptance:** on any node, prints every peer Online/Offline with version + the elected leader.
- [ ] **ENT-9: `mackesd logs` + fix the GUI Logs panel** — no `logs` verb; the workbench Logs panel reads dead desktop paths (`mackes-shell`/sway). Add `mackesd logs [--since]` over journald/tracing and point the panel at mackesd's output. **Acceptance:** `mackesd logs --since 1h` returns current structured logs; the GUI panel shows them.
- [ ] **ENT-10: `mackesd test connectivity`** — a peer-to-peer connectivity self-test (overlay reachability per peer), distinct from the scattered LAN probes. **Acceptance:** prints reachable/unreachable per peer over the overlay.
- [ ] **ENT-11: DR backup hardening** — move `MDE_BACKUP_PASSPHRASE` off the systemd env into systemd-creds; add a multi-copy / off-mesh backup option (the sealed CA bundle is currently single-copy on QNM-Shared, replicated to every node). **Acceptance:** passphrase not visible via `systemctl show`; a documented restore works from an off-mesh copy.
- [ ] **ENT-12: operator + end-user documentation** — install guide, per-node-type setup guide, troubleshooting guide, and a DR runbook (the code points at a missing `docs/help/mesh-recovery.md`). Fix the stale `DISCLAIMER.md` ("Mackes Workstation" → Magic Mesh; revisit "not for production"). **Acceptance:** a new admin provisions all 3 node types + recovers a dead lighthouse using only the docs.
- [ ] **ENT-13: replace the `mesh_latency` ping placeholder** — `mesh_latency.rs:10` shells `ping` as an admitted placeholder pending the transport handshake. Use the real transport RTT probe. **Acceptance:** latency reflects the overlay path, not ICMP.
- [ ] **ENT-14: security-event audit** — enroll/sign/revoke/rotate go to `tracing` only; append them to the hash-chained `events` table and wire the KDC `.also_log` no-op (`dispatch.rs:116`). **Acceptance:** `mackesd events list` shows enroll/sign/revoke records; `audit-verify` covers them.

---

*Audit (sweeps 1–2): 18 findings, A1–H8. **8 shipped** (H1 §3, D1/H5/H2 §4, F1–F3/H7 §5-doc, G1 §1, A1/A2 deletion). The 7 open findings are now **specified** by the survey and resolve into the epics above.*
*Survey (2026-06-09): 100/100 answered → 6 epics, 51 tasks. Packaging (PKG-*) is held until every feature is §7-complete; releasing is operator-gated.*
*Enterprise-readiness verification (2026-06-09): verdict **prototype with enterprise direction**; +14 ENTERPRISE tasks (ENT-1/2/3 CRITICAL). Full report: `docs/design/enterprise-readiness.md`.*
