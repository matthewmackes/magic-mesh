# WL-FUNC-033 leftover — keep lint for `own_nebula_ip` — r1

Date: 2026-08-22  
Classification: keep-guard; **not** stack deletion and **not** archive  
Source revision: after `f9f448bf0` (this change)  
`production_admitted: false`

FUNC-033 leftover is keep `own_nebula_ip` in lib `voip_rtt.rs`. Other
mackesd paths still call it. Archiving this epic would invite a later
agent to delete that function. This lint fails closed if the keep
disappears or if `crates/` / `packaging/` grow a live
`mde-voice-config` / `kamailio-mde` / `rtpengine-mde` spawn.

Archive, ledger, evidence, salvage, and COMPLIANCE diary stay out of
scope. `own_nebula_ip` was not deleted. Seats were not mutated.

## Verification

Local (tiny helper, no cargo):

```text
install-helpers/lint-func033-keep.sh --self-test
install-helpers/lint-func033-keep.sh
```

Self-test: missing keep fails; keep without `pub fn own_nebula_ip`
fails; keep + clean trees pass; planted packaging unit fails.
Live scan: PASS.
