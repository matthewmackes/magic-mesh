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

## P8 — Sweep-3 findings (2026-06-09, post-execution re-sweep)

- [✓] **I1 · voice-hud recents → REMOVE (operator decision 2026-06-09)** — done — recents.rs deleted, notification is the record, "Magic Mesh Voice" rename (incl. SIP UA/SDP). — recent calls are documented as a **system notification, not a stored recents list**. The notification already exists (`main.rs:210–212` fires `notify-send "Incoming call"` on every incoming call), so: delete `recents.rs` wholesale (`record_incoming`/`load`/`RECENTS_LIMIT` + the on-disk history) and the `main.rs:213` call. While there: the notify-send app-name is the stale "Mackes Workstation Voice" and its comment cites the retired "mde's notifyd → Action Center" path — rename to Magic Mesh Voice / Cosmic's notification daemon. Acceptance: incoming call → desktop notification; no call history written anywhere.
- [✓] **I2–I4 · voice-hud dead-code cluster (46 warnings)** — done — REGISTER trio + 27 dead theme consts + tok_a + RosterSource::label + dead AgentCommand::Shutdown removed; voice-hud warning-free. — REMOVE: the superseded standalone-socket REGISTER trio (`sip.rs:368/402/412` — live path is the agent-socket REGISTER at `sip.rs:1028`), ~27 H2-residue `theme.rs` constants + `tok_a`, and `RosterSource::label()` (`roster.rs:79`). Acceptance: `cargo check -p mde-voice-hud` warning-free (minus I1's pieces if kept).
- [✓] **I5 · mackes-mesh-types Tailscale doc-drift** — done — Nebula-correct peer docs. — `lib.rs:67,72` claim "Headscale-known machine" / "Tailscale-assigned 100.x.x.x" as current; fabric is Nebula (10.42.x.x, own CA). FINISH (fix docs).
- [✓] **I6+I7 · document the §3 MD5 interop exceptions** — done — §3 documented-exception note added. — `airsonic.rs:40` (Subsonic API auth-token scheme; require TLS to the server) + `thumbnails.rs:73` (freedesktop thumbnail-spec cache key, non-crypto). FINISH: add a short documented-exception note to `AI_GOVERNANCE.md` §3 so future sweeps don't re-litigate.
- [✓] **I8 · `mde-music` `TransportDone(Result<…>)` dead field** — done — unit variant. — inner `Result` never read; REMOVE (unit variant).
- [✓] **I9 · workbench `Noop` stale comment** — done — comment now describes the functional fallback. — `app.rs:215–217` says "placeholder for buttons" but all live uses are functional fallbacks; FINISH (fix comment).

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

- [✓] **GUI-1: add Gray 90 theme** — done — Theme::Gray90 + Palette::gray_90() (published g90 mapping, pin-tested); `Theme::Gray90` + `Palette::gray_90()`, the full 3-theme set §4 names (Q35).
- [✓] **GUI-2: live theme switching** — done — live_theme module (RwLock<Tokens>); 93 Palette::dark() sites now read the live palette; swap repaints without restart; thread the resolved `Palette` through `App` state so a theme change repaints live (Q36).
- [✓] **GUI-3: Carbon Themes-panel rewrite** — done — exactly Gray 10/90/100 + density via mde_theme::Preferences (load/save); GTK fields, ChromeOS/Ableton presets, gsettings path all deleted; offer exactly Gray 10/90/100 via the mde-theme pref store; drop the retired presets + gsettings shell-out (Q37).
- [✓] **GUI-4: remove dead `mde-card` surfaces** — done — migration mod + RenderMode + TemplateSpec + CardKind::Template (sway-era) deleted; SCHEMA_VERSION relocated to schema.rs (live wire field); delete `migration`, `RenderMode`, `TemplateSpec`+`CardKind::Template` (Q38–40). *(resolves H3.)*
- [✓] **GUI-5: remove dead `mde-iced-components` widgets** — done — 457 lines cut; motion slimmed to a private 2-fn module (context_menu_surface deps); crate warning-free; delete `motion`, `skeleton_shimmer`, `toast_chip`, `elevation_container`, `icon_fill_morph` (Q41–45). *(resolves H4.)*
- [ ] **GUI-6: build `mde-cosmic-applet`** — a libcosmic applet subscribing to `mde-bus`: health pip + quick actions (join/leave, DnD, transfers) + deep links into Workbench (Q46/47).
- [ ] **GUI-7: maximize-Cosmic-native cutover** — notifications via Cosmic's daemon, mde-files chrome reskinned to libcosmic, panel hosted by Cosmic (Q43/51).
- [✓] **GUI-8: density boot-apply** — done — density resolves from preferences.toml at boot (live_theme Tokens) and all render-path Density::Comfortable sites now read the live density; read `theme.density` at boot and apply app-wide (Q50).
- [!] **GUI-9: reduced-motion from Cosmic** — BLOCKED upstream (2026-06-09): Cosmic has no reduce-motion/animations setting (cosmic-comp#376 open) and the FDO portal appearance namespace lacks the key; nothing exists to read. Local coverage: MDE_REDUCE_MOTION + prefs a11y.reduce_motion. Revisit when cosmic-comp lands the setting. Original: source the reduce-motion flag from Cosmic's a11y setting (Q49).
- [✓] **GUI-10: refactor `palette.rs` onto the carbon ramp** — done — zero raw hex in palette.rs; GRAY_10_HOVER added to the ramp (test-pinned); `dark()/light()` reference `carbon::*` so the ramp is the sole hex source (Q52). *(closes the H2 follow-up.)*

## SERVICES — Remote Access, music, voice, files, KDC (resolves B1, H6)

- [✓] **SVC-1: Remote Access panel** — done — SSH folded into remote_desktop (cosmic-term `ssh $USER@host` per L7, port-22 probe per row, local sshd state in header); mesh_ssh nav entry dropped, slug aliased; B1 closed; fold a per-peer SSH status+launcher into `remote_desktop` (SSH+RDP+VNC); drop the `mesh_ssh` nav entry; launch via `.remmina`, reuse remmina probes, hostname targets, show local+remote sshd state, no ACL (Q53–62). *(resolves B1.)*
- [✓] **SVC-2: SSH pubkey-gossip worker** — done — ssh_pubkey_gossip worker (rank 0): publishes ~/.ssh/id_ed25519.pub to <root>/ssh-keys/, merges all peers into an authorized_keys managed block; a `mackesd` worker gossips each peer's mesh ed25519 pubkey into every peer's `authorized_keys` (Q60).
- [✓] **SVC-3: build `list-radio`** — done — getInternetRadioStations client + verb + verb_for(Radio) + URL-passthrough stream ids; click plays the station; Airsonic `getInternetRadioStations` client + `list-radio` verb + `verb_for(Radio)`; play = enqueue the stream URL as a pseudo-track (Q63/64). *(resolves H6.)*
- [ ] **SVC-4: voice HUD promotion** — Cosmic autostart for `--agent` + Workbench presence; Bus-native presence (every peer publishes `state/voice/status`) (Q65/66).
- [✓] **SVC-5: document the 3 file bridges** — done — co-equal lock documented in the mde-files crate doc; keep mesh / SMB / KDC co-equal in mde-files (Q67); no code change, just the lock.
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
- [→] **OBS-5: mesh-replicated structured logging** — **RE-HOMED → PLANES-14** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [→] **OBS-6: Mesh Health Workbench panel** — **RE-HOMED → PLANES-20** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [ ] **OBS-7: upgrade-transition alerts** — `alert_relay` emits each upgrade-state transition as a desktop alert (Q97).
- [ ] **OBS-8: alerts via the cosmic-applet** — deliver through the mde-bus → cosmic-applet FDO Notifications path instead of `notify-send` (Q100).

## PEERS — Directory of Mesh Peers, the platform Front Door (design: `docs/design/peer-directory.md`, 2026-06-09)

> 26-Q survey + 3 operator directives **+ 25-Q level-2 survey (L1–L25)**. The Peers directory
> is the **Front Door**: Workbench lands on it; it shows everything the running mesh offers
> (peers, services: remote access / Podman / KVM / media / voice) + an advanced health/design
> view (presence, sync, drift, Netdata health, live path map → wallpaper). Evolves
> `mesh_topology`; retires the CR-6.c modal.
> **Sequencing (L25):** PD-1/2 → PD-3/4/5 (+11/12/13) → PD-6/7 → PD-8/9 → PD-10; layer-shell
> spike runs early in parallel. Every slice independently shippable + §7-complete.

- [ ] **PD-1: PEERS — the `action/mesh/directory` Bus verb + `mackesd peers` CLI**
  **As** any directory consumer (GUI or CLI),
  **I want** one mackesd Bus verb returning the joined per-peer record (hostname, overlay IP, role, machine-presence tier, voice presence, mde_version, revision currency, drift count + last event, Netdata-derived health, service descriptors),
  **so that** every surface reads the same answer with zero GUI shell-outs (Q9/Q23).
  **Acceptance:**
    - [ ] `mde-bus call action/mesh/directory` returns all known peers incl. self, with every field above
    - [ ] `mackesd peers` prints the same record set as an aligned table; `--json` emits raw records (L24)
    - [ ] presence tiers derive from `last_seen_ms` (Online ≤2 min, Idle ≤10 min, Offline) (Q11)
- [ ] **PD-2: PEERS — peer-published service descriptors (remote access + Podman + KVM + media)**
  **As** a peer's mackesd,
  **I want** to locally probe sshd/xrdp/vnc listeners, Podman containers (name+image+state+published ports, L10), libvirt guests (name+state+vCPU/mem+qemu-agent IPs, L11), media services via a localhost port-scan of a pinned list (Jellyfin 8096, Navidrome/Airsonic 4533, MPD 6600, DLNA, mde-musicd — L12), and my Netdata alarm summary (3-tier: healthy/degraded/critical, worst alarm named, L15), publishing the result on the ~30 s presence heartbeat (L13) into my replicated PeerRecord/PeerProbe,
  **so that** the directory knows what every peer offers without any remote probing (Q19/Q26c/D1).
  **Acceptance:**
    - [ ] starting/stopping sshd (or a container/VM) on a peer changes its descriptor set within one heartbeat cycle
    - [ ] a WARNING alarm flips health to degraded; a CRITICAL to critical, with the alarm named (L15)
    - [ ] the port-scan touches localhost only; the scan list is a pinned constant, never user input
    - [ ] no network probe leaves the publishing host
- [ ] **PD-3: PEERS — the master-detail Peers panel (mesh_topology evolves)**
  **As** an operator,
  **I want** the topology panel reborn as "Peers": list left (self pinned "(this machine)" → Online → Offline grayed → Devices group from `action/connect/devices`) with hostname + colored tag chips (L1) and a type-to-filter box matching hostname/tag/service (L2); detail pane right (identity header + role badge, two presence fields, version + sync currency, drift count + last event → pre-filtered Drift panel link, Services Provided section, inline result strip); Bus-subscription refresh + 30 s poll floor; guided empty states when the mesh isn't fully running (unenrolled → "Join a mesh", mackesd down → one-click "Start the mesh service", no peers → "Invite a peer", L3); device rows carry presence+battery, Ring, Send-file, and a jump to the KDC hub card (L6),
  **so that** the whole fleet and everything it offers is one surface (Q1–7, Q10–12, Q15, Q20–22, D1, L1–L3, L6).
  **Acceptance:**
    - [ ] every known peer renders in the correct group; offline peers show ops disabled
    - [ ] Podman containers, KVM guests (with state/specs/IPs), and media services list per peer
    - [ ] typing in the filter narrows by hostname, tag, or offered service
    - [ ] each degraded mesh state shows its guided CTA and the CTA works
    - [ ] the CR-6.c graph modal is deleted; graph-node click selects the peer in the directory
- [ ] **PD-4: PEERS — Front Door: Workbench lands on Peers**
  **As** a user launching the Workbench,
  **I want** Peers as the default landing panel and first nav entry,
  **so that** the platform's front door shows what the mesh offers and its health (D2).
  **Acceptance:**
    - [ ] `mde-workbench` with no `--focus` opens Peers; nav lists Peers first; Overview remains reachable
- [ ] **PD-5: PEERS — per-peer ops wiring (Call / SSH / RDP / VNC) via a shared launcher engine**
  **As** an operator on a selected peer,
  **I want** Call → `action/voice/dial {peer}` (HUD pops), SSH → cosmic-term `ssh $USER@<overlay-ip>` (L7), RDP/VNC → remmina via a launcher module extracted from `remote_desktop.rs` and shared with the Remote Access panel; buttons gated by descriptors + presence (Call additionally by voice presence); no permission gate — desktop = operator (L8),
  **so that** every reach-this-peer gesture is one click from the directory (Q8/Q17/Q18/Q19).
  **Acceptance:**
    - [ ] each op connects to the actual peer when offered; buttons absent/disabled when not
    - [ ] Remote Access panel still works, now through the same launcher module (no duplicated launch code)
    - [ ] launch failures land in the inline result strip (Carbon danger), not a silent no-op
- [ ] **PD-6: PEERS — transport path probe (shared with ENT-13)**
  **As** the directory and the live map,
  **I want** a transport-layer probe verb returning RTT, direct-vs-lighthouse-relay, chosen underlay endpoint, and NAT class per peer — the same implementation that replaces the `mesh_latency.rs` ping placeholder,
  **so that** one probe feeds the RTT figure, the trace card, and the map edges (Q13/Q14, ENT-13).
  **Acceptance:**
    - [ ] probing a relayed peer reports the relay path; a direct peer reports direct + endpoint
    - [ ] `mesh_latency` no longer shells ICMP `ping` (ENT-13 closes here)
- [ ] **PD-7: PEERS — the visual augmented traceroute + live map (graph view reborn)**
  **As** an operator,
  **I want** the GraphProgram canvas grown into the live mesh map — **force-directed layout** (RTT-proportional edge pull, L17), presence-styled nodes, edges styled direct/relay/unreachable with **log-scaled width + animated flow particles** from per-host Netdata throughput (L18); clicking an edge opens the augmented trace card: overlay path report + RTT + NAT class + endpoints tried + **both hosts' session RTT sparkline** (L20) + an **expandable underlay traceroute** (L19),
  **so that** the health and design of the mesh is visible at a glance (Q13/Q14/Q24/Q26b, L17–L20).
  **Acceptance:**
    - [ ] map edges visibly distinguish direct / relay / unreachable with live RTT labels; near peers cluster, relayed peers drift outward
    - [ ] edge click renders the trace card with the overlay path report, RTT sparkline, and expandable underlay hops
    - [ ] particles flow along an edge while real traffic moves between the two peers (Netdata-sourced)
    - [ ] the canvas honors the adaptive render budget (L22) — idle mesh ≈ idle CPU
- [ ] **PD-8: PEERS — Netdata in the detail pane (sparklines + deep-link)**
  **As** an operator inspecting a peer,
  **I want** live CPU/load/net/disk sparklines (60 s window, ~2 s refresh while selected, L14) pulled from that peer's own Netdata REST API (:19999 over the overlay, bound to the overlay interface only) plus a Metrics button opening its full dashboard,
  **so that** per-peer telemetry is one selection away with no central aggregation (Q26a/Q26d, Q95/96 held).
  **Acceptance:**
    - [ ] selecting an online peer renders moving sparklines within 2 s; offline peers degrade honestly
    - [ ] Metrics opens `http://<overlay-ip>:19999` in the browser
    - [ ] Netdata is reachable over the overlay but not the underlay (bind/firewall check)
- [ ] **PD-9: PEERS — update nudge (show + converge)**
  **As** an operator seeing a peer "behind N" on revision currency,
  **I want** an Apply-now button publishing a targeted reconcile nudge that hurries that peer's convergence to the existing baseline (never per-peer divergence),
  **so that** a lagging box is one click from converged (Q15/Q16; depends on FPG-4/FPG-5 — until then the field reads "unknown" honestly, no fake data).
  **Acceptance:**
    - [ ] a behind peer converges after the nudge; the badge flips to synced on the next ack
    - [ ] the button is absent when synced/unknown
- [ ] **PD-10: PEERS — the live-map Cosmic wallpaper**
  **As** a user,
  **I want** the live mesh map rendered as the Cosmic desktop background (a second output target of the same canvas scene) — pure render, clicks pass through (L21); adaptive power: ~30 fps under traffic, 1 fps idle, paused on battery and when fullscreen-covered (L22); configured as a "Live mesh map" choice in the Wallpaper panel beside static images (L23),
  **so that** the mesh's living state is ambient on the desktop (Q25, L21–L23). **Risk-first: prototype the layer-shell/cosmic-bg surface before building on it.**
  **Acceptance:**
    - [ ] enabling it shows the live map as the desktop background under Cosmic with peers/edges updating
    - [ ] clicks on the wallpaper reach the desktop beneath; on battery the animation pauses
    - [ ] disabling restores the prior static wallpaper

- [ ] **PD-11: PEERS — remote service lifecycle (Podman + KVM start/stop/restart)**
  **As** an operator on a peer's Services Provided section,
  **I want** start/stop/restart buttons per container and VM, routed over a new `action/services/lifecycle {peer, kind, name, op}` Bus verb that the target peer's mackesd executes locally — with a confirm dialog on the stop/restart direction ("Stop win11 on oak?"), start one-click (L9/L16),
  **so that** the directory operates the fleet's services, not just lists them.
  **Acceptance:**
    - [ ] starting a stopped container/VM on another peer brings it up; descriptor flips within one heartbeat
    - [ ] stop/restart requires the inline confirm; start does not
    - [ ] the verb refuses any target not present in the peer's published descriptor set (no arbitrary podman/virsh passthrough)
    - [ ] failures land in the inline result strip with the executor's error text
- [ ] **PD-12: PEERS — Wake-on-LAN for offline peers**
  **As** an operator looking at an offline peer,
  **I want** a Wake action that asks the nearest online peer sharing the target's LAN segment to send the magic packet (Bus verb; MAC from the existing peer-MAC cache) (L4),
  **so that** Offline is actionable, not just gray.
  **Acceptance:**
    - [ ] waking a WoL-enabled offline peer brings it Online in the directory within its boot time
    - [ ] the button is absent when no online peer shares the target's segment or no MAC is cached
- [ ] **PD-13: PEERS — presence-transition alerts**
  **As** a user anywhere on the desktop,
  **I want** peer Online↔Offline transitions emitted through alert_relay → the cosmic-applet FDO notification path (L5, rides OBS-7/OBS-8 plumbing),
  **so that** mesh-shape changes reach me without watching the directory.
  **Acceptance:**
    - [ ] killing a peer produces a desktop notification on the others within one presence tier window
    - [ ] its return notifies likewise

> Cross-refs: ENT-13 closes inside PD-6 · SVC-1's Remote Access panel consumes PD-5's launcher · OBS-6's health signals source from PD-2's Netdata-alarm summary · Call gating depends on SVC-4 (voice presence) · PD-9 depends on FPG-4/5 · PD-13 rides OBS-7/8 · L9 lifecycle verb is descriptor-gated (see design-doc level-2 risks).

## PLANES — the five-plane Workbench console (design: `docs/design/planes.md`, 2026-06-09)

> 100-Q survey (W1–W100) + hero round (H1–H10) + D-W1 (mesh tooling first, Red Hat second).
> Governance: new §9. **Order (W93): IA → This Node → Controller → Network → Fleet → Provisioning.**
> **Gates: starts after the PEERS epic (W100); FPG is a hard prerequisite for Controller/Network (W94);
> Provisioning waits on PKG core (W64).** Engines = mackesd workers @ Server rank (W90/W91); per-plane
> Bus prefixes (W89); all state = LizardFS + typed verbs, GUIs are renderers, CLI parity (W88/W27).

### IA + foundations

- [ ] **PLANES-1: the nav re-IA** — five plane sections (short labels: This Node · Controller · Network · Fleet · Provisioning), Peers Front Door above, Desktop group below (W5/W6/W10); full tree day-one with guided empty states naming their worklist item (W16); clean-break deep-link ids (W11); folds: Mesh Services→Node/Health, Drift→Controller/Remediation, Fleet Revisions→Controller/Config, Playbooks→Controller/Jobs, Mesh Control→Controller own entry (W12/W13/W8/W40/W52); Peers = Controller/Inventory dual-door (W7).
- [ ] **PLANES-2: hero images** — line-art originals (H2/H5) for Ansible/LizardFS/Nebula/Fedora/Netdata/Podman/libvirt/Cosmic/systemd/Remmina/PipeWire/rustls/VPN (H1); mde-theme compiled `Hero` enum + `hero_stroke` token w/ palette tests (H6/H7); header band right, 96–128px (H3/H4); NAME + live version caption (H8); hover stack card (H9); always renders, honest "not installed" (H10).
- [ ] **PLANES-3: capability-tags substrate** — hop/execution/headless in PeerRecord + `mackesd tag` verb; any enrolled surface edits, audit-logged (W82/W83); **tags GATE**: job acceptance needs execution, relay/exit config needs hop, headless flips GUI-unit systemd presets (W84/W85). Lands before any gate flips.

### This Node (W9: always the local box; W27: every panel = a mackesd verb)

- [ ] **PLANES-4: Registration panel** — full cert lifecycle (enroll/re-enroll/leave) + invite-token minting in-panel (W17/W18); fingerprint hex + word-pair (W25); tags view (W26). Absorbs **ENT-4** (`mesh init` lighthouse bootstrap) + **ENT-5** (unified `leave`) as the CLI/daemon side.
- [ ] **PLANES-5: Inventory panel** — the replicated PeerProbe rendered (PCI/USB/kernel/power/descriptors), no new collectors (W19).
- [ ] **PLANES-6: Health panel** — ENT-7 doctor checks live + re-run, folded service start/stop controls, local Netdata alarm list (W20); mackesd self-restart via systemd with honest reconnect (W24). Absorbs **ENT-7**.
- [ ] **PLANES-7: Config-apply panel** — applied vs newest revision, last Ansible log, Reconcile-now (W22); RPM version + repo source + update-now via typed job (W28).
- [ ] **PLANES-8: Logs/metrics panel** — journald mesh-unit view with `--since` (the ENT-9 verb rendered) + Netdata strip/deep-link (W23). Absorbs **ENT-9**.

### Controller (FPG prerequisite, W94)

- [ ] **PLANES-9: the jobs engine** — job = playbook ref + vars + targets (W29); signed bundles, target runs locally, **no push-SSH, no raw shell** (W21/W32); LizardFS `jobs/{templates,runs}` (W33); fleet-parallel serial-per-node (W34); leader-fired schedules (W35); Run-once (W37); failure-only alerts (W39).
- [ ] **PLANES-10: Job templates panel** — form + read-only YAML (W38), schedule field (W30), tag/role/peer targeting (W31), history runs→targets→output (W36). Playbooks panel absorbed (W40).
- [ ] **PLANES-11: Remediation panel** — drift-type→template map w/ event var bindings, drift list + matched plan + fire (W41); per-plan auto flag default-off, auto fires loud (W42). Drift panel folds in (W13).
- [ ] **PLANES-12: Audit** — hash-chained events: security + jobs/remediation + config/policy + lifecycle ops (W43); timeline viewer + verify-chain (W44); **72 h rolling retention** (W45, operator lock). Absorbs **ENT-14**.
- [ ] **PLANES-13: Policy engine** — TOML declarative assertions over replicated data (W46/W47); eval on-change + hourly leader sweep (W48); **violation = drift event** (W49); core pack ships enabled (W50); report default / enforce = opt-in auto-plan (W51).
- [ ] **PLANES-14: Fleet logs search** — Controller-side search over the OBS-5 mesh-replicated structured logs (W15). Absorbs **OBS-5**.

### Network

- [ ] **PLANES-15: netstate engine + panel** — full nmstate via NetworkManager (W65/W66), state in the BaselineSpec (W67), desired-vs-actual diff UI (W68); **all-at-once apply + post-apply self-test auto-revert** (must re-reach lighthouse + one peer or checkpoint reverts; ships in the SAME task as apply — W77/W78).
- [ ] **PLANES-16: firewall policy** — firewalld zones in the baseline; **overlay = trusted zone**, underlay per-role tight (W69/W70); revocation stays Nebula-blocklist-only (W71).
- [ ] **PLANES-17: VPN/tunnels + gateways** — Nebula topology (lighthouses/relays/punchy/unsafe_routes) as fleet state; external VPN client profiles (never transport, §1); **hop nodes: subnet advertise + full exit nodes** (W72/W73); exit path covered by validation before the toggle ships.
- [ ] **PLANES-18: mesh DNS** — mackesd → systemd-resolved per-link, `<host>.mesh`, no server (W74/W75); routing display-only otherwise (W76).
- [ ] **PLANES-19: validation suite** — overlay reachability fleet test (W79), post-apply + nightly leader job + Run-now, failures → drift (W80). Absorbs **ENT-10**.

### Fleet

- [ ] **PLANES-20: Fleet rollup dashboard** — groups by role + tag, cards = members online / worst health / drift (W86), live map centerpiece, drill-down selects into Peers (W81/W87); `mackesd fleet status` CLI parity. Absorbs **OBS-6** + **ENT-8**.

### Provisioning (after PKG core, W64)

- [ ] **PLANES-21: install profiles** — role + tags + ks fragments + join-token slot, TOML form-edited (W56); **boot-menu profile choice** on one image (W57); **auto-join firstboot** via single-use bearer (W60); USB/ISO only, PXE deferred (W59).
- [ ] **PLANES-22: images** — ISO+ks, VM golden, container images, USB writer (W53); builds = jobs on execution-tagged nodes (W54, builder tag deferred per W82); versioned dirs + TOML manifests on LizardFS (W55).
- [ ] **PLANES-23: Node roles panel** — fleet view of role pins + the tag editor (W58, the W26 edit surface), linked to install profiles.
- [ ] **PLANES-24: mirrors** — magic-mesh COPR mirror dir on LizardFS (W61); **every node serves itself via `file://` baseurl** + upstream fallback (W62); sync = scheduled one-puller job (W63).

> Out of scope (W99): multi-mesh federation · cloud nodes · non-Fedora agents · multi-tenancy.
> CI: standard gates + OBS-2 convergence tests for fleet-state engines (W97).

## ENTERPRISE — operability + security-enforcement gaps (from the enterprise-readiness verification)

> Source: `docs/design/enterprise-readiness.md` (verdict: **prototype with enterprise direction**).
> These are the enterprise-specific gaps the survey epics did NOT capture. **ENT-1/2/3 are CRITICAL.**
> Overlaps: installation/packaging/role-chooser = the **PKG** epic; CI/observability = **OBS**;
> control plane = **FLEET-PHASE-G**. The minimum bar for honestly claiming "enterprise-grade" is
> PKG + ENT-1, 2, 3, 5, 6, 7, 8, 9, 12 + OBS CI.
>
> **Corrective decisions (2026-06-09):** C1 enrollment → **single-use issued bearer allow-list** ·
> C2 revocation → **nebula pki.blocklist + reload** · C3 unpinned role → **refuse to start (fail
> closed)** · C4 operator UX → **a `meshctl` facade** (ENT-15) · C5 resilience → **systemd unit +
> hardened in-process supervisor** · C6 off-boarding → **both self-service `leave` + operator
> decommission** · C7 trust → **keep open-mesh, document the blast radius** (→ governance, ENT-12) ·
> C8 backup → **systemd-creds passphrase only** (keep the single QNM copy + replication) · C9
> positioning → **production workgroup-grade (≤8 peers)** (→ governance, ENT-12) · C10 audit →
> **hash-chained security events**.

- [ ] **ENT-1: enforce the enrollment bearer (CRITICAL — security)** — `sign_pending_csr` (`nebula_enroll.rs:571`) + `nebula_csr_watcher` sign any well-formed CSR and **never check the bearer/passcode** against an issued list, though the docs claim they do. Maintain an issued-but-unredeemed allow-list (single-use bearers); refuse a CSR whose bearer isn't pending-issued. **Acceptance:** an enroll with a wrong/replayed/absent bearer is refused (test); a valid single-use bearer signs once then can't be reused.
- [ ] **ENT-2: pin `role.toml` at provision + fail-closed when unpinned (CRITICAL)** — `mde_role::pin_at` is lib-only; nothing writes `/var/lib/mde/role.toml`, so every box runs unpinned→Workstation. Add `mackesd role pin <role>` (= PKG-4), have the installer/chooser call it, and **change `resolve_rank()` so an unpinned box REFUSES to start its worker pool** (C3, fail closed) instead of defaulting to Workstation. **Acceptance:** a Server install gates to rank-1 workers; an unpinned box refuses to start with a clear "pin a role" error; downgrade refused.
- [ ] **ENT-3: revocation evicts the data plane (CRITICAL — security)** — `ca revoke` only marks the DB + ban list + a bus event; the Nebula data plane keeps trusting the cert until expiry. Push a Nebula `pki.blocklist` (or equivalent) to running nebula + reload on revoke. **Acceptance:** a revoked node can no longer reach any peer within N seconds (integration test).
- [→] **ENT-4: `mackesd mesh init`** — **RE-HOMED → PLANES-4** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [→] **ENT-5: unify `mackesd leave` / decommission** — **RE-HOMED → PLANES-4** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [ ] **ENT-6: `mackesd.service` + supervisor hardening** — no systemd unit means nothing restarts mackesd on crash; the worker supervisor is a 250 ms fixed-retry stub (`workers/mod.rs:430`, no max-restarts/circuit-breaker). Ship the unit (Restart=on-failure) (= PKG-3) + bounded exponential back-off + circuit-breaker + max-restarts. **Acceptance:** `kill -9 mackesd` → restarted ≤ N s; a hot-looping worker trips the breaker instead of spinning at 250 ms forever.
- [→] **ENT-7: `mackesd doctor`** — **RE-HOMED → PLANES-6** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [→] **ENT-8: `mackesd fleet status`** — **RE-HOMED → PLANES-20** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [→] **ENT-9: `mackesd logs` + fix the GUI Logs panel** — **RE-HOMED → PLANES-8** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [→] **ENT-10: `mackesd test connectivity`** — **RE-HOMED → PLANES-19** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [ ] **ENT-11: backup passphrase hardening (C8)** — move `MDE_BACKUP_PASSPHRASE` off the systemd env into systemd-creds (TPM/host-bound); keep the single QNM-Shared copy carried by LizardFS replication (no off-mesh export required per C8). **Acceptance:** the passphrase is not visible via `systemctl show` / `/proc`; backup + `state-restore` still work end-to-end.
- [ ] **ENT-12: operator + end-user documentation + positioning (C7, C9)** — install guide, per-node-type setup guide, troubleshooting guide, and a DR runbook (the code points at a missing `docs/help/mesh-recovery.md`). **Document the open-mesh blast radius** as an accepted ≤8-peer trade-off (C7). Rewrite `DISCLAIMER.md`: "Mackes Workstation" → Magic Mesh, and reposition from "not for production" to **production workgroup-grade (≤8 peers)** with a stated supported envelope (C9). **Acceptance:** a new admin provisions all 3 node types + recovers a dead lighthouse using only the docs; the disclaimer + a SUPPORT.md state the production envelope.
- [ ] **ENT-13: replace the `mesh_latency` ping placeholder** — `mesh_latency.rs:10` shells `ping` as an admitted placeholder pending the transport handshake. Use the real transport RTT probe. **Acceptance:** latency reflects the overlay path, not ICMP.
- [→] **ENT-14: security-event audit (C10)** — **RE-HOMED → PLANES-12** (W96, planes re-IA); spec text lives in the PLANES task + `docs/design/planes.md`.
- [ ] **ENT-15: `meshctl` operator facade (C4)** — a thin friendly binary over mackesd/magic-fleet exposing the lifecycle gestures as one learnable tool: `install`, `status`, `doctor` (ENT-7), `mesh init` (ENT-4), `provision`/`join` (= enroll), `fleet status` (ENT-8), `test connectivity` (ENT-10), `logs` (ENT-9), `repair` (= heal/reconcile), `leave`/`decommission` (ENT-5). The named subcommands ENT-4/5/7/8/9/10 land here (or as mackesd verbs `meshctl` wraps). **Acceptance:** every §8 verification command in the enterprise-readiness doc runs as shown; `meshctl --help` lists them.

---

*Audit (sweeps 1–2): 18 findings, A1–H8. **8 shipped** (H1 §3, D1/H5/H2 §4, F1–F3/H7 §5-doc, G1 §1, A1/A2 deletion). The 7 open findings are now **specified** by the survey and resolve into the epics above.*
*Survey (2026-06-09): 100/100 answered → 6 epics, 51 tasks. Packaging (PKG-*) is held until every feature is §7-complete; releasing is operator-gated.*
*Enterprise-readiness verification (2026-06-09): verdict **prototype with enterprise direction**; +14 ENTERPRISE tasks (ENT-1/2/3 CRITICAL). Full report: `docs/design/enterprise-readiness.md`.*
