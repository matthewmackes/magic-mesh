# WL-UX-013 / WL-ARCH-009 node-availability Bus transaction recovery r88

Date: 2026-08-09

Base revision: `dc7906bede3e6595d38b194acce72f86647aac93`

## Correctness model

`RuntimeAvailabilityPublisher` now treats its durable `current.json` record as
the restart-safe publication outbox and opens a fresh Bus transaction for every
publish or corrected-forward pass. Opening captures `index.sqlite` device/inode
identity before and after `Persist::open`, and rejects a connection whose
captured inode is not the current path inode. The same identity is verified
after the complete bounded durable input, after the bounded latest canonical
Bus row, before and after durable replacement, and after the exact Bus write.

The retained Bus input is one latest row, not an unbounded history fold. Its
body and typed intent are checked against the independent 4 KiB record ceiling,
node/device identity, and generation. An identical row is idempotent, an older
row is corrected forward, and malformed, conflicting, or same/newer retained
truth fails closed instead of being overwritten.

Admission remains staged on a cloned `AvailabilityLedger`. The durable write
may precede Bus success only as an explicit outbox: no ledger state, replay
window, fingerprint, or cursor is committed until the exact publication is
visible through the same verified Bus generation. If replacement lands after a
write, the retired index may contain the row, but the transaction reports
failure, retains the durable bytes, and the next fresh transaction publishes
them into the current index before admitting newer state.

## Hostile verification

Farm host: Machine 193 build VM `172.20.0.90`

Isolated slot: `/home/mm/magic-mesh-farm-node-availability-bus-r88`

The shared worktree contained an unrelated, unfinished `compute_expose.rs`
change that did not compile. For the final scoped gate only, the disposable
farm slot's copy of that file was replaced from `HEAD` using `git archive`; the
local/shared file was not changed. The formatted node-availability source was
then compiled as part of the complete `mackesd` library test binary and the
four exact `bus_transaction_` fixtures were selected:

```text
git archive HEAD crates/mesh/mackesd/src/workers/compute_expose.rs |
  ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'cd /home/mm/magic-mesh-farm-node-availability-bus-r88 && tar -xf -'

ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'source $HOME/.cargo/env; \
   cd /home/mm/magic-mesh-farm-node-availability-bus-r88 && \
   cargo test -p mackesd --lib \
     workers::node_availability::tests::bus_transaction_ -- --nocapture'
```

Result: `4 passed; 0 failed; 0 ignored; 4614 filtered out` in `0.02s` after a
`1m 08s` complete library compile/link. The exact passing fixtures prove:

- late Bus storage is consumed by the same long-running owner;
- an unreadable same-path replacement preserves durable truth, then publishes
  retained and forward generations in order after recovery;
- a spawned long-running worker remains alive across same-path replacement,
  consumes the forward request without reconstruction/restart, and never leaks
  the forward generation into the retired Bus; and
- replacement immediately after a write leaves the staged ledger uncommitted,
  while the durable outbox recovers into the current index.

The build emitted 258 pre-existing warnings from unrelated modules; there were
no node-availability diagnostics.

File-only formatting and whitespace gates:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'source $HOME/.cargo/env; \
   cd /home/mm/magic-mesh-farm-node-availability-bus-r88 && \
   rustfmt --edition 2021 --check \
     crates/mesh/mackesd/src/workers/node_availability.rs'
# exit 0

git diff --check -- crates/mesh/mackesd/src/workers/node_availability.rs
# exit 0
```

## Source identity

- `crates/mesh/mackesd/src/workers/node_availability.rs` SHA-256:
  `46c1e1a0b50c94ef39b79ad2fdd4b36ad07fcef74d825fda89730f6d907b1fb2`
- Binary patch SHA-256 against the base revision:
  `7bde7c6a278536544de72ad8d3b854239248e5cf00132aac22ccd720d311ab20`

## Residual live-proof gaps

This is deterministic farm evidence, not installed-seat evidence. It does not
exercise the real `/run/mde-bus` mount, an actual filesystem/storage outage,
daemon crash timing, logind suspend/resume, NetworkManager migration, or two
independent producer processes racing on a live node. Those remain release/live
recovery proofs; no claim is made that r88 closes WL-UX-013 or WL-ARCH-009.
