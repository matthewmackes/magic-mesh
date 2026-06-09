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

- [ ] **E1 · retarget `mackesd/tests/integration_testcontainers.rs` to Nebula** (or REMOVE) — 6 non-`#[ignore]`d tests spin up real `headscale/headscale` + `tailscale/tailscale` containers. §1 pins the fabric to Nebula; the system-under-test is the retired substrate.

## P4 — Unreachable pub surface (§7)

- [ ] **H3 · `mde-card` dead pub surface** — REMOVE or wire: `migration` mod (`migrate`/`MigrationError`/`SCHEMA_VERSION`), `render_mode::RenderMode`, `schema::TemplateSpec` — zero workspace refs.
- [ ] **H4 · `mde-iced-components` dead pub surface** (~half the lib) — REMOVE or wire into the GUIs: `skeleton_shimmer`, `elevation_container`, `toast_chip`, `icon_fill_morph`, and the entire `motion` module (`SelectionSlider`, `fade_in_alpha`, `fade_out_alpha`, `slide_in_offset`, `theme_crossfade`, `stagger_delay_ms`, `shimmer_alpha`). All test-only callers.

## P5 — Mockup / dead nav / stub surfaces (§7)

- [ ] **B1 · `mesh_ssh` ("Mesh SSH")** — build the panel **or** drop it from `nav_model()` (`model.rs:290`). No panel module / no `panel_body` arm → always lands on `panel_under_construction()`.
- [ ] **H6 · `mde-music` Radio card** — back it with a `list-radio` daemon verb **or** REMOVE `HubCard::Radio` (`hub.rs`). Today it renders a card that can never populate.
- [ ] **C1 · Fleet Phase-G control plane** — `mackesd/src/ipc/fleet.rs:54–67` replies "not implemented until v2.0.0 Phase G" for `push/list/diff/rollback-revision`. Implement the no-fixed-center revision logic (§1). Honest stub today; §7-incomplete.

## P6 — Doc drift (FINISH — fix docs)

- [✓] **F1 · `mde <subcommand>` dispatcher doc-drift** — done (`b6d74de`). Also fixed the mde-role NotPinned error + role.toml header (operator-facing strings pointing at the non-existent `mde setup`) and two extra `pre-mde-setup` comments in mackesd.
- [✓] **F2 · labwc-as-current doc-drift** — done (`b6d74de`). `repair.rs` reload action marked the legacy labwc path (code untouched, pending A1/A2). `mackesd/Cargo.toml:240` left — it's accurate heritage.
- [✓] **F3 · GlusterFS-lock doc-drift** — done (`b6d74de` + `38edbcf`); also caught mesh-types `tags.rs`/`peers.rs`. **Residual (defer):** `mackesd/workers/nebula_ca_backup.rs:37` "GlusterFS topology snapshot (GF-9.2)" describes a versioned backup payload field — needs checking the snapshot code before relabeling (don't guess). `mackesd/src/ipc/nebula.rs:7` + `window_manager.rs:8` cite deleted `crates/shell/` paths — the latter dies with A2.
- [✓] **H7 · `mde-music/src/library.rs:24–26` stale comment** — done (`b6d74de`); only Radio is unbacked.

## P7 — Vestigial model / soft seams

- [✓] **G1 · vestigial `derp` field** — done (`d8d79f7`); dropped the field + render fragment. mde-files 271/0.
- [ ] **H8 · `mde-kdc-proto/src/discovery.rs` `SyntheticAnnounce`/`inject_synthetic`** (soft) — land the KDC2-4 mesh-shunt worker that consumes it, or drop the pub seam if KDC2-4 isn't imminent.

---

*Packaging (RPM / COPR / ISO) is unbuilt — tracked separately, not a defect (sweep §5 note).*
*Sweep-1 found A1–G1 (mackesd + workbench); sweep-2 added H1–H8 (music/voice/bus/fleet/kdc/transport/shared). 18 findings total.*
