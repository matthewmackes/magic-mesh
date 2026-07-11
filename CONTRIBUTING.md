# Contributing to MCNF

The operational rulebook is [`AI_GOVERNANCE.md`](AI_GOVERNANCE.md) — read its
locks (§1–§8) before changing anything load-bearing. The short version: the
substrate (Nebula / etcd / Syncthing / Bus / max-crypto) is **locked**, and the UI
is **egui-native** — one shell that owns the DRM/KMS seat directly (no Wayland
compositor); the single source of look is the shared `Style` in `mde-egui` plus
`mde-theme::brand` (QBRAND), and strict IBM Carbon as a token/lint gate is retired
(§4). New code is glue over the existing crates, and a feature isn't done until
it's runtime-reachable with no stubs (§7).

## Build prerequisites

> **Canonical, full build environment + the build farm + every gotcha:**
> [`docs/BUILD-ENVIRONMENT.md`](docs/BUILD-ENVIRONMENT.md). This is the short version.

Fedora (the target platform + the build-farm VMs):

```bash
sudo dnf install -y gcc gcc-c++ cmake mold pkg-config \
  gtk3-devel alsa-lib-devel libxkbcommon-devel opus-devel protobuf-compiler
```

**EL9 / Rocky 9 (the actual dev host `172.20.145.192`) — two things bite:**
1. `opus-devel` is in **CRB**, not the default repos:
   `sudo dnf --enablerepo=crb install -y opus-devel`.
2. Heavy local `cargo` (`build`/`test`/`clippy`) is **guard-disabled** on this
   dev host (`cargo-farm-guard.sh`) — build on the farm:
   `install-helpers/xcp-build.sh cargo build --workspace`. (A fresh, unguarded
   EL9 box does build locally, but its gcc 11.5 rejects `mold` — `.cargo/config.toml`
   selects it — so it needs `RUSTFLAGS="-C link-arg=-fuse-ld=gold"`. The farm VMs
   run gcc 15, so mold works there with no override.)

- Rust: MSRV **1.85** (the floor, CI-enforced); `rust-toolchain.toml` pins
  **1.94** as the dev ceiling (softbuffer 0.4.8 breaks on 1.95 — see the
  file header).
- The vendored Opus tree needs `CMAKE_POLICY_VERSION_MINIMUM=3.5`;
  `.cargo/config.toml` sets it, CI sets it explicitly.
- The workspace has ~40 crates (see `Cargo.toml` `members`); the two
  browser-engine crates (`mde-web-cef`, `mde-web-preview`) are **separate excluded
  workspaces** with their own `Cargo.lock`, built on their own (the RPM cut builds
  `mde-web-preview` separately before `generate-rpm`).
- **Offload heavy/parallel builds to the farm:** `install-helpers/xcp-build.sh
  cargo …` (routes to the farm); status via `install-helpers/farm.sh status`.

## Test rules

Run these on the farm (`install-helpers/xcp-build.sh cargo test …`, or
`xcp-build.sh gates` for the full pyramid) — local `cargo test` is guard-disabled.
The parallelism rules the farm applies:

```bash
cargo test --workspace --exclude mackesd          # parallel — everything else
cargo test -p mackesd --features async-services -- --test-threads=1
```

**mackesd MUST run serially.** Several of its tests mutate process-global env
(`std::env::set_var`) while siblings read it; in parallel the suite corrupts
`environ` and hangs (tracked: EFF-18). `--features async-services` is the
superset (compiles + runs the daemon-worker suites).

Carbon token / palette / metric changes additionally require
`cargo test -p mde-theme` — a token value changes only with a matching test
assertion (§4).

## Gates (all must pass before a commit lands)

The cargo gates run on the **farm** — `install-helpers/xcp-build.sh gates` bundles
build + clippy + test (local `cargo` is guard-disabled by `cargo-farm-guard.sh`);
the `lint-*.sh` / `cargo deny` steps run locally. The always-on
`install-helpers/ci-gate.sh` runs the whole set on every `origin/master` advance.

```bash
cargo build --workspace --locked
cargo clippy --workspace --all-targets     # crypto/unwrap lints are deny-level
cargo fmt --all -- --check
./install-helpers/lint-mesh-boundary.sh    # §6 — no mesh→desktop-shell dep
./install-helpers/lint-carbon-tokens.sh    # §4 — no raw colour outside mde-theme
./install-helpers/lint-motion.sh           # §4 — no bespoke animation duration outside mde-theme
./install-helpers/lint-bus-names.sh        # §2 — no private D-Bus names
cargo deny check                           # EFF-16 — advisories/licenses/bans/sources
```

CI (`.github/workflows/ci.yml`) runs all of the above on pinned 1.85, plus a
nightly `--include-ignored` job, a Fedora-native container job, a coverage
floor, and a CycloneDX SBOM artifact.

## Conventions that will bite you if skipped

- **No raw hex / scattered metric literals** outside
  `crates/shared/mde-theme` (§4). Use tokens; the lint gate catches colours,
  review catches metrics.
- **No new MDE-private D-Bus names** (§2). MDE-internal IPC rides `mde-bus`
  (`action/<prefix>/<verb>` → `reply/<ulid>`); only FDO `org.freedesktop.*`
  interop touches D-Bus.
- **Bus responders cap bodies** at `ipc::MAX_RPC_BODY_BYTES` before parsing —
  follow the existing `poll_once` patterns when adding a surface.
- **Worker shell-outs go through `workers::proc`** (kill-on-timeout); a bare
  `Command::output()` on a tick path pins a runtime thread when the child
  hangs (EFF-20).
- **UI is egui-native** — surfaces render through `mde-egui` (eframe/wgpu; the
  shell owns the DRM/KMS seat directly, no Wayland compositor) using the shared
  `Style` + `mde-theme::brand`. The two excluded browser engines (`mde-web-cef`
  CEF/Chromium, `mde-web-preview` Servo) are pinned in their own workspaces and
  bumped on their own cadence (Servo tracked monthly).
- **Crypto floor** (§3): Ed25519 / AES-256-GCM / ChaCha20-Poly1305 / RSA-4096
  own keys / rustls. `cargo deny` bans openssl outright.

## Commits

Small, scoped, individually runtime-reachable + stub-free (§7). Why-not-what
messages. Pushing and releasing are operator-gated — don't push from
automation; RPM cuts stay operator-gated.

## Where things live

| Area | Path |
|---|---|
| **Build & dev environment** (canonical) | [`docs/BUILD-ENVIRONMENT.md`](docs/BUILD-ENVIRONMENT.md) |
| Build farm + IaC | [`docs/farm.md`](docs/farm.md) · [`infra/`](infra/) |
| Architecture map | [`docs/architecture.md`](docs/architecture.md) |
| Operator day-2 guide | [`ADMIN.md`](ADMIN.md) |
| Operator runbooks | [`docs/help/`](docs/help/) |
| Worklist (single tracker) | [`docs/WORKLIST.md`](docs/WORKLIST.md) |
| Audit reports | [`docs/COMPLIANCE.md`](docs/COMPLIANCE.md) |
| Design archive (historical) | [`docs/design/`](docs/design/) |
