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
| **lizardfs / lizardfs-adm** | §1 shared-state substrate | **Fetch-only (deferred)** | Held under **BIRTHRIGHT-1** (the substrate-provisioning epic). Until then, an air-gapped install has NO shared state — stated loudly, not silent. |

**Why bundle ntfy + starship but not LizardFS:** ntfy + starship are small
(~31 MB + ~5 MB), permissively licensed (Apache-2.0 / ISC — see NOTICE), single
static binaries, and already wired this session. LizardFS is a larger,
role-aware, multi-package substrate whose provisioning (master vs chunkserver vs
client, the mount, goal/quota) is a design problem in its own right — that is
BIRTHRIGHT-1, held for operator go-ahead.

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

## Out of scope (BIRTHRIGHT-1, held)

Auto-provisioning LizardFS/QNM-Shared at enrollment (install the binaries +
auto-run `setup-qnm-shared` role-aware during `found`/`join`) so a fresh install
is a working shared-state mesh. Large substrate change — operator-gated.
