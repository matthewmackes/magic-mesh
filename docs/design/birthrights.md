# Birthrights — install + first-boot provisioning policy

Audit: docs/COMPLIANCE.md "Birthrights vs advertised service requirements"
(2026-06-16). Worklist: the **BIRTHRIGHT** epic in docs/WORKLIST.md.

A *birthright* is anything a fresh `dnf install magic-mesh` + `network-enroll
found`/`join` must provision automatically for the advertised platform to be
fit-for-purpose out of the box. The audit found two classes of dependency:

1. **In-repo deps** — present in the Fedora base repos. Declared as hard RPM
   `Requires:` (`nebula`, `nmap`, `alsa-lib`) or weak `Recommends:`
   (`ansible-core`, `kamailio`, `rtpengine`, …). These resolve at `dnf install`
   time. Correct as-is.
2. **Out-of-repo deps** — NOT in the Fedora repos, so they *cannot* be an RPM
   `Requires:`. These were silently un-provisioned (the failure mode the audit
   caught). Each needs an explicit provisioning policy, below.

## BIRTHRIGHT-2 — bundle-vs-fetch decision per out-of-repo dependency

The provisioning model is a first-boot helper (`mesh-install-<dep>` +
`*-setup.service`, all roles) that installs a **pinned, sha256-verified**
upstream binary. Pure-fetch breaks an **air-gapped** first boot (no internet →
no provision). Per dependency:

| Dependency | Role of dep | Decision | Offline behavior |
|---|---|---|---|
| **ntfy** | cross-node notification broker (NOTIFY-DIST-1) | **Bundle** | RPM ships the pinned tarball under `/usr/share/magic-mesh/vendor/`; first boot provisions with **no network**. |
| **starship** | mesh bash prompt (SHELL-2) | **Bundle** | Same — bundled-first, offline-capable. |
| **lizardfs / lizardfs-adm** | §1 shared-state substrate | **Bundle (BIRTHRIGHT-1, done)** | The fc43 LizardFS family is bundled in the RPM (`/usr/share/magic-mesh/vendor/lizardfs/`) and installed `--nodeps` on F44 by `mesh-install-lizardfs`; `mackesd found`/`join` auto-provisions it role-aware. |

**LizardFS (BIRTHRIGHT-1):** the fc43 LizardFS family (`lizardfs-master`,
`-chunkserver`, `-client`, `-adm`, `-cgi*`, `-metalogger`; 7 RPMs / ~2.5 MB) is
bundled the same way. `vendor-lizardfs-rpms.sh` runs `dnf download 'lizardfs*'`
(family only — NOT the base-OS closure, since F44 already has glibc/systemd/
fuse) inside a fedora:43 container into `vendor/birthright/lizardfs/`; the RPM
ships them to `/usr/share/magic-mesh/vendor/lizardfs/`. They install on F44 via
`rpm --nodeps` (the fc43 binaries run on F44, the only path that works since
LizardFS is absent from the F44 base repos).

### Mechanics (bundle path)

- **Bundled-first, fetch-fallback.** `mesh-install-ntfy` / `mesh-install-starship`
  prefer the bundled tarball at `/usr/share/magic-mesh/vendor/$ASSET` (checksum
  verified); only when it is absent/invalid do they reach the network. So an
  online install still self-heals if the bundle is ever dropped, and an offline
  install Just Works.
- **Pins are single-sourced** in the install scripts (`VER` / `SHA256` / `ASSET`
  / `URL`). `install-helpers/vendor-birthright-blobs.sh` parses those pins and
  fetches + verifies the blobs into `vendor/birthright/` at **build** time — the
  blobs are NOT committed to git (third-party binaries), they are
  `.gitignore`d and produced by the build. `build-rpm-fedora43.sh` runs the
  vendor step before `cargo generate-rpm`; the `[package.metadata.generate-rpm]`
  `assets` array ships them into the RPM. The bundle therefore can never drift
  from what the installer verifies.
- **License coverage:** NOTICE records ntfy (Apache-2.0) + starship (ISC) as
  bundled, aggregated (not linked) third-party binaries, GPL-3.0-compatible.

### Air-gapped build caveat

The *build* machine still needs network the first time it stages the blobs
(the vendor step fetches them). The *install/first-boot* machine does not. A
fully air-gapped build would pre-seed `vendor/birthright/` out-of-band; the
vendor step is idempotent and leaves a present, checksum-valid blob untouched.

## BIRTHRIGHT-1 — auto-provision LizardFS/QNM-Shared at enrollment (done)

A fresh `dnf install magic-mesh` + `mackesd found`/`join` now stands up the
shared-state plane automatically:

- **`mackesd found`** (founding lighthouse) → `provision_qnm_shared(role,
  is_founder=true, …)` installs LizardFS and runs `setup-qnm-shared --master
  --chunkserver --client` (master bound on the founder's overlay IP), mounting
  `/mnt/mesh-storage`, BEFORE `mackesd.service` starts.
- **`mackesd join`** → role-aware: Workstation = `--client`; Server / a second
  Lighthouse = `--chunkserver --client`; master via the floating VIP
  (`10.42.0.1`). Never a second master.
- **Binary install** (`mesh-install-lizardfs <role>`): dnf-first (F43), then the
  bundled fc43 RPMs (F44 / offline), then a pinned fetch manifest, then a loud
  non-fatal warning. Best-effort — a miss never aborts the overlay join.
- **Fail-loud runtime check:** `run_serve` asserts at startup that
  `/mnt/mesh-storage` is a real FUSE mount; if not, it logs an ERROR (the
  shared-state plane is down) instead of degrading silently — the ONBOARD-6
  failure class. The `mesh-health` watchdog then restarts `qnm-shared.service`.
- Flag policy is unit-tested (`qnm_setup_flags`); `tests/mesh_shared_state.rs`
  (the multi-node gate) and `install-helpers/lint-shared-substrate.sh` remain.

Air-gapped build still needs network once to stage the blobs (`build-rpm-
fedora43.sh` runs both vendor scripts); the install target is fully offline.
