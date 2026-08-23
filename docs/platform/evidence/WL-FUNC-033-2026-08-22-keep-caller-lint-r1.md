# WL-FUNC-033 leftover — keep lint requires live callers — r1

Date: 2026-08-22  
Classification: keep guard; **not** a delete of `own_nebula_ip`  
Source revision: after `7d84ac1eb` (this change)

Keep leftover is `own_nebula_ip` in lib `voip_rtt.rs` because other
mackesd paths still call it. The keep lint only grepped the function
name, so a caller-less stub would have passed.

## Act

`lint-func033-keep.sh` now requires at least one
`voip_rtt::own_nebula_ip` call in `crates/mesh/mackesd` outside
`voip_rtt.rs`. The function rustdoc names the leftover. Do not archive
while leftover is keep. Salvage / COMPLIANCE diary stay out of scope.

## Verification

```text
install-helpers/lint-func033-keep.sh --self-test
lint-func033-keep.sh: self-test passed
install-helpers/lint-func033-keep.sh
lint-func033-keep: PASS: own_nebula_ip kept with callers; crates/packaging have no live PBX spawn
```

Already-green `@farm:{cargo test -p mackesd}` was not re-run as filler.
The keep lint remains in ci-gate `POLICY_LINTS`.
