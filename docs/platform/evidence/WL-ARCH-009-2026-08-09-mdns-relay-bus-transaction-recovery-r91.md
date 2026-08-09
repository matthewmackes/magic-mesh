# WL-ARCH-009 r91 — mDNS relay Bus transaction recovery

Date: 2026-08-09  
Scope: `crates/mesh/mackesd/src/workers/mdns_relay.rs`  
Base commit observed at final verification: `339678bb3f766d810646bf0e3ef9e4ebbc892fbc`

## Delivered correction

- Each relay pass fresh-opens `Persist` and binds the connection to the live
  `index.sqlite` device/inode. Identity is rechecked before and after Bus writes,
  before and after mDNS registration, and before cursor commit.
- Initial, late-Bus, and same-path replacement activation reads the complete
  retained announce lane under a 256-row bound and atomically installs its tail.
  Retained transient announcements are skipped; the first forward row is consumed
  once without restarting the worker. Oversized retained or forward batches fail
  closed without advancing a partial cursor.
- Inbound JSON is fully staged before effects and bounded to 64 KiB, 64 TXT
  records, allowlisted service types, literal peer IPs, and bounded identity
  fields. Private, malformed, and oversized rows cannot reach `mdns_sd`.
- Local discoveries survive a missing/replaced Bus in a bounded 512-service
  corrected-forward queue that coalesces updates by service identity.
- mDNS registration claims are installed only after `ServiceDaemon::register`
  accepts the command. A failed batch keeps its cursor for retry; successful
  rows collapse on replay, and `mdns-sd`'s same-fullname registration contract is
  an idempotent update. The registration lifetime is the worker-owned daemon
  lifetime, so restart deliberately re-registers rather than retaining a stale
  cross-process claim.

## Focused farm proof

Host: machine 194 build VM, `172.20.0.170`  
Slot: `mdns-relay-r91`

The shared checkout contained unrelated in-progress files. The slot was synced
with the required explicit route, then those unrelated source paths were restored
to `HEAD`; r91 remained the only source overlay under test.

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=mdns-relay-r91 \
  install-helpers/xcp-build.sh sync

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=mdns-relay-r91 \
  ssh ... 'cd /home/mm/magic-mesh-farm-mdns-relay-r91 && \
  cargo test -p mackesd --lib mdns_r91_ -- --nocapture'
```

Result: **PASS**, 6 passed, 0 failed, 4,614 filtered out. Covered exact
same-path replacement retained-skip/first-forward-once behavior, late-Bus
recovery, complete-batch refusal, hostile rows, success-bound idempotent effect
claims, and bounded outbound coalescing.

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=mdns-relay-r91 \
  ssh ... 'rustfmt --edition 2021 --check \
  /home/mm/magic-mesh-farm-mdns-relay-r91/crates/mesh/mackesd/src/workers/mdns_relay.rs'
```

Result: **PASS**.

The changed pre-existing cap/dedup regression was also run in the warm slot:

```text
cargo test -p mackesd --lib \
  republish_candidates_are_capped_but_duplicates_remain_noops -- --nocapture
```

Result: **PASS**, 1 passed, 0 failed, 4,619 filtered out.

## Artifact identity and residual evidence

```text
56b947dcb309f123a655dab9b24c1db503fa1b018303e21cc4a44ddf9e491ff1  crates/mesh/mackesd/src/workers/mdns_relay.rs
```

No live multicast LAN, real `mdns_sd` socket registration, peer-to-peer service
discovery, package install, or worker-restart proof is claimed by r91. Those
remain live/package acceptance evidence rather than a focused transaction test.
