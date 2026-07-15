# RPM payload size — the 100 MB channel ceiling and the browser sub-package split

**Status:** implemented and release-cut verified (2026-07-15). The size GUARD
(below) is wired into both RPM cut paths; the asset-array split now emits a base
`magic-mesh` RPM plus a co-installable `magic-mesh-browser` RPM. BigBoy
(`172.20.0.130`) cut the full release path through `install-helpers/xcp-build.sh rpm`
and produced:

| package | compressed RPM | SHA-256 |
|---|---:|---|
| `magic-mesh-12.0.0-1.x86_64.rpm` | **69.9 MiB** / 73,340,947 bytes | `a831d1014db25b1005f7bc4caefa8b8b2998efdf1a1b6598ea50b0dc4f0846d6` |
| `magic-mesh-browser-12.0.0-1.x86_64.rpm` | **39.0 MiB** / 40,851,412 bytes | `96dd4637c3f04457be0dff5ea32020abee2117872c82d8b2d2945475dd519df7` |

Both artifacts passed the 90 MiB cut limit and payload inspection.

**Finding:** `build-deploy-12` (PLATFORM-REVIEW-2026-07-10) — *Monolithic ~186 MiB RPM is
one growth step from breaking the public channel (GitHub 100 MB file limit).*

---

## 1. The cliff

The earlier one-artifact doctrine (PKG-1) packaged every workspace binary + all
packaging assets into a single `magic-mesh` RPM (`crates/mesh/mackesd/Cargo.toml`
`[package.metadata.generate-rpm]`). The public dnf channel is served from GitHub Pages:

```
# packaging/repo/magic-mesh.repo:17
baseurl=https://matthewmackes.github.io/magic-mesh/fedora-$releasever-$basearch/
```

GitHub hard-blocks any file larger than **100 MiB** in a pushed branch (documented by GitHub
as "100 MB"; the strictest decimal reading is 100 MB = 95.37 MiB). gh-pages *is* a git branch,
so the served `.rpm` file is subject to that block. The Cargo.toml comment records the history:
default **zstd** already produced ~103 MiB (over the limit), and the fix was switching
`payload_compress = "xz"` to squeeze "the same payload under 100 MiB". There is **no headroom
left** and no size monitoring: the next binary/asset that tips the compressed file back over
the limit makes the fleet's primary upgrade channel silently un-publishable, and it would be
discovered at publish time of an otherwise-finished release.

The RPM file that is size-limited is the **compressed** artifact (what gets pushed), not the
uncompressed payload.

---

## 2. Payload composition (evidence-based)

The current asset array has **never actually been cut** — the browser helper binaries
(`mde-web-preview`, `mde-web-cef`, `mde-web-cef-renderer`) were added to the `assets` array
*after* the last real cut. Real measurements from the last cut RPM that exists in a build
scratchpad (`magic-mesh-12.0.0-1.fc43`, **pre-browser**, zstd):

| metric | value |
|---|---|
| compressed RPM file (zstd) | **55.2 MiB** |
| uncompressed installed payload | **93.96 MiB** |
| compressor | zstd (pre-xz-switch snapshot) |

Top files in that pre-browser payload (uncompressed):

| size (uncompressed) | file | class |
|---:|---|---|
| 30.10 MiB | `/usr/share/magic-mesh/vendor/ntfy_2.24.0_linux_amd64.tar.gz` | vendored birthright blob |
| 14.14 MiB | `/usr/bin/mackesd` | Rust daemon |
| 13.03 MiB | `/usr/bin/mde-shell-egui` | Rust GUI shell |
|  7.08 MiB | `/usr/bin/mde-role-chooser` | Rust GUI |
|  6.38 MiB | `/usr/bin/mde-bus` | Rust CLI |
|  6.13 MiB | `/usr/bin/mde-musicd` | Rust daemon |
|  4.81 MiB | `/usr/share/magic-mesh/vendor/starship-x86_64-...tar.gz` | vendored birthright blob |
|  2.85 MiB | `/usr/bin/mde-enroll` | Rust TUI |
|  1.01 MiB | `/usr/bin/magic-fleet` | Rust CLI |
|  0.68 MiB | `/usr/share/backgrounds/mcnf-11-winter.png` | wallpaper |
|  0.65 MiB | `/usr/bin/meshctl` | Rust CLI |
|  0.58 MiB | `/usr/bin/magic-setup` | Rust CLI |
| ~2 MiB | brand/wallpaper/icons/help/automation/units | assets |

