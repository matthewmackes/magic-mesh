# WL-FUNC-023 — turn-key fresh install, heal nags, and 2026-08-24 fleet solutions

Operator 2026-08-24: document dirty-upgrade leftovers vs onboard/health/heal
gaps, with corrected-forward solutions. Fresh install must be user-friendly
and turn-key. If a service is disabled or broken, Construct must nag with an
automated typed-mackesd fix (not silent fail, not SSH). `production_admitted`
unchanged. This note is an evidence **input** to `WL-FUNC-023` S11/S13/S17;
it is not a second worklist.

Live topology at write: mesh `mcnf-clean-20260728`; LH `10.42.0.1`–`.3`;
Dell `10.42.0.4`, Seat 15 `10.42.0.5`, Surface `10.42.0.7`; unpublished
`magic-mesh-13.0.0-35` (`7e3474eeb`).

## Nag contract (Construct)

Reuse the existing System and Mesh Health modal and `event/toast/show`.
Do not add a parallel Notifications surface (chat already subsumes that).

Each blocking or warning condition MUST carry:

1. Honest title and observed fact (no “unknown” when systemd is explicit).
2. One primary **Fix** that publishes a typed `mackesd` verb / lifecycle
   `VerifyAndCorrect` step (same path as `health_pending_action` in
   `mde-shell-egui` `health_modal.rs`).
3. Red `AI-GENERATED-ALERT` + five-second hold for seat-mutating fixes.
4. Stop on integrity/auth/health failure; recover corrected-forward.
5. If Fix is impossible without a dest (identity receipt, arming
   credential, Vitelity), the nag says so and offers the ONBOARD landing —
   never invents a mesh-id, bearer, or unsigned JSON.

`pending-convergence` is itself a nag source: first-boot must publish a
toast and a Health row, not only a systemd failed unit.

## Findings and solutions

### F1. Firstboot always Pending (`units` + `verification`)

**Class:** health/onboard gap. A clean `13.0.0-35` install still fails.

**Cause:** `gather_live` uses `units_for_role()`, which still expects
monolithic `mackesd.service` and `etcd.service` on every node, and includes
`mcnf-lifecycle-firstboot.service` itself. Workstations run grouped
`mackesd-*.service` (postinstall removes monolithic `mackesd.service`).
Workstations are etcd **clients**. While firstboot is `activating`,
`systemctl is-active --quiet mcnf-lifecycle-firstboot.service` fails, so
the unit can never stamp `firstboot-converged` on its own boot.

**Solution:** `runtime_expected_units(role)` for firstboot facts:

- Never require `mcnf-lifecycle-firstboot.service` (self).
- Workstation: require the six grouped units when
  `mackesd-control.service` is shipped; do not require `mackesd.service` or
  `etcd.service`.
- Lighthouse: keep `mackesd.service` + `etcd.service`.
- After groups are active, stamp `firstboot-converged` or nag with Fix =
  `lifecycle-firstboot` retry (idempotent).

### F2. Missing `/etc/mackesd/etcd-endpoints` after join

**Class:** onboard gap (Seat 15 only had the file after leftover-3 reenroll).

**Solution:** `join` / finish_network_enrollment MUST run packaged
`setup-etcd --client-only --anchors <lighthouse overlay IPs from bundle>`
on workstation/server. Fail-closed if anchors unknown. Heal nag: Fix runs
the same helper from live overlay neighbors (never invents IPs). Secret
store then works without SSH.

### F3. Collaboration identity dest not produced by join

**Class:** onboard dest gap. Groups `Requires=` the materializer.

**Solution:** turn-key path is capsule/token carrying the node-scoped
signed receipt + seed put (FUNC-023 S6/S11). Until that capsule exists,
nag: “Collaboration identity missing” with Fix opening ONBOARD (do not
unsigned-JSON, do not copy another node’s receipt). Operator-admitted
receipts on Dell/Seat 15/Surface stay valid.

### F4. Empty `/var/lib/mackesd/nebula/overlay-ip` after reboot

**Class:** heal gap. `nebula1` was up; file was 0 bytes. Supervisor pin
mismatch refused config refresh; sshd_overlay_bind deferred.

**Solution:** control-group publisher writes overlay-ip from `nebula1`’s
global IPv4 every 5s even when supervisor skips rewrite. Heal nag: Fix =
rewrite from live iface (what the 2026-08-24 Surface repair did by hand).

### F5. `nebula_supervisor` relay pin ≠ local enrollment pin

**Class:** heal gap (fail-closed is correct). Overlay can still forward.

