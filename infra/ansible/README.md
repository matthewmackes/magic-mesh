# infra/ansible — build-farm config plane (DEVOPS-SUBSTRATE)

Idempotent config for the build farm — the durable replacement for the
SSH-driven `install-helpers/setup-build-vm-toolchain.sh` /
`xcp-authorize-farm-key.sh`. Pairs with `infra/tofu/` (which creates the VMs):
tofu provisions, Ansible configures.

Auth is the mesh key (`/root/.ssh/mackes_mesh_ed25519`), set in `inventory.ini`.

## Plays

| play | does | proven |
|---|---|---|
| `build-vm-toolchain.yml` | full Rust build toolchain on every build VM (dnf dev libs + rustup-pinned 1.94.0 + clippy/rustfmt + cargo-generate-rpm) | live inventory verified 2026-07-06 on `.50`, `.90`, `.130`, `.170`; all four are reachable/toolchained via `install-helpers/farm.sh status` |

## Run

```sh
cd infra/ansible
ansible build_vms -m ping                 # connectivity
ansible-playbook build-vm-toolchain.yml    # converge the toolchain (no-op when ready)
ansible-playbook build-vm-toolchain.yml -l mcnf-build-home-services   # one node
```

## Inventory

- `build_vms` — the Fedora build VMs (`mm` user): `172.20.0.50`, `172.20.0.90`, `172.20.0.130`, `172.20.0.170`.
- `dom0s` — the XCP hypervisors (`root`): `172.20.0.9`, `172.20.145.193`, `172.20.145.165`, `172.20.145.194`.

> Needs only `ansible-core` (no extra collections — uses `ansible.builtin.*`).