### What pushed it to ~186 MiB raw

The `Cargo.toml` comment states the *current* array is "~186 MiB raw". The delta from the
measured 94 MiB pre-browser payload to ~186 MiB is ~**92 MiB**, and it is entirely the three
browser helper binaries now in the array:

- **`mde-web-preview` (Servo)** — the dominant single contributor. libservo statically linked
  with `baked-in-resources` + `js_jit` is enormous even at `opt-level = "z"` + `strip = true`
  (Servo binaries are routinely 70–90 MiB stripped). This is its own workspace root
  (`crates/desktop/mde-web-preview`), built standalone before `generate-rpm`.
- **`mde-web-cef` + `mde-web-cef-renderer`** — the Chromium/CEF *bridge* helpers. Note the
  311 MB CEF *runtime* is NOT embedded (it is fetched + verified at first boot by
  `mde-cef-runtime-setup.service`); only the thin launcher + renderer bridge ship in the RPM.

So the review's characterization is confirmed by measurement: **the browser helpers are ~half
the raw payload, and they are exactly the growth step that forced zstd→xz and left zero
headroom.** They are already role-gated (not in the `server` variant — a headless Server has no
browser surface).

### Composition summary (raw / uncompressed)

| bucket | ~raw size | notes |
|---|---:|---|
| **Browser helpers** (Servo + CEF bridge) | **~92 MiB** | NEW; the whole reason the RPM is at the cliff. Role-gated already. |
| Vendored birthright blobs (ntfy + starship) | ~35 MiB | Already-gzip tarballs → **near-incompressible floor**; xz gains almost nothing here. Air-gap first-boot birthrights. |
| Rust daemon/shell/CLI bins (9 bins) | ~52 MiB | Already `opt-level="z"` + `lto=true` + `strip=true`. |
| Brand / wallpaper / icons / docs / automation / units | ~7 MiB | Small individually; `build-deploy-11` flags some as retired-Cosmic dead weight. |

---

## 3. Quick-win levers — already at their limit (no change warranted)

Two of the obvious size levers are **already maximal**; confirmed, so do NOT "fix" them:

- **Compression codec.** `payload_compress = "xz"` is already the smallest codec
  cargo-generate-rpm offers (`none` / `gzip` / `zstd` / `xz`; no level knob for xz). There is
  no higher setting to switch to.
- **Debug symbols.** `Cargo.toml` `[profile.release]` already sets `strip = true`
  (plus `opt-level = "z"`, `lto = true`, `codegen-units = 1`). The shipped binaries carry no
  debuginfo; there is nothing to strip.

Genuinely available small wins (each modest, and out of scope for a blind edit here):

- **Retired-Cosmic dead assets** (`build-deploy-11`, ~1 MiB): `cosmic-layout/**/*`,
  `mackes-carbon-icons.tar.xz`, `mde-enforce-layout` + its autostart, the Cosmic wallpaper
  seeder. Sweep tracked under `build-deploy-11`, not here.
- **The 30 MiB ntfy birthright blob** is the largest *single* file and is near-incompressible.
  It could be demoted to a first-boot GitHub-Releases / sovereign-channel fetch (like the CEF
  runtime and netdata already are) rather than embedded, reclaiming ~30 MiB of both compressed
  and uncompressed size. That is a birthright-policy decision (it exists for offline installs),
  tracked separately.

Neither of these is the structural fix; the browser split below is.

---

## 4. The structural fix — a `magic-mesh-browser` sub-package

### 4.1 Why a plain `variants` entry is not enough (the key subtlety)

cargo-generate-rpm `variants` (e.g. the existing `server` variant) produce **mutually-exclusive
alternative packages**: `magic-mesh-server` declares `conflicts = { magic-mesh = "*" }` — a node
carries EITHER the full package OR the server package, never both. A browser split needs the
opposite relationship: the browser payload must be a **separate, additively co-installable**
package that sits *alongside* the base `magic-mesh`.

cargo-generate-rpm can still express this, because a "variant" is just "a package built from
this asset set + these dependency tables". The plan therefore uses a THIRD variant that:

- carries **only** the browser assets,
- does **not** conflict with the base, and
- is pulled in by a **weak dependency** (`recommends`) from the base so a default Workstation
  install still gets the browser, but as a *separately-sized file*.

### 4.2 The split

