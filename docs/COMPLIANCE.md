# Magic Mesh — Compliance & Integrity Sweep

**Date:** 2026-06-09 · **Scope:** 20 crates / ~167.5k LOC · **Rulebook:** `AI_GOVERNANCE.md` (E11 "Magic Mesh" pivot)

Verdicts are binary: **FINISH** (make it real / wire it / fix the doc) or **REMOVE** (delete the dead surface).
Report-only — nothing was modified.

## Headline

The sweep is clean on the easy stuff: **no `todo!()`/`unimplemented!()`**, the mesh↔shell boundary
lint passes, no live Gluster/Tailscale/OpenSSL **dependencies**, crypto values are pinned. The real
finding is a single coherent theme: **a retired labwc/sway desktop-shell surface that is still wired
live** — a cluster of compositor-driving `mackesd` workers (spawned by default) plus a Workbench panel
that rewrites `labwc` config — all driving a compositor the repo no longer ships, now that **Cosmic owns
the desktop**. Secondary: a §4 Carbon-token break (scattered status-color literals that bypass — and
disagree with — the single-source tokens that already exist), retired-substrate integration tests, and
a band of `mde <subcommand>`-dispatcher / GlusterFS doc-drift.

## Findings

| # | Location | Category | Evidence | Conf. | Verdict |
|---|----------|----------|----------|:---:|:---:|
| **A1** | `mackesd/src/workers/{border_tinter, sway_config_watcher, window_rules, tag_layout, tag_mode_writer, tag_autostart, tag_manifest_watcher, workspace_router, workspace_namer, urgency_router, marks_state, auto_mark, session_persist}.rs` | Unreachable / retired surface (§5,§7) | All 13 spawn at the **default Workstation rank** (`worker_role.rs:79–85` defaults unpinned boxes to rank 2). Each drives the **sway/labwc** compositor over `swayipc_async` (`client.focused`, `swaymsg reload`, `move workspace to output`, writes `~/.config/sway/config.d/*`). Cosmic ships no sway session → `Connection::new()` fails → infinite 3 s backoff, **no consumer**. `border_tinter` cites `data/sway/config:60` which **does not exist**. `mackesd/Cargo.toml:240` self-identifies them as "a sway-IPC holdover under labwc; tracked for the labwc/ext-protocol port." | High | **REMOVE** (or port to Cosmic ext-protocols — but as shipped they are dead) |
| **A2** | `mde-workbench/src/panels/window_manager.rs` | Mockup / retired surface (§5,§7) | Live nav-registered panel (`model.rs:263`, wired in `app.rs:157/256/360/782/864`). Its own doc (L1–9): "labwc window-behaviour controls … exposes the real labwc knobs from `~/.config/labwc/rc.xml` … atomic write + `labwc --reconfigure`." Controls the **retired** compositor; under Cosmic the rewrite + reconfigure target a WM that isn't running. | High | **REMOVE** / retarget to Cosmic |
| **B1** | `mde-workbench/src/model.rs:290` (`mesh_ssh`) | Mockup — dead nav entry (§7) | "Mesh SSH" is in `nav_model()` and is a live deep-link target (`--focus network.mesh_ssh`), but there is **no `mesh_ssh.rs` panel module and no `panel_body` arm** — every click/deep-link falls through to the `panel_under_construction()` "isn't ready yet" empty-state (`app.rs:1323/1337`). A sidebar surface that renders but does nothing. | High | **FINISH** (build the panel) **or REMOVE** (drop the nav entry) |
| **C1** | `mackesd/src/ipc/fleet.rs:54–67` | Stub (§7) | The `action/fleet/{push,list,diff,rollback}-revision*` Bus verbs are wired into `run_serve` but every reply is `"Fleet.<verb> — not implemented until v2.0.0 Phase G"`. Honest stub (surfaces as a "no revisions yet" empty-state, **not** fake data), but §7-incomplete: the fleet-revision control plane is the no-fixed-center core (§1) and is not yet real. | High | **FINISH** (implement Phase G) |
| **D1** | `mde-workbench/src/panels/{home, mesh_services, mesh_pending, mesh_topology, mesh_control, health_check, drift, sync_status, service_publishing, panel_apps}.rs`, `panel_chrome.rs`, `header.rs`; `mde-iced-components/src/lib.rs`; `mde-files/src/widgets.rs:755`; `mde-music/src/main.rs:1057` | Convention violation §4 (raw color literals) | ~40 production-path `Color::from_rgb(0.20,0.80,0.40)` / `(0.95,0.70,0.20)` / `(0.92,0.32,0.30)` status colors hardcoded outside `mde-theme`. **`mde-theme::Palette` already single-sources** `success`/`danger`/`warning` (`palette.rs:36–82`) — and the literals **aren't even the Carbon values** (`0.20,0.80,0.40` = `#33CC66`, not Carbon Green 50 `#24a148`). Direct §4 break: scattered literals bypassing the lint-gated token source. *(Test-only `from_rgb` in `#[cfg(test)]` excluded.)* | High | **FINISH** (read `palette.success/danger/warning`) |
| **E1** | `mackesd/tests/integration_testcontainers.rs` | Substrate lock §1 (retired transport) | Six non-`#[ignore]`d `#[test]`s spin up **real `headscale/headscale` + `tailscale/tailscale` containers** and assert they serve (`headscale_starts_and_serves_api`, `tailscale_peer_starts_against_test_headscale`). §1 pins the fabric to **Nebula** (no Tailscale/Headscale). The system-under-test is the retired substrate, not Nebula. | High | **FINISH** (retarget to Nebula) / **REMOVE** |
| **F1** | `mde-workbench/src/panels/{displays.rs:1, keyboard.rs:1, mouse.rs:1, repair.rs, home.rs:537}`, `app.rs:119/123/760`; `mde-files/src/{picker.rs:11, app.rs:33/357, main.rs:45}`; `mde-role/src/lib.rs:5/114/127/321`; `mackesd/src/workers/kdc_host.rs:66` | Doc drift §0/§5 (`mde` dispatcher as live) | ~13 doc-comments describe a `mde <subcommand>` dispatcher as the current entrypoint — `mde settings … --page`, `mde setup --profile`, `mde display`, `mde files`, `mde mount`, `mde phone`, `mde filedialog`. No such dispatcher exists post-pivot (separate binaries). `picker.rs:11` also points at the **deleted** `crates/shell/mde/src/filedialog.rs`. | High | **FINISH** (fix docs) |
| **F2** | `mde-role/src/lib.rs` (Workstation variant), `mde-workbench/src/panels/repair.rs`, `mackesd/Cargo.toml:240` | Doc drift §5 (labwc as current) | `mde-role` defines Workstation as "Server + the **labwc / iced** desktop surfaces" — but §5 says "Workstation = a Cosmic desktop." `repair.rs` reload-compositor action prose: "Ask **labwc** to re-read its config." | High | **FINISH** (fix docs) |
| **F3** | `mde-files/src/{views.rs:189/1150/1193, model.rs:247}`, `mde-card/src/schema.rs:29` + `mde-card/Cargo.toml:13` | Doc drift §1 (GlusterFS as current lock) | Comments assert "**per the v5.0.0 GlusterFS lock** these dirs ARE … full-mesh-replicated" and "the mesh GlusterFS layer can replicate it." §1 retired Gluster wholesale for **LizardFS**; the live worker is `meshfs_worker.rs`. (Heritage "mirrors gluster_worker shape" comments elsewhere are **not** flagged — they're provenance, not current-state claims.) | Med | **FINISH** (say LizardFS) |
| **G1** | `mde-files/src/model.rs:37` (`derp: String`), rendered `views.rs:850` / `widgets.rs:314` | Vestigial Tailscale-era data model (§1) | The peer model carries a `derp` (DERP relay-region) field — a Tailscale concept. `RealBackend`/`bus_backend` always set it `""` (`backend.rs:778`, `bus_backend.rs:316`); only `demo_data` fills it ("fra"/"ord"). Real app renders "`{ms} ms via `" (empty). Nebula uses lighthouses, not DERP regions. | Med | **FINISH** (drop the field / rename to relay) **or REMOVE** |

## Counts by category

| Category | Findings | Verdict skew |
|----------|:---:|---|
| Unreachable / retired desktop-shell surface (A) | 2 *(A1 = 13 workers, A2 = 1 panel)* | REMOVE / port |
| Mockup — dead nav entry (B) | 1 | FINISH or REMOVE |
| Stub (C) | 1 | FINISH |
| Convention violation §4 — raw color literals (D) | 1 *(~40 sites / ~14 files)* | FINISH |
| Substrate lock §1 — retired transport in tests (E) | 1 | FINISH / REMOVE |
| Doc drift (F) | 3 bands *(~20 sites)* | FINISH |
| Vestigial data model (G) | 1 | FINISH |

## Checked clean (no finding — recorded to avoid re-litigating)

- **Stubs:** `rg` for `todo!()`/`unimplemented!()`/`panic!("not …")` → **zero**.
- **Mesh↔shell boundary:** `install-helpers/lint-mesh-boundary.sh` → **clean**.
- **Live substrate deps:** no Gluster/Tailscale/OpenSSL **dependency or live module** — all such names are heritage comments, legacy-token-rewrite shims (`derp_relay → nebula_lighthouse_relay`), or the retired test (E1). LizardFS (`meshfs_worker.rs`) is the live storage worker.
- **`mde-files/src/demo_data.rs`:** correctly fenced — only `DemoBackend` consumes it, and every `DemoBackend` construction is under `#[cfg(test)]`. The shipping app uses `RealBackend`; never reaches demo data.
- **Workbench panels:** 57/58 nav panels wired to real views (`mesh_ssh` is the lone exception, B1).
- **Device-tag colors** (`mesh-types/tags.rs`, `tag_manifest.rs`) storing CSS hex like `#42be65`: user data in the mesh model, not theme literals — out of §4 scope.
- **`DISCLAIMER.md` pre-flight gate:** exists, non-empty (3.5 KB), gated by `mde-disclaimer`.

## Sweep 2 (2026-06-09) — under-examined crates (music/voice/bus/fleet/kdc/transport/shared)

A second pass over the crates the first sweep covered lightly. It surfaced **a §3 crypto-lock
violation and three more substantial findings the first pass missed** — two because the first hex
regex (`#[0-9a-fA-F]{6}` / `from_rgb(`) had a blind spot for the `rgb(0x..)` and struct-literal
`Color { r:.. }` forms. Net: the easy locks (§2 Bus boundary, AES/ChaCha/rustls, no live OpenSSL)
are clean; the new findings are below.

| # | Location | Category | Evidence | Conf. | Verdict |
|---|----------|----------|----------|:---:|:---:|
| **H1** | `mde-kdc-host/src/pairing.rs:236` (`generate_pkcs8`), reached via `PairingStore::open:101` | **Substrate lock §3 (crypto)** | The **live** KDC device identity is `RsaPrivateKey::new(&mut rng, 2048)`. §3 pins **RSA-4096** KDC device identity. A compliant 4096 generator exists and is exported (`keygen.rs:63`, `RSA_MODULUS_BITS=4096`, `lib.rs:41`) but every caller is `#[cfg(test)]` (`lan.rs`/`tls.rs` test mods). The on-disk `identity.pkcs8` served by the live TLS listener (`lan.rs:429`) is 2048-bit. | High | **FINISH** (rewire `PairingStore::open` to the 4096 `keygen`) |
| **H2** | `mde-voice-hud/src/theme.rs` (~30 consts) | Convention violation §4 | A full parallel Carbon palette built from raw `rgb(0x16,0x16,0x16)`… literals — Gray ramp, `SUCCESS rgb(0x42,0xbe,0x65)`, `WARNING 0xf1c21b`, `ERROR 0xfa4d56`, Blue accents. Duplicates `mde-theme::Palette` outside the token module (§4 single-source). Worse, **divergent**: `SUCCESS` here is Green 40 `#42be65` vs theme's Green 50 `#24a148`. The header self-justifies as "a canonical token site," contradicting §4. | High | **FINISH** (source from `mde-theme`) |
| **H3** | `mde-card`: `migration` mod (`migrate`/`MigrationError`/`SCHEMA_VERSION`), `render_mode::RenderMode`, `schema::TemplateSpec` (re-exported `lib.rs:30/34/35`) | Unreachable pub surface §7 | `mde-card`'s `schema::{Card,CardKind}`/`probe::*` are heavily consumed (mackesd, workbench) — but these three pub items have **zero refs anywhere** in the workspace (not even tests), confirmed by `rg`. | Med-High | **REMOVE** (or wire) |
| **H4** | `mde-iced-components`: `skeleton_shimmer`, `elevation_container`, `toast_chip`, `icon_fill_morph`, the entire `motion` module (`SelectionSlider`, `fade_in_alpha`, `fade_out_alpha`, `slide_in_offset`, `theme_crossfade`, `stagger_delay_ms`, `shimmer_alpha`) | Unreachable pub surface §7 | Only `object_card` + `overlay_white_on`/`overlay_color_on`/`with_alpha` are consumed downstream. The rest build + unit-test green but have **zero production callers** — every reference is the def or a `#[cfg(test)]` test (`lib.rs:928–1258`). Roughly half the 1258-line lib. | Med-High | **REMOVE** (or wire into the GUIs) |
| **H5** | `mde-music/src/main.rs:1244–1250` (and white-60% literals 1198–1234) | Convention violation §4 | Hardcoded `iced::Color { r:0.36, g:0.42, b:0.96, a:1.0 }` indigo for the selected maxi-tab label — duplicates `Palette.accent`, not album-art-derived (the file's `c()` helper at 44–67 already does this correctly). Extends finding **D1**. *(Album-art `from_rgb8` at 1057 and `color.rs` extraction are legit — not flagged.)* | High | **FINISH** (route through palette) |
| **H6** | `mde-music`: `HubCard::Radio` (`hub.rs`), `verb_for(Radio)→None` (`library.rs:36`) | Mockup — unbacked feature §7 | The hub renders a **Radio** card, but clicking it does `Task::none()` (no fetch) and the daemon has **no `list-radio` verb** (`bus_responder` handles albums/artists/genres/podcasts/recents/playlists only). The page falls to a generic "start mde-musicd" empty-state that can never populate — an unbuilt feature presented as a working card. | Med | **FINISH** (back it) **or REMOVE** (drop the card) |
| **H7** | `mde-music/src/library.rs:24–26` | Doc drift §7 | Comment claims Playlists/Recents/Genres/Podcasts "not yet backed by a daemon verb" — but the code (32–35) returns verbs for all four and the daemon serves them. Only **Radio** is actually unbacked. | High | **FINISH** (fix comment) |
| **H8** | `mde-kdc-proto/src/discovery.rs:71/413` (`SyntheticAnnounce`, `Registry::inject_synthetic`) | Forward-declared seam §7 (soft) | Pub surface reachable only from `#[cfg(test)]` today; honestly documented as awaiting the KDC2-4 mesh-shunt worker. Data model + `is_fresh` are real. Acceptable as an honest forward-decl — flag only if KDC2-4 isn't imminent. | Med | **FINISH** (land KDC2-4) — soft |

### Sweep-2 checked clean (no finding)

- **§2 Bus boundary:** no `dbus`/`zbus` dependency in any of the 8 crates; **zero** D-Bus name ownership (`request_name`/`RequestName` → no hits; no `org.mackes.*`/`dev.mackes.*` claimed). Only historical `EPIC-RETIRE-DBUS` doc-comments.
- **§3 crypto (rest):** AES-256-GCM session (`crypto.rs:359/388`), ring `RSA_PKCS1_SHA256` signing, rustls 0.23 + ring (no OpenSSL), no MD5/SHA1-for-security. `transport_capabilities` `Aes128Gcm`/`ChaCha20` are *descriptive peer-capability metadata* for the scorer, not MDE cipher selection.
- **§1 transport:** `mackes-transport` enum is fully Nebula-named; `rewrite_legacy_token("derp_relay" → "nebula_lighthouse_relay")` is the allowed old→new shim.
- **Reachability:** `mde-bus` (cli/correlate/dnd/retention/audit all wired + a real binary), `mackes-nebula-https-tunnel` (wired into `nebula_https_listener`), `magic-fleet` (real CLI engine), and all `mde-music`/`mde-musicd`/`mde-voice-hud` modules are reached. MPRIS is fully built (its "not yet built" note is narrowly about engine-driven auto-advance; Seek/SetPosition/Quit no-ops are dbus-trait-forced).
- `mde-kdc-proto/src/dispatch.rs:116` `.also_log` — a *private* documented-inert hook, not a pub feature surface. Acceptable.

## Packaging reachability (§5) — not-yet-implemented

No RPM spec / `generate-rpm` metadata exists in the repo yet (the one-RPM install-time role chooser,
signed COPR, and Magic-on-Cosmic ISO are unbuilt). Per the skill, this is the **expected gap**, not a
defect — flagged for tracking, not as a finding. The `DISCLAIMER.md` gate that packaging will need is
already present.

## Suggested order of execution (when you choose to act)

1. **A1 + A2** — decide port-to-Cosmic vs. delete for the labwc/sway surface; this is the largest dead
   surface and the clearest §5/§7 break. (Resolving it also clears F2's labwc doc-drift and the
   `swayipc-async` dep.)
2. **D1** — mechanical and high-value: swap ~40 literals to `palette.{success,danger,warning}`. Restores
   §4 Carbon compliance and fixes the off-brand colors.
3. **E1** — retarget or remove the Headscale/Tailscale integration tests.
4. **B1, C1, F1/F3, G1** — wire/remove the Mesh SSH entry; track Fleet Phase-G; fix the
   `mde`-dispatcher + GlusterFS doc-drift; clean the vestigial `derp` field.
