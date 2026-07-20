# Install Guide

MCNF (magic-mesh) ships as **one signed RPM** (plus an immutable bootc/ostree
image) with an install-time role chooser. There is no separate package per node
type — one byte-identical stack ships, and the role you pick is a configuration
flag that decides which systemd units run.

## 1. Get the bits

Two supported paths:

- **Construct ISO / bootc image** — boot it, and the installer's role chooser pins
  the node's role during install. Best for a fresh machine.
- **GitHub RPM** on an existing Fedora host — one-shot, the latest release asset:

  ```bash
  sudo dnf install \
    https://github.com/matthewmackes/magic-mesh/releases/latest/download/magic-mesh.rpm
  ```

  That one-shot install also leaves the update channel behind: the RPM itself
  ships the `[magic-mesh]` dnf repo to `/etc/yum.repos.d/` and the project's
  public signing key to `/etc/pki/rpm-gpg/` (GitHub Pages baseurl, gpgcheck
  on), so a plain `sudo dnf upgrade` picks up later releases. There is no
  separate release/bootstrap RPM and no COPR.

The package installs the control-plane binaries (`mackesd`, `meshctl`,
`magic-fleet`, `mde-bus`, …), the **egui/DRM desktop shell** (`mde-shell-egui`)
and its surfaces, the systemd units, the role-scoped assets, and the help docs
under `/usr/share/mde/help/`. The desktop is DRM-native — it owns the KMS seat
directly and does **not** require a Wayland compositor.

## 2. Pick this node's role

```bash
meshctl install --role lighthouse   # or: workstation
```

`meshctl install` pins the role (so the role-gated systemd units self-gate) and
then runs `meshctl doctor` to preflight the prerequisites (nebula, nebula-cert,
ansible, firewalld, libvirt). There are **two roles**:

- **Lighthouse** — the always-on relay + Nebula CA/signer + leader control plane
  (and media server). No local desktop. Start here: the first node in a new mesh
  is a lighthouse.
- **Workstation** — the full Construct egui thin client: it brokers and displays VM
  desktops and runs libvirt/QEMU-KVM + Podman. A **headless** box is a
  Workstation with no local display (daemon stack only, serving VMs/containers to
  the mesh) — "headless" is a capability, not a separate role.

Role is a flag: a box is re-roled with `meshctl install --role <role>` (upgrade
only — Lighthouse → Workstation is fine; downgrades fail closed) without a
reinstall.

## 3. Bootstrap or join

- **First node ever** (new mesh): see `node-setup.md` → "Bootstrap a new mesh".
- **Joining an existing mesh**: get a single-use enrollment token from any
  lighthouse, then:

  ```bash
  meshctl join --token <token>
  # or, to also pin the role in one step:
  meshctl provision --role workstation --token <token>
  ```

## 4. Verify

```bash
meshctl doctor              # binaries + service + overlay link
meshctl status              # this node + fleet status
meshctl test connectivity   # overlay reachability across the fleet
```

A healthy node shows the `mackesd` service active and an overlay IP on `nebula1`.

## Notes

- The infrastructure envelope is a small flat-trust workgroup (see
  `DISCLAIMER.md`); VM desktop guests are first-class members on top of it. Split
  larger groups into separate workgroups.
- All node management is `meshctl` (and the `mackesd` daemon CLI), plus the egui
  shell's **Workbench** surface (opened from the dock), whose five planes — **This
  Node**, **Cloud**, **Network**, **Fleet**, and **Provisioning** — are the
  graphical mesh view. Run `meshctl --help` for the full lifecycle command set.
