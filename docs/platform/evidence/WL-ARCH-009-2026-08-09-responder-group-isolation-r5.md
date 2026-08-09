# WL-ARCH-009 responder process-group isolation — 2026-08-09

## Isolation correction

The six grouped `mackesd serve --group` processes correctly filtered supervised
workers, but 20 raw responder and maintenance threads bypassed the Supervisor
and were started by every group process. This duplicated action consumers,
state responders, and maintenance loops across process boundaries.

Every raw start site now fails closed unless its canonical
`WORKER_REGISTRY` entry belongs to the selected process group. The exact
`--group value` and `--group=value` spellings are admitted; absent, duplicate,
non-UTF-8, and unknown values are refused. A bidirectional source guard compares
all guarded names with every `ResponderThread` registry entry so a future raw
thread cannot silently escape group ownership.

## Verification

- BigBoy (`172.20.0.130`), slot `arch009-responder-isolation-r1`:
  `cargo test -p mackesd --bin mackesd process_group_thread_admission_tests --locked -- --nocapture`
  passed 3/3.
- Machine 9 (`172.20.0.50`), slot `arch009-responder-registry-r1`:
  `cargo test -p mackesd --lib worker_role::tests::responder_threads_are_admitted_only_by_their_registered_group --locked -- --nocapture`
  passed 1/1.
- Scoped `rustfmt --check` passed for `spawn.rs`; `git diff --check` passed.
  Full-file `worker_role.rs` formatting still reports two unrelated pre-existing
  `navigation`/`clock` registry reflows, so no full-file format pass is claimed.

## Source hashes

```text
d37cfb2a6746ca107905fe5d4c8891779bfb27ab0f825f52cab9a911f94f22b3  crates/mesh/mackesd/src/bin/mackesd/spawn.rs
9bc9670e38dee61e8a01141e61f9603e0fb56480daffc0764bdeea61edc061cd  crates/mesh/mackesd/src/worker_role.rs
```

## Remaining acceptance gap

The source boundary is closed, but a built-package six-process inventory must
still prove each responder appears in exactly one live cgroup after restart and
failure recovery. ARCH-009 stays `Remaining`.
