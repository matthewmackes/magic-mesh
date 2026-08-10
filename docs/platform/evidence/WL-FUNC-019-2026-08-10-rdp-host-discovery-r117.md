# WL-FUNC-019 — RDP host-discovery admission (r117)

Date: 2026-08-10

## Correction

The bounded nmap probe could discard an ICMP/HTTP-quiet Windows endpoint before
its curated ports were scanned because the default discovery set does not
include TCP/3389. The fast and deep scans now spell out the existing privileged
discovery probes and add TCP SYN discovery on 3389 alongside 443. The change
does not use `-Pn`, expand the approved target set, or expand the curated scan
ports.

The focused library regression also proves that both scan modes retain the
exact supplied targets and curated port specification while carrying the RDP
discovery probe.

## Focused farm proof

Machine 9 (`172.20.0.50`) passed the exact test:

```text
cargo test -p mackesd --lib \
  probe_nmap::tests::rdp_host_discovery_keeps_the_bounded_target_and_port_scope \
  -- --exact --nocapture

test result: ok. 1 passed; 0 failed; 4666 filtered out
```

No broad or duplicate integration test is counted for this checkpoint.

## Remaining boundary

This source checkpoint does not claim live Windows discovery. The corrected
daemon must still be packaged and deployed to seat 15, after which a fresh
TCP/3389 inventory row and its authenticated, approval-gated Remote Sessions
card must be observed.
