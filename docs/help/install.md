# Install Guide

Magic Mesh ships as **one signed RPM** with an install-time role chooser. There is
no separate package per node type — the role you pick decides what runs.

## 1. Get the bits

Two supported paths:

- **Magic-on-Cosmic ISO** — boot it, and the installer's `%post` role chooser
  pins the node's role during install. Best for a fresh machine.
- **COPR repo** on an existing Fedora (Cosmic) host:

  ```bash
  sudo dnf copr enable magicmesh/magic-mesh
  sudo dnf install magic-mesh
  ```

The RPM installs every binary (`mackesd`, `meshctl`, `magic-fleet`, `mde-bus`,
`mde-workbench`, `mde-files`, …) under `/usr/bin`, the systemd units, the Carbon
icon set, and the help docs under `/usr/share/mde/help/`.

## 2. Pick this node's role

```bash
meshctl install --role lighthouse   # or: server | workstation
```

`meshctl install` pins the role (so the role-gated systemd units self-gate) and
then runs `meshctl doctor` to preflight the prerequisites (nebula, nebula-cert,
ansible, firewalld). Roles nest by capability:

- **Lighthouse** — the relay + CA + leader control plane. Start here: the first
  node in a new mesh is a lighthouse.
- **Server** — everything a lighthouse runs, plus fleet automation + LizardFS.
- **Workstation** — everything a server runs, plus the Cosmic desktop, voice,
  media, and KDC.

## 3. Bootstrap or join

- **First node ever** (new mesh): see `node-setup.md` → "Bootstrap a new mesh".
- **Joining an existing mesh**: get a single-use enrollment token from any
  lighthouse, then:

  ```bash
  meshctl join --token <token>
  # or, to also pin the role in one step:
  meshctl provision --role server --token <token>
  ```

## 4. Verify

```bash
meshctl doctor              # binaries + service + overlay link
meshctl status              # this node + fleet status
meshctl test connectivity   # overlay reachability across the fleet
```

A healthy node shows the `mackesd` service active and an overlay IP on `nebula1`.

## Notes

- The envelope is **≤8 peers** in one mesh (see `DISCLAIMER.md`). Split larger
  groups into separate workgroups.
- All node management is `meshctl` + the Workbench. Run `meshctl --help` for the
  full lifecycle command set.
