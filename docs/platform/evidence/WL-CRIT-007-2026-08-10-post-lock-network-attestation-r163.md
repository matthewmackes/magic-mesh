# WL-CRIT-007 post-lock network attestation — r163

- Revision: `15997659` (`fail closed when peer recovery loses network`).
- Scope: peer recovery rechecks physical network readiness after acquiring its single-flight lock, preventing a stale positive event from restarting Nebula or mutating downstream services.
- Farm gate: seat 90 recovery self-test for `install-helpers/test-mesh-peer-recovery.sh`.
- Result: all recovery fixtures passed, including the stale-network fixture; no service mutation occurred after the second readiness check failed.
