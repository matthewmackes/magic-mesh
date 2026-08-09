# WL-ARCH-010 Dell and seat-15 live acceptance r15 — 2026-08-09

Status: live acceptance remains refused. This slice was read-only on both seats: no package deployment, service restart, VM operation, reboot, Bus publication, or credential mutation occurred.

## Proof identity

- Repository revision: `3cc8b098e92282f544839253b3ab0b9ee1e08f2d`.
- Verifier: `install-helpers/verify-workloads-live-proof.py`, SHA-256 `85f293813e4203cd713db18ac9165a6fb2b38e0dac1b9a0b4fde53c585d24f2b`; the file had no tracked diff.
- BigBoy `172.20.0.130`, disposable slot `arch010-live-r15`: Python compilation and `--self-test` passed. The slot was then removed.
- Governed targets from `docs/BUILD-ENVIRONMENT.md`: Dell `172.20.146.225` / `10.42.0.4`; basement seat 15 `172.20.0.15` / `10.42.0.5`.

## Dell

Direct SSH to `mm@172.20.146.225` failed with `No route to host`. From seat 15, bounded ICMP and TCP/22 probes to both `172.20.146.225` and `10.42.0.4` were also unreachable. No installed package, revision, typed Workload projection, runtime, attachment, or VM identity claim is made for Dell.

Minimal governed prerequisite: restore Dell's governed LAN or Nebula reachability, then inventory its installed package and binary digests before deciding whether rollout is required. If it differs from the promoted revision-bound candidate, perform the normal corrected-forward package rollout; do not infer adoption from an old Dell receipt.

## Basement seat 15

The strict current-tree verifier was streamed over SSH and executed locally on `Basement-Test-Workstation` with `--node "$(hostname)" --require-all --json`; it returned exit 2 as required for an incomplete acceptance.

Installed identity:

- package `magic-mesh-12.1.6-23.x86_64`, built `2026-08-08 23:00:40 EDT`, installed `23:01:43 EDT`; `rpm -V` reported no payload differences under the bounded verification options;
- `/usr/bin/mackesd` SHA-256 `a81c8aa4b43ef923ddd508bb23d1cbbf92f2ce8b7f657219bd1f480976b510ab`;
- `/usr/bin/mde-shell-egui` SHA-256 `b1abe7822f5e3c85220a3a30f8c84b5a84bbf219d99d3d4fa7056e1efc3933e1`;
- no installed release manifest or source-revision receipt was present at the governed candidate paths. NEVRA and binary digests bind this observation to exact payload bytes, but cannot establish a source revision.

Observed runtime:

- all six grouped mackesd services and `mde-shell-egui.service` were active; compute and shell each reported `NRestarts=0`;
- Workstation placement, Podman, KVM, the active `default` network, and active `mde-vms` pool passed;
- the encrypted cloud credential existed root-only, but `/etc/systemd/system/mackesd-compute.service.d/50-cloud-arm-credential.conf` was absent;
- the fresh cloud mirror was healthy for OpenTofu, Ansible, and libvirt, but `apply_armed=false`;
- no unambiguous `state/workloads/Basement-Test-Workstation` projection, retained typed Workload operation, or successful open-broker acknowledgement existed.

The exact local libvirt object was `browser-vm`, UUID `5c299393-fa06-458d-9afe-c6fe56b3b458`, persistent and autostart-enabled, with inactive XML SHA-256 `ba4c29d827e5ce795b7d858c822137edfda45e2adfaa1dcbf2224f1d850316d8`. It was shut off with reason `failed`; its disk was `/var/lib/libvirt/images/browser-vm-seat15-r1-overlay.qcow2` and seed CD-ROM was `/var/lib/libvirt/images/browser-vm-seat15-r1-seed/seed.iso`.

That libvirt identity is inventory only. Because the authoritative typed projection is absent, it cannot be bound to a Workload ID, runtime generation, observed phase, adapter evidence, attachment protocol, or attachment generation. No attachment or successful VM operation is claimed.

Minimal governed rollout prerequisite: promote a package that carries an immutable source-revision receipt, verify its payload includes the grouped Workload reconciler and credential-provisioning contract, install it corrected-forward on seat 15, and materialize the compute credential drop-in through the governed provisioner. Acceptance must then issue one authorized typed Workload operation and require a fresh projection whose Workload identity, runtime evidence, attachment generation, and exact VM UUID all agree. A reboot is not intrinsically required.

## Disposition

The verifier showed no concrete integrity defect, so it was not modified. Dell reachability and seat-15 revision/projection/operation/attachment evidence remain open; this evidence does not close WL-ARCH-010.
