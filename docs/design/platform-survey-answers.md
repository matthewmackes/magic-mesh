# Platform Survey — Locked Answers

Running record of operator answers to `platform-survey.md` (2026-06-09). Each section's locks are
synthesised into an architecture note, then lifted into `docs/WORKLIST.md` as an epic when the
survey completes.

## §1 — Mesh Substrate & Fleet Control Plane (Phase-G) — COMPLETE

| Q | Lock | Q | Lock |
|---|------|---|------|
| Q1 | **A** revision id = magic-fleet u64 version | Q10 | **B** apply = host-local Ansible |
| Q2 | **A** wire format = YAML | Q11 | **B** LizardFS master = Lighthouse-pinned |
| Q3 | **C** transport = via LizardFS replication | Q12 | **A** replication goal 2 default |
| Q4 | **A** conflict = last-writer-wins (version order) | Q13 | **A** mount owns five XDG dirs (bind) |
| Q5 | **A** leaderless authoring | Q14 | **A** apply success = gossip ack |
| Q6 | **A** rollback = mint higher-version copy | Q15 | **A** event/fleet/signals topic |
| Q7 | **A** diff = flat top-level | Q16 | **B** list-revisions = full held set |
| Q8 | **C** history = LizardFS append-only log | Q17 | **A** auth = Nebula transport (author advisory) |
| Q9 | **B** fold settings into BaselineSpec | Q18 | **A** cold node applies newest, lazy back-fill |

**Architecture (FLEET-PHASE-G epic):** Fleet desired-state is one unified `BaselineSpec` (OS state
+ folded-in settings, Q9) serialised as YAML (Q2) with a monotonic `u64 version` id (Q1). Revisions
are written into LizardFS mesh storage, which is both the transport (replication carries them, Q3)
and the authoritative append-only log (Q8); any node authors leaderlessly (Q5) and the highest
`version` wins deterministically (Q4). Apply is host-local Ansible (Q10). Rollback mints a new
higher-version copy of the target (Q6); diff is flat top-level (Q7); `list-revisions` returns the
full held set tagged with the elected winner (Q16). Nodes gossip apply-acks (Q14) which advance the
author's lifecycle FSM to Verified and emit on `event/fleet/signals` (Q15) for the Workbench. Auth
rests on the Nebula transport — only enrolled peers gossip; the `author` field is advisory (Q17).
Cold/partitioned nodes apply the newest revision immediately and back-fill history lazily (Q18).
The LizardFS master is pinned to Lighthouse-role nodes (Q11) at default replication goal 2 (Q12);
the mount owns the five XDG dirs by bind-redirect, never `~/Local/` (Q13).

**Lifts to worklist tasks:** FPG-1 unify revision model · FPG-2 LizardFS revision log/store ·
FPG-3 leaderless election · FPG-4 the push/list/diff/rollback Bus verbs (replace stubs) ·
FPG-5 apply-ack + event/fleet/signals + Workbench subscription + Verified FSM · FPG-6 cold-node
convergence · FPG-7 LizardFS mount ownership (5 XDG dirs, goal 2, Lighthouse master) · FPG-8
host-local Ansible apply of the unified baseline.

## §2 — Security, KDC, Enrollment & Crypto — COMPLETE

| Q | Lock | Q | Lock |
|---|------|---|------|
| Q19 | **B** peer certs non-expiring | Q27 | **A** relayer trust = accept any (pinning is the gate) |
| Q20 | **C** CA rotate needs operator passphrase | Q28 | **B** revocation = gossiped signed records |
| Q21 | **A** enrollment = auto-sign (TOFU, open-mesh) | Q29 | **A** ban list = per-node files |
| Q22 | **C** passcode = QR/file 256-bit token | Q30 | **A** CA key at rest = FS perms only |
| Q23 | **A** KDC identity = keep RSA-4096 | Q31 | **B** CA backup mandatory on lighthouse |
| Q24 | **A** first-pair = keep TOFU | Q32 | **A** backup = one combined bundle |
| Q25 | **A** pinning = outbound first-pair flow | Q33 | **A** KDC session AEAD = AES-256-GCM |
| Q26 | **A** build the KDC2-4 mesh-shunt worker (resolves H8) | Q34 | **B** persist KDC session keys encrypted |

