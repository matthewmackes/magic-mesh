# Per-Node-Type Setup

Every MCNF node is one of three roles. They nest by capability:
**Lighthouse ⊂ Server ⊂ Workstation** — a Server runs everything a Lighthouse
does plus more, and a Workstation runs everything a Server does plus the desktop.
Pin a role with `meshctl install --role <role>` (or the ISO's install-time
chooser).

## Bootstrap a new mesh (the first Lighthouse)

The very first node has no one to enroll with, so it bootstraps the mesh and mints
the CA:

```bash
meshctl install --role lighthouse
meshctl mesh init            # mint the CA + this lighthouse's cert, bring up nebula1
meshctl doctor               # confirm nebula1 is up and mackesd is active
```

A Lighthouse is the relay, the Nebula CA, and the leader control plane. At least
one must be reachable for new peers to enroll and for the fleet to elect a leader.
For resilience, promote a second node to Lighthouse once the mesh has a few peers
(losing your only lighthouse is the one painful case — see `mesh-recovery.md`).

## Add a Server

A Server adds fleet automation (Ansible-on-each-node, jobs, netstate/firewall
convergence) and LizardFS replicated storage.

```bash
# On a lighthouse: mint a single-use enrollment token.
mackesd enroll-token --mesh-id <mesh>   # prints a token

# On the new node:
meshctl provision --role server --token <token>
meshctl doctor
```

Tag a Server with the `execution` capability if it should run fleet jobs:

```bash
mackesd tag --set execution
```

## Add a Workstation

A Workstation is a Server plus the Cosmic desktop, voice/media services, and the
KDC. Same enrollment flow:

```bash
meshctl provision --role workstation --token <token>
```

Log into Cosmic; the MCNF applet shows the mesh-health pip and quick
actions, and the Workbench is the console for Peers, Health, Config, Jobs, and
the network planes.

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
- `headless` — no desktop expected.

## Verify any node

```bash
meshctl status              # node + fleet
meshctl fleet status        # every enrolled node
meshctl test connectivity   # overlay reachability
meshctl test dns            # <host>.mesh resolution wired
meshctl test firewall       # nebula1 in the trusted zone
```
