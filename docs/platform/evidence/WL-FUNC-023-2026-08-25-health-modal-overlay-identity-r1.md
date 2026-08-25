# WL-FUNC-023 — Construct Health Fix labels for overlay-identity leftover (2026-08-25)

Farm contract evidence only. Does **not** claim live Construct Fix proof,
dest-cut click, Sunshine capture, or operator dest minting.

## Scope

`crates/desktop/mde-shell-egui/src/health_modal.rs` confirmation copy for
`HealthAction::OpenOnboarding`. Parent `node_grade` maps
`overlay-identity-missing` (host cert absent / overlay-ip empty without live
`nebula1`) to Open Onboarding; this slice only keeps the renderer labels
honest.

## Label / copy delta

| Action | Button label | Confirmation copy |
|---|---|---|
| `OpenOnboarding` | unchanged: `Open Onboarding` | **new:** covers missing host cert *and* empty overlay-ip leftover *and* signed identity receipt; refuses dest inventing; does not say “publish overlay IP” |
| `PublishOverlayIp` | unchanged: `Publish overlay IP` | live nebula1 rewrite only; not the Fix when the host cert is missing |
| `RestartMackesd` | unchanged: `Restart mackesd` | still CONFIRM-gated in the UI when the typed descriptor requires confirmation |

Generic fallback remains: “Confirm this guided action after reviewing its expected impact.”

## Farm verification

`--lib` is wrong for this crate (binary-only). First dispatch:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui --lib health_modal
```

Result: `error: no library targets found in package mde-shell-egui` (exit 101).
KVM-XCP1 `172.20.0.90`, slot `2`. Admission: `111,206,336 KiB` free; required
`8,388,608 KiB`. No source mutation from that probe.

Working filter (same host/slot):

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui --bin mde-shell-egui health_modal
```

- Host: KVM-XCP1 `172.20.0.90`, slot `2` (BigBoy `.130` left for parent)
- Source revision at dispatch: `4071ed295` plus this uncommitted health-modal slice
- Result: **PASS**, `38 passed; 0 failed; 0 ignored; 1609 filtered out; finished in 23.29s`
- New tests: `open_onboarding_label_and_confirmation_cover_overlay_identity_leftover`,
  `restart_mackesd_stays_confirmation_gated_when_descriptor_requires_it`
- Compile: `Finished test profile in 6m 10s`
- Exit: `0`

Pre-existing unused-item warnings in `mde-vdi-rdp`, `mde-collab-egui`, and
`mde-maps-location-egui` are outside this write scope.

## Boundary

Live Construct Fix for overlay-ip / join leftovers remains open. This note
does not authorize dest minting, identity-admission loosening, or a seat click.
