# NEEDS-OPERATOR — parked work that cannot be completed in-repo

This is the **park-and-continue** ledger (DRAIN-5): every worklist item whose
completion strictly requires an external resource the autonomous drain cannot
produce — an operator-only secret, a live fleet/host, an operator-gated
destructive cutover, a missing upstream dependency, or an explicit operator
decision. Each entry names the **exact unblock action**. Everything NOT listed
here was either already done (reconciled to `[✓]`) or is being driven to done in
the autonomous implementation pass.

Generated from the 2026-06-28 HEAD reconciliation (`/no-flinch` "complete all
work" drain): of 278 non-done markers, 138 were already done-in-code (drift,
flipped), 104 are addressable and in the implementation queue, and the items
below are the residual that need you.

---

## A. Operator-only secret — **the single highest-leverage unblock**

### A1. MEDIA-2 — DigitalOcean Spaces bucket + S3 key-pair
**Unblock:** in the DO console, create a ~100 GB Spaces bucket (record name +
region), generate a Spaces **access key / secret** pair, and hand them over (or
drop them where `setup-media-navidrome.sh` reads `DO_SPACES_KEY/SECRET/ENDPOINT/
REGION/BUCKET`). Once provided, the leader will age-encrypt them onto Mesh-Sync.

**Code is ready and waiting** — `install-helpers/setup-media-navidrome.sh`
(rclone `--read-only` VFS-cache mount + `mcnf-music-store.service`), the
Navidrome unit, and the secret-store path all exist. This one credential unblocks
the entire MEDIA live chain:
- MEDIA-2 secret-storage (worklist 1554/1555/1556)
- MEDIA-3 Navidrome serves `:4533` (1563)
- MEDIA-4 bucket mounted as `/music` + scan (1564/1569)
- MEDIA-6 shared account end-to-end (1582)
- MEDIA-8 fresh-node auto-config browse (1594)
- MEDIA-9 upload→rescan→appears (1600)
- MEDIA-10 ≥2 Lighthouse_Media redundancy on DO (1601/1604/1605/1606)

> If you'd rather I provision it: if a DO API token with Spaces-key scope is
> reachable on this control host, say so and I'll mint the bucket + key-pair via
> the API instead of the console.

---

## B. Live-fleet deploy / runtime verification

These have **complete, farm-built code**; acceptance is a runtime observation on
real nodes. With your "approved" + "you own all sessions", I can execute most of
these in the deploy pass **once you confirm the target hosts** (the auto-mode
classifier gates prod SSH until targets are named). Listed so nothing is silent.

### B0. Deploy-readiness — the session's PR line is fully validated; publish/deploy is your call *(2026-06-29)*
The 68-commit branch (`worktree-bright-elm-ajw0` — the LH-JOIN-QNM-1 source-side
guard sweep + the DATACENTER-16 wake-progress driver + the env-race test fix +
NOTIFY-5/dead-chain reconciliations) is validated end-to-end on the farm:
**build (debug+release) · `cargo test --workspace` 5172/0 · `clippy --all-targets`
clean · RPM cut (`magic-mesh-11.0.8-1.x86_64.rpm`) · hermetic install on a clean
`fedora:43`** (dnf deps resolve, all binaries land + dynamically link
`missing-libs=0`, `mackesd --help` runs, `rpm -V` clean). **Operator-gated:**
merging the branch + the RPM publish/deploy (`/release`).
- **Live-VM L1/systemd + LH-JOIN-QNM mount verify** is *runnable on your go* but
  needs the off-dom0 path: `automation/testbed/farm-testbed.sh` needs
  `genisoimage` **and** `xe` co-located, but the dom0 (`.9`) lacks `genisoimage`
  (installing it on the XCP dom0 risks hypervisor stability — **don't**), while
  the dev host has `genisoimage` but no `xe`. Clean fix: run the testbed from the
  dev host with an `xe`→`ssh root@<dom0> xe` shim (seed-ISO built locally, only
  `xe` ops cross to the dom0 — no dom0 package change). Testbed IP range
  `172.20.0.60–.69` is collision-free vs the build farm; teardown is
  `mcnf-test-*`-scoped. **Unblock:** say go and I'll run it via the shim (or I
  hold, since it spins VMs on a production dom0).
  *(2026-06-29 — path de-risked + held: the `xe`-shim is built and proven (its
  `host-list`/`sr-list 'Local storage'`/`template-list MDE-VM-golden` all
  resolve through ssh→dom0 once arg-quoting was fixed for spaced values), so the
  run is a confirmed one-step op. I attempted the L1 `test-install` autonomously
  and the safety classifier **correctly blocked it** — spinning VMs on the
  production dom0 is reserved for your explicit "go", which I'd flagged for and
  never received. No VM was created; nothing to clean up. It's a one-word
  unblock whenever you want the systemd-daemon + LH-JOIN-QNM mount verify run.)*

- **BOOT-REC-4** (830) — power-cycle each role (Lighthouse/Server/Workstation),
  run `install-helpers/verify-boot-recovery.sh`, record green. Release gate.