**Architecture (SECURITY epic):** Enrollment stays open-mesh and low-friction — `nebula_csr_watcher`
auto-signs (TOFU) on a high-entropy QR/file-delivered 256-bit token (Q21, Q22). Peer certs are
effectively non-expiring (Q19); turnover happens via CA rotation (gated on an operator passphrase,
Q20) and revocation, which now gossips a signed retract record peer-to-peer (Q28) alongside the
durable per-node ban files (Q29). The CA private key stays sealed by 0600 + owner-uid (Q30), but CA
backup becomes mandatory on the lighthouse role — refuse-start / loud-warn without a passphrase
(Q31) — in one combined CA+topology bundle (Q32). The KDC keeps RSA-4096 for stock KDE-Connect
interop (Q23) and TOFU fingerprint-pinning (Q24), completed by a new operator-initiated outbound
first-pair flow that writes the pin (Q25). The KDC2-4 mesh-shunt worker is built — consuming
`SyntheticAnnounce`/`inject_synthetic`, accepting any relayer since pinning is the real gate (Q26,
Q27), resolving worklist H8. KDC sessions stay AES-256-GCM (Q33) but session keys are now persisted
encrypted-at-rest so links survive a daemon restart (Q34).

**Lifts to worklist tasks:** SEC-1 non-expiring peer certs · SEC-2 passphrase-gated CA rotation ·
SEC-3 QR/file 256-bit enrollment token · SEC-4 outbound first-pair flow (writes the pin) ·
SEC-5 KDC2-4 mesh-shunt worker (resolves H8) · SEC-6 gossiped signed revocation records ·
SEC-7 mandatory CA backup on lighthouse · SEC-8 encrypt KDC session keys at rest.

## §3 — Carbon GUIs, UX & Component Library — COMPLETE

| Q | Lock | Q | Lock |
|---|------|---|------|
| Q35 | **A** add Gray 90 as a third theme variant | Q44 | **A** remove elevation_container (H4) |
| Q36 | **A** live-repaint theme switching | Q45 | **A** delete icon_fill_morph (H4) |
| Q37 | **A** Carbon-rewrite the Themes panel | Q46 | **A** build mde-cosmic-applet (libcosmic) |
| Q38 | **A** remove card migration module (H3) | Q47 | **B** applet scope = status + quick actions |
| Q39 | **A** delete RenderMode (H3) | Q48 | *not a concern* — colorblind a11y, no task |
| Q40 | **A** delete TemplateSpec (H3) | Q49 | **B** reduced-motion sourced from Cosmic |
| Q41 | **A** remove motion module (H4) | Q50 | **B** density boot + relaunch |
| Q42 | **A** remove skeleton_shimmer (H4) | Q51 | **C** maximize Cosmic-native cutover |
| Q43 | **A** delete toast_chip; Cosmic owns notifications (H4) | Q52 | **A** refactor palette.rs onto the carbon ramp |

**Architecture (GUI epic):** The Carbon look gets the full three-theme set — add a `Theme::Gray90`
+ `Palette::gray_90()` (Q35) — with live in-app repaint on theme change (Q36) and a rewritten
Themes panel offering exactly Gray 10/90/100 through the `mde-theme` preference store, dropping the
retired presets + gsettings shell-out (Q37); density is read at boot and applied app-wide (Q50).
**H3 and H4 resolve to clean removal** — the dead `mde-card` surfaces (migration, RenderMode,
TemplateSpec; Q38–40) and the dead `mde-iced-components` widgets (motion, skeleton_shimmer,
toast_chip, elevation_container, icon_fill_morph; Q41–45) are all deleted rather than wired, since
none has a real Cosmic-era home. The Bus→Cosmic bridge is built as a real **libcosmic
`mde-cosmic-applet` crate** (Q46) scoped to a health pip + quick actions (join/leave, DnD,
transfer count) with deep links into the Workbench (Q47). The v1 cutover **maximizes Cosmic-native**
(Q51): notifications via Cosmic's daemon (Q43), mde-files' chrome reskinned to libcosmic, the panel
hosted by Cosmic. Reduced-motion is sourced from Cosmic's a11y setting (Q49); `palette.rs` is
refactored to reference the single-sourced `carbon` ramp (Q52, closing the H2 follow-up). The
colorblind-accent/§4 tension is explicitly out of scope (Q48).

