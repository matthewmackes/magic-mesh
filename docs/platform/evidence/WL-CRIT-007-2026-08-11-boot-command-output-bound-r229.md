# WL-CRIT-007 boot readiness command bound — 2026-08-11

- Scope: boot readiness caps `systemctl` stdout at 4096 bytes, kills oversized producers, and fails the probe closed.
- Farm: BigBoy `172.20.0.130`, agent slot `1`.
- Test: `workers::boot_readiness::tests::oversized_host_command_output_fails_closed`.
- Result: PASS, 1 passed, 0 failed, 4772 filtered out.
