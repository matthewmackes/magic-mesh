# XCP build farm — parallel build + GUI-render slots

The MCNF dev/build/test toolchain that keeps heavy compute on the idle XCP hosts
(operator directive 2026-06-20: "only AI work local; farm everything else to XCP")
and keeps **parallel build/test in flight** while the AI works locally. It also
gives the GUI work a **headless render/screenshot VM** so look/feel/effects
changes are verified, not guessed.

## Pieces

| Tool | Role |
|------|------|
| `install-helpers/xcp-slots.sh`   | provision / rekey / **bootstrap** / list / destroy build+render slots; writes the `.xcp-slots.conf` registry |
| `install-helpers/xcp-build.sh`   | drive a slot: `sync` `cargo` `gate` `gates` `crate` **`render`** `rpm` `pull` `shell` `result`; structured JSON results in `.xcp-build/results/` |
| `install-helpers/xcp-parallel.sh`| split the gate set across slots so `test`/`clippy` run concurrently on different VMs |
| `install-helpers/preview-capture.sh` | the headless render (sway `WLR_BACKENDS=headless` + grim, `WLR_RENDERER=pixman` software) `xcp-build.sh render` calls on the slot |

A **slot** = one VM = `{name, host, user, key, remote_dir}` in `.xcp-slots.conf`
(gitignored, machine-local). Each slot keeps its own `target/` warm cache, so
slots build concurrently without clobbering — ideally on different physical XCP
hosts (each host has only 4 cores) for true parallelism.

## Hosts (from the infra inventory)

- **Build host dom0** `172.20.0.9` (XEN-HOME-SERVICES) — 4 core / 24 GB / 207 GB SR.
  Has a staged Fedora-Cloud raw at `/tmp/fedora-build.raw`.
- **Test bed dom0** `172.20.145.193` (KVM-XCP1) — 4 core / ~23 GB / empty SR; make
  throwaway VMs at will.
- dom0 access is `root` over ssh (password). Export it once per shell — **never
  commit it**:
  ```sh
  export MCNF_XCP_PASS='<dom0-root-password>'
  ```

## One-time bring-up (operator runs the destructive steps)

The AI authored the tooling but does not run destructive infra ops; the operator
runs the `--yes` steps below. Non-destructive steps (`bootstrap`, `gates`,
`render`) the AI runs once a slot is keyed.

```sh
# 0. Inventory both dom0s (read-only) — confirm what's there before deleting.
export MCNF_XCP_PASS='<dom0-root-password>'
./install-helpers/xcp-slots.sh list 172.20.0.9
./install-helpers/xcp-slots.sh list 172.20.145.193

# 1. Destroy the locked mcnf-build VM (its dev key is lost) + any confirmed
#    orphans — frees the build host's 4 cores. (Delete scope: operator-approved
#    "mcnf-build + orphaned XCP VMs".)
./install-helpers/xcp-slots.sh destroy 172.20.0.9 mcnf-build --yes

# 2. Provision a fresh build+render slot 'a' on the build host, reusing the
#    staged raw, with the AI's key (~/.ssh/mackes_mesh_ed25519.pub). Registers
#    it in .xcp-slots.conf.
./install-helpers/xcp-slots.sh provision 172.20.0.9 mcnf-a 172.20.0.50/16 \
    --gw 172.20.0.1 --vcpus 4 --mem 16GiB --from-raw /tmp/fedora-build.raw \
    --register a --yes
```

Then the **AI** finishes (non-destructive):

```sh
# 3. Install the toolchain + GUI render stack on slot 'a' (rust 1.94, mold, deps,
#    sway+grim+mesa, fonts).  ~a few minutes.
./install-helpers/xcp-slots.sh bootstrap a

# 4. Validate the slot: full gates + a render of the Workbench home.
./install-helpers/xcp-build.sh gates --slot a          # structured result JSON
./install-helpers/xcp-build.sh render '' --slot a      # → .xcp-build/renders/a-*.png
```

### Add a second slot for true parallelism (optional)

Stage a Fedora raw on the test bed, then provision slot `b` there (different
physical host = real concurrency):

```sh
# copy the base raw from the build host to the test bed dom0 (operator)
ssh root@172.20.0.9 'cat /tmp/fedora-build.raw' | ssh root@172.20.145.193 'cat > /tmp/fedora-build.raw'
./install-helpers/xcp-slots.sh provision 172.20.145.193 mcnf-b 172.20.145.40/16 \
    --gw 172.20.0.1 --from-raw /tmp/fedora-build.raw --register b --yes
./install-helpers/xcp-slots.sh bootstrap b
```

## Daily use (the AI, autonomous once slots exist)

```sh
./install-helpers/xcp-build.sh slots                       # list + reachability
./install-helpers/xcp-build.sh gate test --slot a          # one gate, JSON result
./install-helpers/xcp-build.sh crate mde-cosmic-applet test --slot a   # per-crate
./install-helpers/xcp-build.sh render maintain.audit --slot a          # screenshot a panel
./install-helpers/xcp-parallel.sh gates a b                # test on a, clippy on b, concurrently
./install-helpers/xcp-build.sh result latest               # pretty-print the last result
```

Fire long gates in the background and keep working locally; the structured JSON
(`.xcp-build/results/`) is read back when the run finishes. Two `--slot` values
are always safe to run at the same time.

## GUI look/feel/effects workflow

For MOTION / APPS-FX / NOTIFY-FX / launcher / brand work (operator 2026-06-20:
"run all look/feel/effects work on the test VM"):

1. Edit the GUI crate locally.
2. `xcp-build.sh crate <crate> test --slot a` — unit/token tests (§4 Carbon lint
   via `gate carbon`).
3. `xcp-build.sh render <slug> --slot a` — pull a PNG and inspect the actual
   render (the §7 visual gate is lifted, but rendering catches real regressions).
4. Commit when green + the render looks right.

## Notes / frontier

- `render` currently builds + captures **mde-workbench** (the main look/feel
  surface) via `preview-capture.sh`. Capturing other standalone GUIs
  (mde-files/mde-music/notify-center) is a generic-capture follow-on; the
  cosmic-applet launcher *dropdown* needs a panel host, so its sizing
  (APPS-FIT-2) is verified by the `parse_menu_size_from_kdl` unit tests + tokens.
- `.xcp-slots.conf` + `.xcp-build/` are gitignored (per-host IPs/keys, results).
- The release path (`xcp-build.sh rpm`) and the original env fallback
  (`MCNF_BUILD_HOST=…`) still work without a registry.