**Lifts to worklist tasks:** GUI-1 add Gray 90 theme · GUI-2 live theme switching · GUI-3 Carbon
Themes-panel rewrite · GUI-4 remove H3 dead card surfaces · GUI-5 remove H4 dead widgets ·
GUI-6 build mde-cosmic-applet (libcosmic) · GUI-7 maximize-Cosmic-native cutover (mde-files chrome +
panel + notifications) · GUI-8 density boot-apply · GUI-9 reduced-motion from Cosmic · GUI-10
refactor palette.rs onto the carbon ramp (closes H2 follow-up).

## §4 — Mesh SSH, Remote Access & Services — COMPLETE

| Q | Lock | Q | Lock |
|---|------|---|------|
| Q53 | **A** Mesh SSH = per-peer status + launcher | Q62 | **B** B1 first slice = status + launcher |
| Q54 | **B** launch via the .remmina SSH profile | Q63 | **A** build list-radio (resolves H6) |
| Q55 | **B** merge into Remote Desktop → "Remote Access" | Q64 | **A** radio = enqueue stream URL |
| Q56 | **A** stays under Network | Q65 | **A** voice = keep + promote (Cosmic autostart) |
| Q57 | **A** SSH target = hostname/mesh-DNS | Q66 | **A** SIP presence = Bus-native |
| Q58 | **A** no ACL (flat-trust open-mesh) | Q67 | **B** keep all three file bridges (mesh/SMB/KDC) |
| Q59 | **C** show both local sshd + remote peers | Q68 | **A** KDC = full phone hub (lands KDC2-4) |
| Q60 | **A** worker gossips mesh pubkeys → authorized_keys | Q69 | **A** phone actions = peer-card only |
| Q61 | **A** reuse remmina_sync probes | Q70 | **A** services Workstation-only |

**Architecture (SERVICES epic):** Worklist B1 resolves to a renamed **"Remote Access"** panel:
fold a per-peer SSH status+launcher into the existing `remote_desktop` panel (SSH+RDP+VNC), drop
the standalone `mesh_ssh` nav entry (Q53/55), launching via the `.remmina` SSH profiles that
`remmina_sync` already maintains (Q54) and reusing its probe results (Q61); SSH targets the peer
hostname via mesh DNS (Q57), honors flat-trust with no per-peer ACL (Q58), and shows both this
peer's own sshd reachability and the remote launch list (Q59), under Network (Q56). A new worker
gossips each peer's mesh ed25519 pubkey into every peer's `authorized_keys` for auto-trust within
the enrolled mesh (Q60); the first shippable slice is status + a working launcher button (Q62).
Worklist H6 resolves to **building** the Radio card: an Airsonic `getInternetRadioStations` client
method + `list-radio` verb + `verb_for(Radio)` mapping (Q63), playing a station by enqueueing its
stream URL as a pseudo-track through the existing engine (Q64). The voice/SIP HUD is kept and
promoted to a first-class service — Cosmic autostart for `--agent` + Workbench presence (Q65) with
Bus-native presence (every peer publishes `state/voice/status`, Q66). mde-files keeps all three
remote-file bridges co-equal (mesh / SMB / KDC, Q67). KDC becomes a full phone hub — land the
KDC2-4 mesh-shunt (Q68, same worker as SEC-5) — with phone actions surfaced on the device card
only, not a Workbench panel (Q69). All user-facing services are Workstation-only; Servers and
Lighthouses run `sshd_overlay_bind` + mesh plumbing only (Q70).

