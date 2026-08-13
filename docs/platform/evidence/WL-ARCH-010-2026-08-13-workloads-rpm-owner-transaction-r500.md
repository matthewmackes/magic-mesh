# WL-ARCH-010 — Workloads RPM owner transaction (r500)

Date: 2026-08-13

## Gap closed

The package gates checked individual grouped-service activation tokens and RPM
payload dependencies, but did not prove the upgrade transaction that transfers
runtime ownership from retired `mackesd.service` to `mackesd.target`. A reordered
or weakened scriptlet could start the sole Workloads/libvirt/Quadlet actuator
before systemd forgot the retired owner, start grouped services on a fresh
install, or block the RPM database while six process groups converge.

`install-helpers/test-boot-status-upgrade.sh` now parses the base RPM's actual
post-install script and fails closed unless it performs this ordered transition:

1. capture the retired and grouped owners' pre-upgrade activity independently;
2. disable and stop `mackesd.service`;
3. remove its local and vendor unit definitions;
4. reload systemd's owner table before enabling `mackesd.target`;
5. start the grouped owner only when the retired owner had been active; and
6. after package setup, queue a non-blocking corrected-forward restart only
   when the grouped owner had already been active.

Four in-memory hostile mutations prove the verifier rejects a retained vendor
owner, enable-before-reload ordering, an unguarded migration start, and a
synchronous grouped-target restart. This is transaction/owner verification; it
does not duplicate RPM payload assertions.

## Farm evidence

- `.90`, slot `arch010-upgrade-transaction-full-r500`:
  `bash install-helpers/test-boot-status-upgrade.sh` passed the full contract and
  rejected all four hostile fixtures.
- `.50`, slot `arch010-upgrade-transaction-syntax-r500`:
  `bash -n install-helpers/test-boot-status-upgrade.sh` passed.
- BigBoy `.130`, slot `arch010-upgrade-activation-r500`:
  `bash install-helpers/test-rpm-seat-service-activation.sh` passed its adjacent
  RPM activation contract and extracted-script syntax gate.

The checks used isolated farm workspaces. No live systemd manager or installed
seat was mutated.

## Remaining acceptance

ARCH-010 still requires the full release RPM/repository transaction, real
libvirt/Quadlet `StartAndAttach` readiness, KMS/Display1 recovery, and the
post-release installed-seat/fleet lifecycle matrix. Those live proofs remain
deferred and non-blocking until after the first release under the current
operator direction.
