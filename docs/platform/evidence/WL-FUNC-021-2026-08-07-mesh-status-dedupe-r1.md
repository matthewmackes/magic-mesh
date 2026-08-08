# WL-FUNC-021 — mesh-status/Syncthing churn reduction (2026-08-07)

## Audit

`packaging/systemd/mesh-status.timer` invokes the oneshot every 30 seconds on
every node. The helper previously rewrote the replicated
`<workgroup>/<hostname>/shell-status.json` on every invocation, even when the
version, role, and service booleans were unchanged. The only changing field
was `updated_ms`; no current consumer reads that field, and the peer snapshot
reader consumes the stable service/version payload.

## Scoped mitigation

`install-helpers/mesh-status-snapshot.sh` now compares the existing and
candidate peer records after removing only the numeric `updated_ms` field. It
rewrites when the record is missing, unreadable, malformed, or materially
changed, and otherwise performs no write. The existing direct best-effort write
and graceful failure behavior are retained. The local aggregate snapshot and
timer cadence were not changed.

## Verification

`bash -n install-helpers/mesh-status-snapshot.sh` passed locally. The focused
root probe ran the helper three times against a temporary workgroup on farm
host `.90`, slot `mesh-status-dedupe-r1`, with stable stubbed service probes.
The first run created the peer record; the second run left its inode mtime and
SHA-256 unchanged despite a new candidate timestamp; a third run with a
changed `nebula` probe rewrote the record and the aggregate snapshot completed:

```text
first_mtime=1786075589 second_mtime=1786075589 third_mtime=1786075592
first_sha=5ba2d758d222e6c11795b921d0a385472d421d177b3c73342afed88307deb623
second_sha=5ba2d758d222e6c11795b921d0a385472d421d177b3c73342afed88307deb623
third_sha=f19bb6615dba71397a58960592216855e6a70674386ed5328513d222b9b65ede
PASS root probe: stable peer record was not rewritten; changed service state rewrote it; aggregate snapshot completed
```

This is source/probe evidence only; installed-seat post-change CPU sampling and
live Syncthing counters remain open.