**Lifts to worklist tasks:** SVC-1 Remote Access panel — fold SSH in, drop mesh_ssh entry (resolves
B1) · SVC-2 SSH-pubkey gossip worker · SVC-3 list-radio (Airsonic + verb + enqueue URL; resolves
H6) · SVC-4 voice HUD Cosmic-autostart + Bus-native presence · SVC-5 document the 3 file bridges as
co-equal · SVC-6 KDC full phone hub (overlaps SEC-5/KDC2-4) · SVC-7 Workstation-only service role
gating.

## §5 — Deployment, Roles & Packaging — COMPLETE

| Q | Lock | Q | Lock |
|---|------|---|------|
| Q71 | **A** RPM tool = cargo-generate-rpm | Q79 | **A** COPR built-in per-project GPG signing |
| Q72 | **A** one monolithic magic-mesh RPM | Q80 | **A** ISO = kickstart + livemedia-creator |
| Q73 | **D** chooser = Cosmic first-run GUI | Q81 | **A** role chosen inline in Anaconda %post |
| Q74 | **B** pin via `mackesd role pin` | Q82 | **C** disclaimer gate at build + install |
| Q75 | **A** single self-gating mackesd.service | Q83 | **A** Nebula enrollment deferred post-install |
| Q76 | **A** all bins ship on every role | Q84 | **B** separate init-vs-join-mesh prompt |
| Q77 | **C** unit-only upgrade (re-pin + reload) | Q85 | **A** metadata in a top-level packaging/ dir |
| Q78 | **C** downgrade refused: scriptlet + pin lib | Q86 | **A** RPM enables nothing role-specific (self-gates) |

**Architecture (PKG epic):** One monolithic `magic-mesh` RPM built via cargo-generate-rpm metadata
(Q71/72) ships all 8 binaries on every box; a single self-gating `mackesd.service` reads `role.toml`
and gates its in-process workers via `resolve_rank()`, so the RPM enables nothing role-specific
(Q75/76/86). The role is pinned through a `mackesd role pin` subcommand (Q74); the operator-facing
chooser is a Cosmic first-run GUI (Q73) — with the role also settable inline via a kickstart %post
during Anaconda (Q81) (reconcile: Anaconda-inline is the automated/headless path, the Cosmic GUI
handles interactive desktop first-run / unpinned boxes), plus a separate "initialize new mesh vs
join existing" prompt orthogonal to role (Q84). Upgrades are unit-only — re-pin + daemon reload,
all bins already present (Q77) — and downgrade is refused at both the RPM scriptlet and the
`mde_role::pin` lib (Q78). Distribution: a signed COPR via COPR's built-in per-project GPG, shipping
the pubkey + a `magic-mesh-release.rpm` (Q79), and a Magic-on-Cosmic ISO built from a Fedora-Cosmic
kickstart with livemedia-creator (Q80). The DISCLAIMER.md gate binds at both build (refuse to
package without it) and install (mandatory accept screen) (Q82). Nebula enrollment is deferred to
post-install via `mackesd enroll --token` (Q83). All packaging metadata, units, .ks, and .repo live
in a new top-level `packaging/` dir (Q85).

**Lifts to worklist tasks:** PKG-1 cargo-generate-rpm monolithic RPM · PKG-2 packaging/ dir scaffold ·
PKG-3 self-gating mackesd.service + app surface units · PKG-4 `mackesd role pin` subcommand · PKG-5
Cosmic first-run GUI chooser + Anaconda %post + init-vs-join prompt · PKG-6 disclaimer build+install
gate · PKG-7 upgrade-only enforcement (scriptlet + lib) · PKG-8 signed COPR · PKG-9 Magic-on-Cosmic
ISO (kickstart + livemedia-creator) · PKG-10 post-install enrollment flow.

