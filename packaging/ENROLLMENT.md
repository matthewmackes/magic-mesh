# Post-install enrollment (PKG-10)

After the `magic-mesh` RPM installs (or the Magic-on-Cosmic ISO boots),
a box is **unpinned** — `mackesd` refuses to start its worker pool until
a role is pinned (ENT-2 fail-closed). Two paths:

## Found a new mesh (the first lighthouse)
```
mackesd mesh-init --mesh-id <name> --external-addr <public-ip>:4242
```
Mints the CA, self-signs this node as the founding lighthouse, prints a
single-use join token (ENT-1).

## Join an existing mesh
On a lighthouse, mint a token:
```
mackesd enroll-token --mesh-id <name>
```
On the new box, redeem it (single-use, ENT-1):
```
mackesd join 'mesh:<id>@<lighthouse-ip>:4243#<bearer>'
```
The token is self-contained — it already embeds the lighthouse address
you passed to `enroll-token` (or this lighthouse's own public address if
omitted). Copy it verbatim; there is no separate IP to hand-carry.
The role is pinned by the install chooser (Workstation) or
`mackesd role-pin <server|lighthouse>` for headless nodes; downgrades
are refused (PKG-7 / upgrade-only).
