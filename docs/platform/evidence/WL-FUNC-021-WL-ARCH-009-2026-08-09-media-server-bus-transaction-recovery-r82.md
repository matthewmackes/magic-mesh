# WL-FUNC-021 / WL-ARCH-009 media-server Bus transaction recovery r82

Date: 2026-08-09

## Scope and result

Owned production source:
`crates/mesh/mackesd/src/workers/media_server.rs`.

The media-server worker no longer retains one `Persist` connection after
activation. Each library pass resolves the explicit/current/system Bus root,
brackets `Persist::open` plus `reopen_if_index_changed()` with path identity
observations, and requires the opened handle inode to match the accepted
device/inode identity. The write is verified against that same identity before
the worker commits its publication fingerprint, serving map, active Bus
generation, or heartbeat schedule. Late and same-path replacement therefore
recover in the same supervised worker without restarting its HTTP/SSDP process
state. Replacement during open is rejected; replacement during append leaves
the fold pending and corrected-forward publication occurs on the live index.

The replicated manifest plane now has a strict production transaction path.
Own-manifest write failure, mount discovery failure, directory-entry failure,
or any admitted manifest-file read failure defers the complete fold. Such a
fault cannot masquerade as an empty/partial mesh library or mutate the serving
map. Missing per-host manifests remain normal absence. The production fold now
rejects malformed JSON, host-directory/manifest identity disagreement,
symlinks, non-regular files, files larger than 16 MiB, more than 4,096 host
entries, and files replaced while being read. The public best-effort helper is
retained for its existing callers/tests, while the worker uses only the
complete bounded `Result` path.

Publication and serialization are explicit `Result` operations. Fingerprint
and heartbeat state advance only after a verified Bus append. A failed forced
heartbeat remains due; a changed fold remains different from the last
successful fingerprint; and a changed Bus identity forces a complete current
fold even when media content is unchanged.

## Focused farm verification

Host: machine9 `172.20.0.50`

Slot: `MCNF_BUILD_SLOT=media-server-bus-r82`

The clean farm stage contained clean `HEAD` plus only the owned media-server
source. The initial exact build/test command was:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-server-bus-r82 \
  ./install-helpers/xcp-build.sh cargo test -p mackesd \
  --features async-services --lib \
  workers::media_server::tests::publication_replacement_defers_state_and_corrects_forward \
  -- --exact --nocapture
```

Result: PASS, `1 passed; 0 failed; 4,591 filtered out`. Cold build and link
completed in 5m03s.

The linked farm library-test binary then ran these additional fully qualified
tests individually with `--exact --nocapture`:

```text
workers::media_server::tests::open_race_and_same_path_replacement_recover_without_worker_restart
workers::media_server::tests::unreadable_manifest_lane_defers_projection_and_serving_state
workers::media_server::tests::late_bus_recovers_and_publishes_library_without_worker_restart
workers::media_server::tests::media_server_bus_root_preserves_override_and_has_system_fallback
workers::media_server::tests::worker_writes_manifest_aggregates_and_publishes
```

Result: PASS for all five, each `1 passed; 0 failed; 4,591 filtered out`.
Together with the initial gate, all six exact tests passed. The hostile proofs
cover replacement between open/reopen and identity acceptance, a second
same-path replacement after activation, replacement after append before state
commit, corrected-forward publication without worker restart, unreadable
manifest-lane deferral with unchanged serving/projection state, bounded
shutdown, canonical system fallback, and the normal aggregate path.

After the final admission refinement (non-directory mount entries are ignored,
while entry metadata failures still defer), the exact unreadable-manifest test
was rebuilt and rerun on the same host/slot. Result: PASS, `1 passed; 0 failed;
4,591 filtered out`; incremental rebuild completed in 1m14s.

Main-agent review found that the first complete-fold implementation still
silently skipped malformed JSON, followed manifest symlinks, and admitted
unbounded files/directories. Those were corrected before integration. The
linked machine9 test binary was rebuilt with the final source and both exact
admission gates passed:

```text
workers::media_server::tests::hostile_manifest_files_fail_the_complete_fold
workers::media_server::tests::unreadable_manifest_lane_defers_projection_and_serving_state
```

Result: PASS for both, each `1 passed; 0 failed; 4,603 filtered out`. The new
hostile gate covers malformed JSON, host mismatch, a sparse oversize manifest,
and a symlink without admitting any partial projection.

Farm file-scoped formatting and identity gates:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/media_server.rs
sha256sum crates/mesh/mackesd/src/workers/media_server.rs
```

Result: PASS. Farm and local source hashes matched. Local scoped
`git diff --check` also passed.

## Hash

```text
a840c255dd2bef027bdd9d9e60eeadc0cb933f6d1fce532c7c28ed0f4cf8ea3f  crates/mesh/mackesd/src/workers/media_server.rs
```

## Residual caveats

- QNM manifest persistence and Bus append are separate stores. A Bus failure
  after the atomic manifest rename can leave the plane ahead of the Bus, but no
  serving/fingerprint/heartbeat state is committed and the next complete pass
  republishes the current fold.
- Bus append and path-identity validation are not one filesystem primitive. A
  replacement immediately after successful validation is detected by the next
  per-pass identity comparison and receives a forced complete fold; restart
  also performs an unconditional initial publication.
- No live port bind, SSDP multicast, DLNA client, broad package suite, WORKLIST
  edit, commit, or push was performed.
