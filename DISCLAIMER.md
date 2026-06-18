# MCNF — Warning, Disclaimer, and Mission Statement

MCNF is an open-source, secure, no-fixed-center **workgroup** platform: a
small set of trusted machines (a household, a lab, a small team) joined into one
encrypted Nebula overlay with replicated storage (LizardFS), peer-to-peer fleet
automation (Ansible-on-each-node), and a Cosmic desktop. Its mission is to make a
**production-grade private workgroup of up to eight peers** something one operator
can stand up, run, and recover — without a cloud control plane, a central server,
or a fixed point of failure.

## Production envelope (what it IS for)

MCNF is **production workgroup-grade within a stated envelope**:

- **Scale:** up to **8 peers** in one trust envelope (the §8 lock). Beyond that,
  split into multiple workgroups.
- **Trust model:** a **flat, open mesh** — every enrolled peer fully trusts every
  other peer at the network layer (one open Nebula firewall rule). Membership is
  gated by an Ed25519/Nebula cert issued at enrollment; revocation is immediate
  and fleet-wide via the CA blocklist. There is **no per-peer network
  segmentation inside the overlay** — see "Blast radius" below.
- **Roles:** every node is a Lighthouse, Server, or Workstation
  (Lighthouse ⊂ Server ⊂ Workstation by capability). One signed RPM; the
  install-time role chooser decides what runs.
- **Crypto floor:** Ed25519 signing, AES-256-GCM / ChaCha20-Poly1305 transport,
  RSA-4096 for the KDC. Nebula is the only overlay transport; there is no
  fallback to an unencrypted path.

It is suitable for a trusted small workgroup running real services. It is **not** a
hyperscale fleet manager, a zero-trust micro-segmentation product, a managed
service, a compliance appliance, or a guaranteed-availability system, and it makes
no SLA. Use beyond the ≤8-peer envelope, or in regulated / safety-critical /
life-critical settings, requires independent security, legal, and operational
review.

## Blast radius (the accepted open-mesh trade-off, C7)

The flat-trust design is a deliberate trade for simplicity and resilience at small
scale. The accepted consequence: **any compromised or misbehaving peer can reach
every other peer on every port inside the overlay.** The mesh contains the blast
radius by (a) keeping the envelope small (≤8 peers you actually trust), (b) gating
membership on a per-node cert, (c) making revocation immediate and fleet-wide, and
(d) keeping the underlay firewall tight so only the overlay is exposed. If you need
peer-to-peer network isolation *within* the group, MCNF is not the right
tool — run it only among machines you would trust on the same LAN.

## Data, replication, and recovery

MCNF replicates data across peers (LizardFS) and converges node
configuration peer-to-peer. Misconfiguration can still cause data loss, service
interruption, or unintended replication of sensitive files. You remain responsible
for backups, for what you place on the mesh, and for recovery. The platform ships
an operator runbook for the one genuinely scary case — losing your lighthouse — at
`docs/help/mesh-recovery.md` (installed to `/usr/share/mde/help/`).

## Warranty and liability

MCNF is provided "as is" and "as available," without warranties of any kind,
express or implied — including security, reliability, availability, performance,
legal compliance, fitness for a particular purpose, data integrity, or suitability
for a given deployment. Support is community/best-effort as described in
`SUPPORT.md`; no SLA, incident response, or recovery assistance is implied unless
separately agreed in writing.

By installing, using, modifying, distributing, or operating MCNF you accept
full responsibility for all risks and outcomes. Do not deploy it on systems,
networks, data, or accounts you do not own or have explicit permission to manage.
To the maximum extent permitted by law, the authors, maintainers, contributors,
and distributors are not liable for any direct, indirect, incidental,
consequential, special, exemplary, or punitive damages arising from its use,
misuse, configuration, modification, distribution, failure, or operation.

MCNF integrates third-party open-source components; each retains its own
license, copyright, and notices. Nothing here supersedes those terms — review and
comply with all applicable upstream licenses.

Use MCNF at your own risk. If you do not understand the risks, do not
install or use it.
