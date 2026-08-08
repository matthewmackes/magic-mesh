# WL-ARCH-009 grouped systemd cutover — 2026-08-08

This checkpoint replaces the source and bootc-image launch boundary for the
retired group-less `mackesd.service` with the six process groups admitted by the
production CLI:

- `mackesd-control.service`
- `mackesd-observation.service`
- `mackesd-actions.service`
- `mackesd-data.service`
- `mackesd-compute.service`
- `mackesd-integrations.service`

`mackesd.target` requires all six services and is enabled by the bootc preset.
Each service invokes exactly `/usr/bin/mackesd serve --group <group>`, is tied to
the aggregate target with `PartOf=`, and declares independent watchdog, restart,
start-limit, memory, CPU, task, and I/O budgets. The bootc image explicitly
disables and removes the RPM-installed legacy unit before enabling the grouped
target, so no group-less command remains in that image.

The source validator now rejects unknown group units, missing target edges,
extra CLI arguments, absent resource/watchdog policy, and any surviving
`packaging/systemd/mackesd.service`. Its hostile fixture includes the previously
proposed `--sqlite-writer` argument and proves that packaging cannot invent a
flag absent from the production CLI.

## Focused farm verification

Host `.50`, slot `arch009-grouped-units-20260808-r1`:

```text
python3 install-helpers/verify-mackesd-process-boundary.py --self-test
verify-mackesd-process-boundary.py: self-test passed

python3 install-helpers/verify-mackesd-process-boundary.py
mackesd process boundary: PASS

systemd-analyze verify --root="$fixture" \
  mackesd.target mackesd-control.service \
  mackesd-observation.service mackesd-actions.service \
  mackesd-data.service mackesd-compute.service \
  mackesd-integrations.service
exit 0
```

The `systemd-analyze` fixture copied the Fedora farm host's system unit tree and
installed a non-executing `/usr/bin/mackesd` placeholder solely so the analyzer
could resolve the packaged absolute path. It reported unrelated dangling dracut
aliases inherited from the farm image; none named a mackesd unit and the command
exited zero.

Base revision: `9e553521f7b50b5dda2895d15f875df6fb459410`.
Combined SHA-256 over the target, six units, validator, bootc Containerfile,
bootc verifier, and preset: `06f77df9d6fe13073db1609007b13b42544f014d80b76bc24e613593800b70c9`.

## RPM and operational-consumer cutover

Release 13 of the base/server package metadata and release 6 of the thin
lighthouse metadata now independently ship `mackesd.target` and all six units.
No RPM variant ships `mackesd.service`. Upgrade scriptlets remember whether the
old daemon was active, stop and remove its vendor unit and known local drop-ins,
daemon-reload, enable the target, and immediately start it only on an active
upgrade. Fresh installs remain enable-only. Full removal disables the target.

The cloud mutation credential is exposed only to `mackesd-compute.service`
(the governed home of the cloud worker) and `mde-shell-egui.service`; the other
five daemon processes cannot read it. The etcd setup helper writes ordering
drop-ins for all six processes. The small-lighthouse helper places all six in a
single `mackesd-small.slice`, preserving the former aggregate 240/320 MiB and
100% CPU budget instead of multiplying it sixfold. Health recovery checks every
group individually, and the Music CPU proof aggregates all six PIDs and restart
counters.

The seat, health/status, VoIP, kickstart, cloud-init, substrate cutover,
five-seat recovery, workload live proof, Nebula rotation proof, and package
activation consumers now reference the target or exact governed group. A final
scan of `install-helpers/` and `packaging/` leaves `mackesd.service` only in
explicit hostile checks, absence assertions, preset disablement, and one-way
upgrade cleanup.

Focused farm verification on `.130` slots
`arch009-rpm-payload-r13`, `arch009-process-contract-r13`, and
`arch009-unit-hygiene-r13`, plus `.50` slot
`arch009-final-operational-r13`, proved:

```text
verify-rpm-payload.sh grouped-process: PASS (base/server/lighthouse)
verify-rpm-payload.sh payload: PASS
test-rpm-seat-service-activation.sh: contract + shell syntax PASS
verify-mackesd-process-boundary.py --self-test: PASS
verify-mackesd-process-boundary.py: PASS
bash -n + ShellCheck over migrated operational helpers: PASS
py_compile test-five-seat-core.py verify-workloads-live-proof.py: PASS
systemd-analyze verify (staged Fedora dependency root): exit 0
```

## Honest remaining runtime blocker

A partial one-persistent-SQLite-writer boundary now exists in the control
group, and seven canonical mutations cross its typed socket. Global read-only
enforcement is still blocked by the checked inventory of 61 residual direct
write sites. Those sites must be classified and migrated before built-RPM
six-process crash/recovery proof; this checkpoint proves package/process
wiring, not complete writer ownership.
