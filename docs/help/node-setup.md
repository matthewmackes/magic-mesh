# Per-Node-Type Setup

Every MCNF node runs the **same byte-identical stack**; **role is a configuration
flag, not a build**. There are **two rank-ordered roles**:
**Lighthouse (rank 0) → Workstation (rank 1)**. Pin a role with
`meshctl install --role <role>` (or the ISO's install-time chooser); re-roling is
upgrade-only (a downgrade fails closed).

## Bootstrap a new mesh (the first Lighthouse)

The very first node has no one to enroll with, so it bootstraps the mesh and mints
the CA:

```bash
meshctl install --role lighthouse
meshctl mesh init            # mint the CA + this lighthouse's cert, bring up nebula1
meshctl doctor               # confirm nebula1 is up and mackesd is active
```

A Lighthouse is the relay, the Nebula CA/signer, and the leader control plane. At
least one must be reachable for new peers to enroll and for the fleet to elect a
leader. For resilience, promote a second node to Lighthouse once the mesh has a
few peers (losing your only lighthouse is the one painful case — see
`mesh-recovery.md`).

## Add a Workstation

A Workstation is the full Quasar egui thin client. It brokers and displays VM
desktops (libvirt/QEMU-KVM through Nova), runs Podman, and carries fleet
automation (Ansible-on-each-node, jobs, netstate/firewall convergence) plus a
Syncthing replicated-storage replica. Enroll it with a single-use token:

```bash
# On a lighthouse: mint a single-use enrollment token.
mackesd enroll-token --mesh-id <mesh>   # prints a token

# On the new node:
meshctl provision --role workstation --token <token>
meshctl doctor
```

Once enrolled, a Workstation with a display boots the egui/DRM shell; a
**headless** Workstation (no display) runs the daemon stack only and serves its
VMs/containers to the mesh. Tag a node with the `execution` capability if it
should run fleet jobs:

```bash
mackesd tag --set execution
```

## Capability tags (optional, any role)

Tags gate fleet behavior independent of role:

```bash
mackesd tag                       # show this node's tags
mackesd tag --set execution,hop   # replace the tag set (audit-logged)
```

- `execution` — may run fleet job playbooks (hard gate; untagged nodes refuse).
- `hop` — may advertise underlay subnets / act as a gateway (`mackesd
  hop-advertise --subnets <cidr>`; add `--exit` for a full exit, which only
  activates once a validation run passes).
- `headless` — a Workstation with no local display expected (daemon stack only).

## Verify any node

```bash
meshctl status              # node + fleet
meshctl fleet status        # every enrolled node
meshctl test connectivity   # overlay reachability
meshctl test dns            # <host>.mesh resolution wired
meshctl test firewall       # nebula1 in the trusted zone
```

On a Workstation with a display, the egui shell's **Workbench** — the mesh-control
surface opened from the dock — is the visual view of the fleet across its five
planes: **This Node**, **Cloud**, **Network**, **Fleet**, and **Provisioning**.