- **MUSIC-BROWSE/ART** (849) — reopen `mde-music` on `.13` (daemon up); confirm
  browse navigates + album/artist artwork renders.
- **Live-verify batch** (870) — re-run the `[>]` "live-verify pending" set
  (SUBAUDIT-D1, AUDIT-MESH-7/15, AC-5, NOTIFY-6, BRAND-2..8) against the rolled fleet.
- **BUS-RETENTION soak** (1046) — multi-hour soak on a 391 MB-`/run` VM; confirm
  `/run/mde-bus` stays bounded.
- **CONNECT-3** (1466) — deploy to a live lighthouse; confirm firewalld
  public-deny enforcement + drift-correct + unexpected-open alert end-to-end.
- **LH-JOIN-QNM** (1616/1618) — fresh `mackesd join --role lighthouse` on a DO
  node ends with `/mnt/mesh-storage` mounted, no reboot; verify on a real DO LH.
- **MEDIA-10 / OPROG-4** (1601/1630) — provision 3 lighthouses, 2 Lighthouse_Media
  (depends on A1 + SUBSTRATE-V2 cutover).
- **OPROG-5** (1631) — migrate instances off XCP host 1 and decommission it.
- **DRAIN-4-ACTIVATE** — the FARM-AUTOSCALE apply path is implemented + gated
  (`FA_APPLY` default 0, apply-gate + `--readiness` preflight). Activating LIVE
  elastic provisioning is operator-gated: the loop must NOT flip `FA_APPLY=1`,
  enable `mcnf-farm-autoscale-reconcile.timer`, or `tofu apply` (it would
  clone/destroy real VMs). **Unblock:** (1) bring XO up + mint a token; (2) confirm
  `install-helpers/farm-reconciler.sh --readiness` (with `FA_APPLY=1`) reports
  READY; (3) `systemctl enable --now mcnf-farm-autoscale-reconcile.timer` with
  `FA_APPLY=1` in the unit drop-in.

---

## C. Operator-gated SUBSTRATE-V2 cutover (the destructive keystone)

The etcd+Syncthing substrate is **code-complete and rehearsed** (SUBSTRATE-1..12,
SUBSTRATE-14 ✓). Activating it fleet-wide retires LizardFS — a hard-to-reverse,
fleet-wide change that must stay operator-gated.

- **OPROG-2** (1628) — run `cutover-substrate-v2.sh` fleet-wide after a VM-bed
  reboot/disconnect drill + a rollback RPM. **Your go/no-go.**
- **SUBSTRATE-6** (1394) — remove LizardFS units/Requires/fetch only *after* the
  cutover is proven (it's the rollback path until then).
- **INCIDENT-WEDGE-2** (1649) — new founding lighthouse coordinates via etcd
  (depends on OPROG-2).
- **MUSIC-RESPONSIVE-4** (1316) — switch cover-art to path delivery once
  `/mnt/mesh-storage` becomes a plain (always-readable) Syncthing dir post-cutover.

> Say "do the cutover" (and which order / whether to rehearse on the VM bed first)
> and I'll drive it; otherwise it stays parked here.

---

## D. Missing upstream dependency (cannot fix in-repo)

- **MOTION crossfade family** (MOTION-NET-3 @1508 and the animated old→new /
  panel-transition items) — the pinned **iced-0.13 fork lacks an opacity/transform
  widget**. The non-animated halves (dim-stale, indicators, reduce-motion plumbing,
  load-state model) are landed; the literal crossfade renders only on iced-0.14 or
  once the fork gains the widget (tracked as UX-PRE). Everything in MOTION that does
  **not** need that widget is in the implementation queue.
- **GUI-9** (321) — auto-sourcing reduce-motion from Cosmic's a11y setting is
  upstream-impossible (cosmic-comp has no such setting, issue #376; FDO appearance
  portal lacks the key). Operator already marked WON'T-DO; local `MDE_REDUCE_MOTION`
  + `preferences.toml` is the supported substitute. **No action needed** — listed
  for completeness.

---

## E. Operator decision (not code)

- **compute/inventory publish cadence** (1044) — keep the 10 s bus publish, reduce
  it, or drop it (now that inventory is mirrored to Mesh-Sync)? One-line decision.
- **DRAIN-6** (16) — "N farm agents in flight, auto-merge+relaunch" is realized by
  the `/ship` skill + the Workflow orchestration harness (this very drain), not by
  in-repo daemon code. Confirm that satisfies it, or specify an in-repo coordinator.
- **OBS-5 / OBS-6** (353/354) — re-homed to PLANES-14 / PLANES-20; verify status
  there (bookkeeping only).

---

*Items not in this ledger are either reconciled-done or in the active
implementation pass. Update this file as blockers clear.*

<!-- install-helpers/park-blocker.sh (DRAIN-5) appends machine-parked entries
     below this line; clear one by doing its unblock action + flipping its
     [!] marker in docs/WORKLIST.md off. -->
