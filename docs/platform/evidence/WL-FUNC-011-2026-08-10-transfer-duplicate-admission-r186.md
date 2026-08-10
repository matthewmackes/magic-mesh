# WL-FUNC-011 — transfer duplicate admission authority (r186)

Date: 2026-08-10

The daemon transfer queue now refuses a second submission with an already
admitted job ID instead of replacing the durable record. This protects a
currently Running transfer from replayed or conflicting client input: the
original source, state, and running-slot count remain authoritative.

Focused farm proof on `.90`:

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=func011-transfer-duplicate-admission-r186
install-helpers/xcp-build.sh cargo test -p mackesd --lib \\
  workers::transfers::queue::tests::duplicate_submit_cannot_replace_running_authority -- --nocapture

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4720 filtered out; finished in 0.00s
```

The test covers a hostile duplicate ID submitted while the original transfer
is Running and proves the durable authority is retained. Live cross-node
transfer, physical seat, and production Bus replacement behavior remain
unproven by this focused gate.