**Solution:** nag with observed pin mismatch. Fix = VerifyAndCorrect
re-materialize only when the replicated bundle’s authority matches the
enroll-endpoint dest; otherwise offer leftover-3 reenroll, never a silent
overwrite.

### F6. Orphan grouped `mackesd` PIDs / “worker group already has a live owner”

**Class:** dirty upgrade leftover + heal gap (`systemctl stop` left Aug-23
PIDs).

**Solution:** upgrade/stop must `KillMode=` the cgroup; heal Fix = stop
unit, SIGKILL leftover owners of that group, start once. Nag names the
PID/start time. Fresh install avoids orphans; heal still required after
any failed stop.

### F7. XDG bind refuse: local data would be obscured

**Class:** heal gap. Empty root `.mde-vdi-clipboard-staging` in Downloads
blocked communal binds. VDI worker wrote local before binds.

**Solution:** worker must create staging on the communal tree (or after
binds). Heal: if the only occupant is empty `.mde-vdi-clipboard-staging`,
relocate or rmdir then retry bind (2026-08-24 Surface). Non-empty user
files: nag “Move to mesh Downloads?” with Fix = copy into
`/mnt/mesh-storage/home/Downloads` then bind — never delete.

### F8. Lighthouse `/run` full, Nebula Recv-Q wedge, etcd MemoryMax 128M

**Class:** dirty age + heal/health gap on thin droplets.

**Solution:** retention must keep `/run/mde-bus` under 50% bytes and 25%
inodes (already intended in `bus_retention_policy`). Heal nag on
`bus/run-low`: Fix = stop grouped/thin mackesd, prune spool, start
(historical BUS-RETENTION workaround). Nebula Recv-Q > 64KiB: Fix =
restart nebula **without** `reload-or-restart` storm (supervisor must not
HUP a wedged process in a loop). etcd catch-up: packaged thin profile
MemoryMax ≥ 256M (live drop-in `30-catchup-headroom.conf` on LH1). Quorum:
mutate one lighthouse at a time.

### F9. Cloud arming credential unavailable

**Class:** onboard dest gap. Privileged Bus mutations stay disabled.

**Solution:** same dest discipline as identity. Nag + ONBOARD Fix. Do not
bypass `Requires=` or invent systemd credentials.

### F10. Browser VM / `mcnf-node-virt` inactive on Dell and Surface

**Class:** onboard gap. Seat 15 has running `browser-vm`; others have KVM
but no guests. Fresh install does not create the VM.

**Solution:** Workstation onboard S11 must admit `mcnf-node-virt` and the
signed Browser VM workload. Health nag: “No Browser VM” with Fix =
typed Workload start (never raw virsh). Missing image dest → ONBOARD, not
a fake guest.

### F11. Syncthing active but `/mnt/mesh-storage` not a mountpoint

**Class:** health honesty. Directory-on-root can be the file plane.

**Solution:** health must publish whether the Syncthing folder is the
governed path, peer-connected, and XDG binds exact. Nag if folder missing
or binds down; Fix = `setup-syncthing --listen <overlay>` (idempotent,
join already documents this) plus XDG recovery.

### F12. 66 pending enrollment tokens (Surface)

**Class:** dirty onboard leftover. Firstboot must not decrement tokens
(existing lock).

**Solution:** Health lists count only (no token bodies). Fix = leftover-3
style dest-signed leave/join when operator authorizes; do not drain tokens
from SSH.

### F13. Stale RPM vs in-tree GUI (Transfers, hotkeys, Files)

**Class:** dirty/stale package, not heal. Set aside for REL cut.

**Solution:** unpublished current-revision candidate on the three seats
(TEST-002). Nags must not claim HEAD features on `13.0.0-35`.

### F14. Feature lane (Calls media, mesh mount, SIP toml, co-edit)

**Class:** product leftovers (FUNC-024–032), not install dirt.

**Solution:** remain on those epics. Core heal nags do not fake media or
invent `gateway.toml`.

## Fresh-install definition of done (this note)

A newly imaged Workstation that redeems a valid join token, without SSH:

1. Overlay up, overlay-ip published, etcd-endpoints written, Syncthing on
   `/mnt/mesh-storage`, grouped mackesd active, Construct on DRM.
2. Firstboot stamps `firstboot-converged` or a Fix-able nag — never a
   silent failed unit plus Ready-looking shell.
3. Identity/arming/Browser VM missing → nag + ONBOARD Fix, not inactive
   groups with no toast.
4. Any later disable/break → Health row + toast + one automated Fix.

Live 2026-08-24 dests (identity on three seats, LH overlay repair, Surface
XDG/overlay-ip) are operator-corrected examples of what Fix must do
unattended.