## §6 — Testing/CI, Observability & Lifecycle — COMPLETE

| Q | Lock | Q | Lock |
|---|------|---|------|
| Q87 | **A** E1 = retarget to real Nebula containers | Q94 | **D** logs mesh-replicated into QNM-Shared |
| Q88 | **A** fixture = testcontainers (swap images) | Q95 | **D** no central metrics agg (per-peer local) |
| Q89 | **A** daemon-absent skip = hard failure | Q96 | **C** dedicated Mesh Health GUI panel |
| Q90 | **A** CI = GitHub Actions (hosted) | Q97 | **C** upgrade transitions as desktop alerts |
| Q91 | **A** multi-process convergence harness | Q98 | **A** keep current backup scope/cadence |
| Q92 | **B** visual = screenshot artifacts (human review) | Q99 | **A** backups in QNM-Shared via replication |
| Q93 | **A** CI gate adds hard 80% coverage floor | Q100 | **B** alerts via the Cosmic applet (FDO host) |

**Architecture (TEST-OBS epic):** Worklist E1 resolves to **retargeting** `integration_testcontainers.rs`
to a real Nebula topology — a `nebula-lighthouse` + 2 peer containers via the existing
`testcontainers` crate (swap the images), asserting overlay reachability + cert handshake end-to-end
(Q87/88); a daemon-absent skip becomes a hard failure, not a silent pass (Q89). CI standardises on
GitHub-hosted Actions (Q90). The no-fixed-center gossip/reconcile loops get a multi-process
convergence harness — N real `mackesd` binaries over one QNM root asserting newest-wins + single
leader (Q91). Visual regression for the Carbon GUIs is a scripted `/preview` capture posting
screenshots as CI artifacts for human review, not an automated pixel gate (Q92); the CI quality bar
keeps the §7 gates (build/test/clippy/fmt + boundary/Carbon/Nebula lints) and adds a hard 80%
line-coverage floor (Q93). Logging is mesh-replicated: each peer writes a structured log file into
QNM-Shared so any peer can read any peer's recent trace (Q94). Observability stays decentralised —
no central metrics aggregator, each peer's Netdata is local-only (Q95) — but a dedicated Mesh Health
Workbench panel unions the lightweight per-peer reachability + alert signals (Q96). Fleet upgrade
transitions are surfaced as desktop alerts via `alert_relay` (Q97). CA/state backup keeps its
current scope (CA + LizardFS topology snapshot, 24h, mandatory on lighthouse) (Q98), stored in
QNM-Shared and carried by LizardFS replication so every peer holds a copy (Q99). Alerts are
delivered through the mde-bus → cosmic-applet FDO Notifications path rather than shelling
`notify-send` (Q100, coheres with §3 Q43/Q51).

**Lifts to worklist tasks:** OBS-1 retarget integration tests to Nebula containers (resolves E1) ·
OBS-2 multi-process convergence harness · OBS-3 GitHub Actions CI (gates + 80% coverage floor) ·
OBS-4 screenshot-artifact visual regression · OBS-5 mesh-replicated structured logging · OBS-6 Mesh
Health Workbench panel · OBS-7 upgrade-transition alerts · OBS-8 alerts via the cosmic-applet FDO
path.

---

## Survey complete — 100/100 answered (2026-06-09)

Six epics specified for the worklist lift: **FLEET-PHASE-G** (8 tasks) · **SECURITY** (8) ·
**GUI** (10) · **SERVICES** (7) · **PKG** (10) · **TEST-OBS** (8) = 51 user-story tasks.
Resolves the under-specified worklist findings: **C1** (Phase-G → FLEET-PHASE-G), **B1** (Mesh SSH →
SVC-1, fold into Remote Access), **H6** (Radio → SVC-3, build), **H8** (KDC seam → SEC-5, build),
**H3/H4** (→ GUI-4/5, remove all), **E1** (→ OBS-1, retarget to Nebula), plus the H2 palette
follow-up (→ GUI-10).
