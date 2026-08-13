# WL-FUNC-019 publisher credential loss — r509

Date: 2026-08-13

Commit: pending at gate time

## Result

The production Remote Sessions chooser now re-reads its exact systemd-managed
resource-publisher credential at every catalog refresh boundary. Credential
loss, invalid replacement, and key rotation revoke the retained authenticated
catalog and any prepared or accepted VDI handoff before a newly attested
snapshot may arm actions. The explicit untrusted compatibility constructor is
unchanged.

This closes a stale-authority gap where the shell previously retained the HMAC
bytes loaded at process construction and could continue authorizing actions
after the credential had disappeared from the systemd credential directory.

## Farm evidence

- `.170`, slot `func019-credential-test`: exact hostile regression
  `managed_publisher_credential_loss_revokes_retained_action_authority` passed
  1/1, with 1,585 tests filtered out.
- `.50`, slot `func019-credential-clippy`: strict production binary Clippy
  (`cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`)
  passed after the disposable farm copy of an unrelated dirty
  `status_bar.rs` was restored to repository HEAD. The local concurrent edit
  was not modified.
- `.196`, slot `func019-credential-fmt`: file-scoped Rustfmt passed for
  `crates/desktop/mde-shell-egui/src/chooser/resources.rs`. The package-wide
  formatting command also reported unrelated existing drift in
  `src/vdi/resources.rs`; that file was not changed.
- `git diff --check`: passed.

The first `.90` formatting route timed out during SSH connection and provided
no code evidence; the non-duplicate file-scoped gate was rerouted to `.196`.

## Remaining acceptance

First-release packaging and the deferred, non-blocking post-release installed
proof remain: publisher credential activation/rotation, route captures,
recovery behavior, and authenticated live RDP rendering on one available seat.
