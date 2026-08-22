# WL-FUNC-033 leftover — keep lint is a ci-gate policy check — r1

Date: 2026-08-22  
Classification: keep-guard wiring; **not** stack deletion and **not** archive  
Source revision: after `1d03b6620` (this change)  
`production_admitted: false`

`lint-func033-keep.sh` existed but was not in `ci-gate.sh` POLICY_LINTS,
so a later delete of `own_nebula_ip` or a new `kamailio-mde` unit could
pass the farm gate. FUNC-033 leftover is still keep; this wires the
keep into the maintained policy suite (self-test then live scan).

`own_nebula_ip` was not deleted. Seats were not mutated.

## Act

Added `lint-func033-keep.sh` to both `POLICY_LINTS` and
`POLICY_SELF_TESTS` in `install-helpers/ci-gate.sh`.

## Verification

Local (tiny helper, no cargo):

```text
install-helpers/lint-func033-keep.sh --self-test
install-helpers/lint-func033-keep.sh
```

Both PASS. `ci-gate.sh` was not rewritten while a gate job was running.