**Implemented `[package.metadata.generate-rpm.variants.browser]`** — `magic-mesh-browser`, carrying
ONLY the browser payload MOVED OUT of the base array:

| moved asset | dest |
|---|---|
| `target/release/mde-web-preview` | `/usr/bin/mde-web-preview` |
| `target/release/mde-web-cef` | `/usr/bin/mde-web-cef` |
| `target/release/mde-web-cef-renderer` | `/usr/libexec/mackesd/mde-web-cef-renderer` |
| `install-helpers/install-cef-runtime.sh` | `/usr/libexec/mackesd/install-cef-runtime` |
| `packaging/browser/cef-linux64-minimal.env` | `/usr/share/magic-mesh/browser/…` |
| `packaging/systemd/mde-cef-runtime-setup.service` | `/usr/lib/systemd/system/…` |
| the BROWSER-DD webextension / widevine / tts / stt / translate helpers + `.env` + setup units | `/usr/libexec/mackesd/…`, `/usr/share/magic-mesh/browser/…`, `/usr/lib/systemd/system/…` |
| `packaging/selinux/mde-web-preview.{te,fc}` + its loader + unit | `/usr/share/magic-mesh/selinux/mde-web-preview/…` |
| `packaging/selinux/mde-web-cef.{te,fc}` + its loader + unit | `/usr/share/magic-mesh/selinux/mde-web-cef/…` |
| `crates/services/mde-adblock/seed/*.txt` | `/usr/share/magic-mesh/adblock/` |

Everything else (daemon, shell, CLIs, substrate, units, brand, docs, automation) **stays in the
base array**.

**Dependency wiring:**

- base `magic-mesh` → `recommends = { magic-mesh-browser = "*" }` so `dnf install magic-mesh`
  still pulls the browser by default on a Workstation, but it is a separately-sized, removable
  file. (Weak dep, not `requires`, so a bandwidth/disk-constrained node can `dnf remove
  magic-mesh-browser` and keep a working desktop with the Browser surface honestly gated off.)
- `server` variant → **no** browser recommend (headless roles already omit it).
- `magic-mesh-browser` → `requires = { magic-mesh = "*" }` (the helpers are launched by the
  shell; they are meaningless without the base), keeps the CEF/bzip2/runtime graphics
  `requires`, and hard-requires `selinux-policy-devel` + `checkpolicy` so Enforcing
  Workstation seats compile and load `mde_web_cef_t` / `mde_web_preview_t` instead of
  silently leaving Browser helpers at `bin_t`.
- **No `conflicts`** between base and browser — they co-install.
- Note: RPM automatic ELF soname scanning still records shell-linked base requirements such as
  `libfontconfig.so.1`, `libfreetype.so.6`, `libharfbuzz.so.0`, `libvulkan.so.1`, and
  `libxkbcommon.so.0`. The split removes the Browser helper payload and Browser-only manual
  dependency policy from the base package; it does not pretend the DRM shell has zero graphics
  library dependencies.

### 4.3 Measured sizes after the split

| package | contents | compressed (xz) | vs 100 MiB ceiling |
|---|---|---:|---|
| `magic-mesh` (base) | everything except browser | **69.9 MiB** | comfortable headroom |
| `magic-mesh-browser` | Servo + CEF bridge + browser assets | **39.0 MiB** | comfortable headroom |
| `magic-mesh-server` | headless (unchanged) | ~25–30 MiB | unchanged |

Both public-channel files drop **well under** the limit, and each can grow independently before
either approaches the cliff. zstd could even be reconsidered per-file for faster installs on
low-end lighthouses, since neither file is near the ceiling anymore.

Fedora 44 deploy cut proof (2026-07-15, `install-helpers/build-rpm-fedora43.sh 44`):

| package | compressed (xz) | bytes | SHA-256 |
|---|---:|---:|---|
| `magic-mesh-12.0.0-1.x86_64.rpm` | **70.0 MiB** | 73,349,769 | `b2e26d1aa557a74631d6a5a27da33904990c2da3a7eab5776b3aeff5d1b3ac95` |
| `magic-mesh-browser-12.0.0-1.x86_64.rpm` | **39.1 MiB** | 41,012,002 | `d4e828adcb3f1b494bf9d664d86b4876b13f44fdf5112a1386d5de6b6816a44f` |

