# WL-CRIT-006 bootc image ID — 2026-08-11

- Scope: bootc verification resolves a mutable candidate tag once and runs all inspection against the validated immutable SHA-256 image ID.
- Hostile boundary: replacing the mutable tag after resolution cannot switch the candidate bytes inspected by the gate.
- Focused gate: `packaging/bootc/verify-image.sh --self-test`.
- Farm: fixed coordinator snapshot on `172.20.0.196`, slot 1.
- Result: **PASS**, hostile mutable-tag substitution rejected.
- Remaining boundary: retain the immutable image ID in a real signed release bundle and prove installed-seat correspondence.
