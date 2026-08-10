# WL-UX-011 — sysfs control final-component nofollow (r183)

Date: 2026-08-10

Base revision: `38b0472b`

## Defect and correction

The device-control executor previously used `std::fs::write` for provider-
planned sysfs controls. If the final control component were replaced by a
symlink after admission, the privileged action could follow it and write a
different file. The executor now opens the existing final component with
`O_NOFOLLOW|O_CLOEXEC` before writing; creation and final-symlink redirection
are refused.

## Focused farm proof

Build VM `.90` (`172.20.0.90`), slot
`ux011-sysfs-nofollow-r183`, passed:

```text
cargo test -p mackesd --lib workers::device_control::tests::sysfs_control_write_refuses_a_replaced_final_symlink -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4714 filtered out
```

The hostile regression proves that a final symlink is refused and its victim
remains unchanged. This is provider/daemon source proof, not live-seat
acceptance.
