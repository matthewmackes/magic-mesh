# Workloads self-service — local VMs, services, and images

Construct's **Workloads** surface is the provider-neutral control point for
machines and services on the mesh. It drives local libvirt/QEMU-KVM through
OpenTofu, configures targets with Ansible, stages Podman/Quadlet services, and
manages bootc/osbuild images. There is no separate cloud login or web portal.

## Safety model

The shipped root DRM shell collects exact typed confirmation and mints one
30-second, single-use HMAC capability. It publishes that capability with a typed
`action/cloud/<verb>` request; `mackesd` validates the exact verb, placement node,
workload/image target, expiry, signature, and durable replay claim. The HMAC key
is never published on the world-readable Bus. A mutation without valid authority
is staged or reported as gated—it must never be shown as applied.

Fleet's Datacenter controls and the local-VM Chooser use the same credential,
full-request digest, and durable nonce ledger for their direct-libvirt actions.
Their read-only roster refresh remains available without a capability; every
request that can reach `virsh` or `qemu-img` is authorized first.

Windowed/user-session shell launches cannot mint. This is intentional: live
mutation and desired-state publication are restricted to the root-owned
physical-seat service. Read, status, and dry-run plan/check operations remain
available without mint authority; a draft can be edited elsewhere, but it is
not published as desired state until an authorized seat confirms it.

### Provision live-mutation authority (operators)

The RPM ships `/usr/libexec/mackesd/provision-cloud-arm-credential`. On one
already enrolled node, initialize the whole-mesh sealed secret and install its
host-bound credential:

```console
sudo /usr/libexec/mackesd/provision-cloud-arm-credential --init --restart
```

After every new node has run `mcnf-secret.sh init-self`, an existing secret holder
must reseal the store to the new recipient. Then run this on the new node:

```console
sudo /usr/libexec/mackesd/provision-cloud-arm-credential --restart
```

The helper decrypts the existing `cloud-arm-key` only into a root-only temporary
directory, encrypts it with `systemd-creds`, and installs
`/etc/credstore.encrypted/cloud-arm-key` mode 0600, then installs
`LoadCredentialEncrypted` drop-ins for `mackesd.service` and
`mde-shell-egui.service`. systemd exposes the plaintext only in each unit's
private read-only `$CREDENTIALS_DIRECTORY`. Missing, malformed, unsealed, or
non-root access fails closed; it never falls back to an environment variable.

Always check the operation result:

- **Applied** means the daemon completed the requested backend operation.
- **Staged** means desired state or a plan exists, but no live mutation ran.
- **Gated** names a missing tool, authority, placement node, or backend.
- **Error** is a real failure; retry only after addressing the reported cause.

## Provision a VM

1. Open **Workloads** and choose **Provision**.
2. Select an explicit mesh node. Blank placement is invalid.
3. Choose the VM delivery type and enter its name, CPU, memory, disk, image,
   and network settings.
4. Review the OpenTofu plan.
5. Arm the request and apply it. Unarmed requests remain dry runs.

The daemon stores one desired document per workload and placement node. OpenTofu
renders only that node's slice and provisions against its local libvirt URI.

## Configure a workload

Use **Configure** after the workload exists. Ansible inventory is derived from
the live mesh roster rather than a static host list. Configuration requests use
the same explicit placement and mutation-authority rules as provisioning.
Secrets resolve through `mde-seal`; do not paste credentials into forms, command
arguments, or repository files.

## Containers and service workloads

The **Containers** view stages a Quadlet unit for a named placement node. The
container name and node must be ordinary path-safe identifiers. Rootless units
are the default; rootful deployment must be an explicit choice. The daemon
reports whether the unit was merely staged or installed by Ansible.

## Images

The **Images** view builds bootc/osbuild artifacts, verifies the produced hash,
and records promoted versions in the mesh image store. Build and promotion are
authorized mutations. Image names and versions are path-safe identifiers, not
filesystem paths.

## Networks and status

Nebula remains the mesh transport. Workload networks are local libvirt networks
managed through NetworkManager/nmstate; they do not replace the overlay or
automatically make a guest a trusted mesh peer. The **Status** view reports the
real provider roster, drift, plans, and backend/tool availability.

## Console and lifecycle

Console attach returns a typed SPICE, VNC, or RDP endpoint only for a workload
that actually exposes one. Start, stop, reboot, and destroy requests must target
one named workload on one selected node. A response that reports no endpoint or
an unavailable backend is an honest gate, not a successful attachment.

## Current hardening status

`WL-ARCH-007` tracks end-to-end request-contract, placement, replay protection,
and target-scoped destroy verification. Until that item is archived with live
evidence, operators should treat Workloads mutation paths as pre-release and
confirm the daemon reply plus the real libvirt/Podman state after each action.

The former OpenStack-based Cloud plane was removed on 2026-07-22. Historical
OpenStack runbooks are retained only as superseded design records and are not
valid operating instructions.