Both F44 artifacts passed payload verification, `rpm -Uvh --test` on the Fedora
44 `.15` Workstation, and installed together as co-installable split packages.
Follow-up live Enforcing proof on `.15` found the Browser setup units must be
started, not merely enabled, on already-booted installs; the Browser `%post`
now queues `systemctl start --no-block $BROWSER_UNITS`. The SELinux policy
loaders also stage hyphenated source files under underscore module names before
calling the SELinux devel Makefile, matching `policy_module(mde_web_cef, ...)`
and `policy_module(mde_web_preview, ...)`.

### 4.4 Cut-script change (the part that makes it real)

The RPM cut becomes a **three-invocation** sequence (both cut paths —
`install-helpers/build-rpm-fedora43.sh` and `install-helpers/xcp-build.sh rpm`):

```
cargo build --workspace --release                       # daemon/shell/CLI bins
cargo build --release --manifest-path .../mde-web-preview/Cargo.toml   # Servo (excluded workspace)
cargo build --release --manifest-path .../mde-web-cef/Cargo.toml       # CEF bridge (excluded workspace)
cargo build --release -p mde-shell-egui --features <drm,...>           # shell re-link
cargo generate-rpm -p crates/mesh/mackesd                 # base  → magic-mesh-*.rpm
cargo generate-rpm -p crates/mesh/mackesd --variant browser  # → magic-mesh-browser-*.rpm
cargo generate-rpm -p crates/mesh/mackesd --variant server   # headless (existing)
```

Both `magic-mesh-*.rpm` and `magic-mesh-browser-*.rpm` are published to the same gh-pages
`createrepo_c` tree; dnf resolves the `recommends` automatically. NEVRA discipline
(`build-deploy-10`) applies to both files.

---

## 5. Alternatives (interim / complementary, from the review)

If the split is deferred, either of these removes the cliff **without** touching the asset
array, and can ship first:

- **A. Host release RPMs as GitHub Releases assets (2 GiB limit)**, referenced from small
  `repodata` on gh-pages. The channel metadata stays tiny; the large payload lives where there
  is no 100 MiB block. Lowest-code interim.
- **B. Promote the sovereign / Forgejo channel (DAR-23) to primary.** It already mirrors the
  gh-pages layout (cloud-init works unchanged) and has no per-file limit. The review notes it is
  currently the "GitHub-unreachable fallback"; promoting it inverts that.

These are *risk-reducing stopgaps*; the browser split is the durable structural fix and also
speeds installs (smaller base, per-file codec freedom).

---

## 6. The size GUARD (implemented)

Regardless of which structural path is chosen, a **cut must never silently produce a file over
the channel ceiling.** A size check is now part of the existing static packaging gate,
`install-helpers/verify-rpm-payload.sh`:

```
install-helpers/verify-rpm-payload.sh size <rpm>      # fail if <rpm> exceeds the ceiling
install-helpers/verify-rpm-payload.sh payload <rpm>   # payload-completeness check ALSO runs the size check
```

- **Threshold:** default **90 MiB** (env override `MCNF_RPM_SIZE_LIMIT_MIB`). 90 MiB leaves
  headroom under even the strictest reading of GitHub's limit (100 MB decimal = 95.37 MiB) and
  ~10 MiB under the 100 MiB git block. The review suggested 95 MiB; 90 MiB is the more
  conservative "danger-zone" default — a cut over 90 MiB means you are within one growth step of
  the cliff.
- **Measures the COMPRESSED `.rpm` file** (`wc -c`), i.e. the actual bytes pushed to gh-pages —
  not the uncompressed payload.
- **Exit non-zero** on breach, so it is drop-in for a release-cut gate.

### CI / cut-gate wiring

The size check is now wired into the cut paths right after `generate-rpm`
produces the files. `install-helpers/build-rpm-fedora43.sh` checks the server
artifact in `--server` mode and both base/browser artifacts in full mode.
`install-helpers/xcp-build.sh rpm` checks the remote generated artifacts before
pulling and checks the pulled local artifacts again.

---

## 7. What is left as an operator decision

- **Publishing both artifacts in one repo transaction** so `dnf` can resolve the
  weak dependency from `magic-mesh` to `magic-mesh-browser`.
- **Choosing the channel strategy** — split (§4) vs GitHub-Releases-assets (§5A) vs
  sovereign-primary (§5B), or a combination.
- **The ntfy-blob demotion** and the **`build-deploy-11` dead-asset sweep** — modest independent
  wins tracked under their own findings.
