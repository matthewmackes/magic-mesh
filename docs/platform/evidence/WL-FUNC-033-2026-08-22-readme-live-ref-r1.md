# WL-FUNC-033 leftover — README live telephony reference — r1

Date: 2026-08-22  
Classification: leftover-honesty / live product-reference correction; **not**
epic closure and **not** a fleet-negative re-run  
Worklist unit: `qu0025fn`  
Source revision (worktree parent): `7af62ace5b1aba53e8f1ec5c8f83532c84f60f02`  
Farm host / slot: `172.20.0.50` / `0` (`mcnf-build-home-services`,
`~/magic-mesh-farm-0`)  
`production_admitted: false`

README.md lines 146–147 advertised the deleted Kamailio + RTPengine mesh-PBX
stack and `mde-voice-config` as live telephony. FUNC-033 remaining work says
greps for deleted modules return only archive and ledger references; README is
a live product reference. This unit replaces that one bullet. It does not
delete `own_nebula_ip`, does not re-run the fleet-negative, and does not close
the epic.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-033`.
- Write scope: README telephony / voice bullet only, this evidence file, and
  the epic Current state plus Remaining-work leftover sentence.
- Keep `own_nebula_ip` in `crates/mesh/mackesd/src/voip_rtt.rs`.

## Old bullet

```text
- **Telephony / voice** — `mde-voice-egui` + `mde-voice-config`: a SIP softphone
  with mesh-internal extensions (Kamailio + RTPengine) and an outbound gateway.
```

## New bullet

```text
- **Telephony / voice** — `mde-voice-egui` / `mde-voice-hud` / Communications
  Calls: the Construct softphone path and Calls surface.
```

The new bullet names only the current softphone path. It does not present
Kamailio, RTPengine, or `mde-voice-config` as current, and it does not invent
features.

## Farm grep

Admitted with `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0
install-helpers/xcp-build.sh sync` (route pinned; remote admission
71287652 KiB free). No `cargo test --workspace`. No local cargo
build/test/clippy. No seat mutation.

Remote command (host `172.20.0.50`, slot `0`, dir
`/home/mm/magic-mesh-farm-0`):

```text
awk "/Telephony \/ voice/,/^- \\*\\*Browser/" README.md | sed "$d"
# then grep -nEi "kamailio|rtpengine|mde-voice-config" on that extract
grep -n "own_nebula_ip" crates/mesh/mackesd/src/voip_rtt.rs
```

Observed:

```text
=== TELEPHONY BULLET ===
- **Telephony / voice** — `mde-voice-egui` / `mde-voice-hud` / Communications
  Calls: the Construct softphone path and Calls surface.
=== BULLET FORBIDDEN TOKENS ===
FORBIDDEN_HIT=no
=== own_nebula_ip ===
77:pub fn own_nebula_ip() -> Option<String> {
118:    if let Some(peer) = own_nebula_ip() {
OWN_NEBULA_IP_PRESENT=yes
```

## Leftover

Leftover is still keep `own_nebula_ip` in lib `voip_rtt.rs`. This record does
not claim WL-FUNC-033 closed and does not claim a fleet-negative re-run.
