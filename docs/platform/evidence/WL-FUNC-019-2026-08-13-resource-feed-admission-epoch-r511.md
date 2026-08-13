# WL-FUNC-019 resource feed admission epoch — r511

## Result

Remote Sessions action authority now has a shell-local admission epoch. Catalog
replacement, catalog conflict, reconnect, or unavailable transitions revoke the
current Android Start bindings and accepted Workload cancellation capabilities.
An asynchronous action reply is adopted only when both its immutable catalog
identity and the local admission epoch still match.

This closes a stale-feed recovery hole: an action launched before publisher or
feed loss cannot become authoritative merely because the identical catalog is
re-admitted before its delayed reply arrives. The daemon may still own any
downstream effect, but the shell neither reports that stale completion as a
current action nor creates a cancellation handle from it.

Production and regression scope:

- `crates/desktop/mde-shell-egui/src/vdi/resources.rs`
- `action_reply_from_before_feed_loss_is_not_adopted_after_same_catalog_recovers`

## Farm evidence

BigBoy `172.20.0.130`, slot/workspace `func019-epoch-test`:

```text
cargo test -p mde-shell-egui \
  action_reply_from_before_feed_loss_is_not_adopted_after_same_catalog_recovers \
  -- --nocapture

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out
```

The initial cold compile observed a concurrent partial write in the unrelated
`storage/mod.rs` scope and did not execute the test. The same warmed workspace
was continued after restoring only that unrelated file to committed `HEAD`; the
successful result above is the authoritative gate.

Farm `.50`, slot `func019-epoch-clippy`:

```text
cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings
Finished `dev` profile [unoptimized + debuginfo]
```

The shared-tree Clippy attempt was likewise blocked by the concurrent Storage
syntax edit. The successful gate used the slice patch with only
`storage/mod.rs` restored to committed `HEAD` in its disposable farm workspace.

Farm `.170`, slot `func019-epoch-fmt-final`:

```text
rustfmt --edition 2021 --check \
  crates/desktop/mde-shell-egui/src/vdi/resources.rs
```

Result: passed with no output. The broader crate formatter was not used as
evidence because the same unrelated in-progress Storage edit was not parseable.

## Remaining acceptance

WL-FUNC-019 still requires first-full-release packaging followed by the
explicitly deferred, non-blocking installed one-node proof for credential/feed
loss and rotation, route recovery, and authenticated live RDP rendering.
