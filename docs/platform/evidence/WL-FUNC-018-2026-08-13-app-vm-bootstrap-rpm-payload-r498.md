# WL-FUNC-018 App VM bootstrap RPM payload — r498

Date: 2026-08-13

## Boundary closed

`infra/tofu/cloud/cloud-init/mesh-join.yaml.tftpl` is the packaged App VM guest
bootstrap. It carries the admitted guest profile input, the
`mcnf-app-vm-runtime.service` definition, and its runtime executable. The RPM
manifest ships its parent directory with a glob. The previous verifier proved
only that the glob destination contained at least one file, so an RPM could
omit this exact bootstrap while the package gate remained green.

`install-helpers/verify-rpm-payload.sh` now fails closed unless:

- the exact source template exists and is selected by the base RPM manifest at
  its canonical destination;
- the template still contains the guest-profile, runtime-service, canonical
  `ExecStart`, and runtime-executable boundaries; and
- real-RPM mode finds the exact installed template path in `rpm -qlp` output.

The self-test includes the prior false-green shape: a non-empty cloud-init RPM
listing with only the App VM bootstrap removed. The exact payload gate rejects
it.

## Farm evidence

- `172.20.0.130`, slot `func018-appvm-payload-selftest-r498`:
  `bash install-helpers/verify-rpm-payload.sh --self-test` passed every
  assertion, including rejection of the non-empty payload missing the exact
  App VM bootstrap.
- `172.20.0.170`, slot `func018-appvm-payload-focused-r498`:
  `bash -n install-helpers/verify-rpm-payload.sh` and
  `bash install-helpers/verify-rpm-payload.sh app-vm-payload` passed.
- `172.20.0.90`, slot `func018-appvm-payload-static-r498`:
  `bash install-helpers/verify-rpm-payload.sh payload` passed the complete
  static package verifier, including the new exact App VM boundary.

`.196` was unreachable during sync and `.50` correctly refused sync with 6.2
GiB free, below the 8-GiB safety floor. The self-test was rerouted to `.130`;
neither unavailable node is part of the claimed evidence.

## Remaining acceptance

Installed App VM boot/readiness, Front Door-to-VDI attachment, reconnect,
cleanup, sandbox, persistence, and package-upgrade proof remains deferred until
after the first full release and is non-blocking under the current operator
direction.
