# packaging/bootc/ — the ONE immutable MCNF image (E12-13)

The §5 delivery lock: **one immutable bootc/ostree image for every role** —
egui-DRM shell + cloud-hypervisor + `mackesd` + Podman + Nebula baked in; VM
disks + mesh state on the writable partition. **Role is a config flag, not a
build**: a Lighthouse runs the byte-identical image with the desktop seat
skipped/masked. The dnf/RPM channel lane (PKG-8, gh-pages) is unchanged and
carries on in parallel — this directory adds the image lane on top of the same
monolithic `magic-mesh` RPM.

Contents:

- `Containerfile` — FROM `quay.io/fedora/fedora-bootc:42`, installs the
  `magic-mesh` RPM (two lanes, below), adds the VDI substrate
  (`podman` + `cloud-hypervisor`), wires the DRM seat, boots to
  `graphical.target`. The writable-partition doctrine is documented inline.
- `units/mde-shell-egui.service` — the Quasar **DRM-seat unit** (greetd-style,
  no display manager, no compositor — `quasar-vdi-desktop.md` lock 34). Image
  lane only; the dnf/RPM lane keeps launching the shell from a session via
  `org.magicmesh.Shell.desktop`.
- `build-image.sh` — typed-gated build driver (podman build + optional
  bootc-image-builder disk image). Refuses with an itemized list when inputs
  are missing; shellcheck-clean.
- `rpms/` — staging dir for the local-RPM lane (populated by
  `build-image.sh --rpm`; only `.gitkeep` is committed).

## Building

Context is always the **repo root** (the Containerfile COPYies
`packaging/repo/magic-mesh.repo` + `packaging/bootc/…`).

```sh
# Lane A — channel: install magic-mesh from the gh-pages dnf repo (needs network)
packaging/bootc/build-image.sh

# Lane B — local RPM: bake a farm-built RPM (no channel dependency)
packaging/bootc/build-image.sh --rpm ~/mcnf-release-artifacts/magic-mesh-11.4.5-1.x86_64.rpm

# Either lane + a bootable disk image (root podman required)
sudo packaging/bootc/build-image.sh --rpm <rpm> --disk qcow2
```

`--base` overrides the Fedora bootc base (e.g. an F43 rebase);
`--tag` names the output (default `localhost/magic-mesh-bootc:latest`).

## Boot-to-seat & the systemd unit set

The image enables **`mde-shell-egui.service`** (new, this directory) +
**`podman.socket`**, sets `graphical.target`, and inherits the RPM
post-install's all-roles set: `mackesd.service`, `nebula.service` (+ the
`nebula.service.d/10-mesh-recovery.conf` drop-in), `mesh-health.timer`,
`mesh-status.timer`, `magic-setup.service` (first-run wizard),
`magic-mesh-brand.service`, the first-boot fetch oneshots
(`mesh-shell-setup` / `mesh-broker-setup` / `mesh-netdata-setup`) and the
`mde-musicd.service` user unit. `etcd.service` + `syncthing.service` ship
condition-gated (they only start where `setup-etcd.sh` /
`setup-syncthing.sh` wrote their config — that config lives in `/etc`, which
persists).

Boot flow on a fresh Workstation: `magic-setup.service` owns tty1 until a role
is pinned to `/var/lib/mde/role.toml`; `mde-shell-egui.service` (ordered
`After=magic-setup.service`, `ConditionPathExists=/var/lib/mde/role.toml`,
`ExecCondition` = role `"workstation"`) then takes the VT and the egui shell
takes DRM master directly (the RPM links `mde-shell-egui` with
`--features drm`). If the shell hits its start limit, `ExecStopPost` restores
`getty@tty1` so the console is never lost.

## Roles (the §5 config flag)

Nothing is masked by default — **every role boots this same image**:

- **Workstation** — `role = "workstation"` in `/var/lib/mde/role.toml` →
  seat unit passes its `ExecCondition`, shell on the DRM seat, VDI stack live.
- **Lighthouse / headless** — any other (or no) role → the seat unit skips
  cleanly at every boot. Hard-off (§5 "Option 1", belt-and-braces on a box
  that must never light a seat):

  ```sh
  systemctl mask mde-shell-egui.service magic-mesh-brand.service
  systemctl --global mask mde-musicd.service
  systemctl set-default multi-user.target
  ```

  Re-roling is the reverse (`unmask` + `set-default graphical.target` + pin
  the role) — no reinstall, per §5.

## Writable-partition doctrine

Full annotated table in the `Containerfile` header. The short form:

| Lane | Semantics across `bootc upgrade` | MCNF state there |
|---|---|---|
| `/usr`, `/boot` | swapped atomically | all binaries + units + assets |
| `/etc` | 3-way merge — local edits persist | `/etc/mackesd`, `/etc/nebula`, `/etc/etcd/etcd.env`, setup-written drop-ins, `/etc/hosts` mesh merges |
| `/var` | machine-local; image `/var` seeds **first boot only** | `/var/lib/mackesd`, `/var/lib/mde/role.toml` (the role flag), `/var/lib/etcd`, `/var/lib/mcnf-syncthing`, `~/Local` VM disks (`/home`→`var/home`), `/mnt/mesh-storage` (`/mnt`→`var/mnt`), `/opt/mcnf` (`/opt`→`var/opt`) |
| `/run/mde-bus` | tmpfs, tmpfiles re-creates per boot | the shared bus spool |

⚠ Known caveat: the RPM's `/opt/mcnf` automation/backoffice plane rides the
`/var` seed, so **image upgrades do not refresh it** — acceptable today (that
plane converges control-VM-side, DAR-46), flagged by `bootc container lint`
during the build (deliberately non-fatal).

## Update / rollback story

```sh
bootc status                      # what's deployed / staged / rolled back
bootc upgrade                     # pull + stage the new image; reboot to apply
bootc rollback                    # flip back to the previous deployment; reboot
bootc switch <registry>/<image>   # rebase to a different image/tag
```

Updates are atomic: the new image stages beside the running one and a reboot
swaps `/usr` wholesale; `/etc` merges; `/var` (VM disks, mesh state, role flag)
is untouched — a failed update is one `bootc rollback` away. Publishing the
image to a registry (so `bootc upgrade` has a source) is **operator-gated**
alongside the RPM channel publish (/release).

## Verification status (2026-07-01)

- **Verified**: `build-image.sh` shellcheck-clean; its refusal gates exercised
  (bad `--rpm` path, bad `--disk` type → itemized `REFUSING to run`, rc 2);
  unit-name set cross-checked against `packaging/systemd/` + mackesd's
  `generate-rpm` assets/scriptlets; every state path grep-verified against the
  units/sources it comes from.
- **Real-build attempt** (`podman build`, farm host mm@172.20.0.130, podman
  5.4.1, lane `repo`): **fails at the base-image pull** — the farm host cannot
  reach `quay.io`:

  ```text
  Error: creating build container: initializing source
  docker://quay.io/fedora/fedora-bootc:42: pinging container registry quay.io:
  Get "https://quay.io/v2/": dial tcp: lookup quay.io: no such host
  ```

  (Verbatim failure re-captured in the E12-13 report; the farm's dnf flows go
  through the LAN mirror — there is no container-registry egress.)
- **Live-gated**: a green end-to-end `podman build` + first boot of the image
  (needs a host with registry egress, or the base image side-loaded via
  `podman load`); the `--disk` bootc-image-builder lane (root podman + the
  same egress); registry publish (operator-gated, /release).
