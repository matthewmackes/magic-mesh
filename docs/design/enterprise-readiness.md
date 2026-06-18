# MCNF — Enterprise-Readiness Verification

> **Objective.** Verify, without assumptions, whether this mesh platform deserves to be called
> enterprise-grade; if it does not, produce the exact worklist required to make that claim true.

**Method.** Evidence-based audit of `/home/mm/magic-mesh` across installation, provisioning, the three
node types, configuration, testing, observability, security, reliability, UX, and documentation.
Every verdict below is anchored to a `file:line` fact found in the tree — nothing is marked Pass on
intent. Date: 2026-06-09. Workspace version 10.0.0.

---

## 1. Platform goal (verification statement)

MCNF is a **secure, no-fixed-center workgroup mesh** plus its native-Rust IBM-Carbon GUIs,
running on stock Fedora-Cosmic (`AI_GOVERNANCE.md` §0). Technically:

- **Problem solved.** Give a small workgroup (sized for ≤ 8 peers — `ca/sign.rs`) an encrypted overlay
  network with shared storage, shared config/desired-state, and device integration, **without a
  mandatory central controller** — any node can author fleet state, any node can leave, the fabric
  heals.
- **Mesh model.** A **Nebula** encrypted overlay (lighthouse-anchored discovery + hole-punching, with
  a lighthouse relay fallback). Identity is an Ed25519 node key; overlay membership is a Nebula peer
  cert signed by a mesh CA. Desired-state is a versioned baseline reconciled per-node (Ansible,
  `magic-fleet`). Mesh storage is LizardFS. The control-plane daemon is `mackesd`.
- **Node roles** (`mde-role`: Lighthouse ⊂ Server ⊂ Workstation, strict supersets):
  - **Lighthouse** — the stable rendezvous + the CA signing root + control-plane visibility.
  - **Headless / Server** — always-on infra (fleet, mesh storage) with no GUI.
  - **Workstation** — a Cosmic desktop with the operator/user GUIs.
- **Expected UX.** Provision a node in one guided step; it enrolls, pins its role, starts its services,
  and shows clear health/connection/error status with obvious recovery actions.
- **What "enterprise-grade" must mean here.** Not complex — **reliable, predictable, easy to operate**:
  a repeatable installer, safe provisioning + decommission, enforced security controls, built-in
  testing, actionable observability, automatic recovery, and complete operator + end-user docs.

The **engineering core is real and well-built.** The gap is that it is delivered as a **Cargo
workspace of service-shaped binaries**, not as an installable, operable, documented product.

---

## 2. Enterprise-readiness audit (by category)

### Installation — **FAIL**
No installer of any kind. The only shell script in the tree is `install-helpers/lint-mesh-boundary.sh`
(a CI lint gate, not an installer). No `Makefile`/`justfile`/`install.sh`. The only documented "install"
is `cargo build --workspace` (`README.md:46`), which does not check prerequisites (`gtk3-devel`,
`alsa-lib-devel`), OS, ports, permissions, or networking. No idempotent, scriptable, prerequisite-
validating install path exists.

### Packaging — **FAIL (unbuilt, self-acknowledged)** *(historical — resolved: PKG-1..10 shipped; `[package.metadata.generate-rpm]` + kickstart + GitHub-hosted repo all exist under `packaging/`)*
No `.spec`, no `[package.metadata.generate-rpm]`, no `cargo-deb`, no kickstart `.ks`, no COPR config,
no Containerfile. `docs/COMPLIANCE.md:85` confirms packaging is unbuilt; the entire PKG-1..10 epic is
open in the worklist.

### systemd / boot-start — **FAIL** *(historical — resolved by ENT-6/PKG-3: `packaging/systemd/` ships `mackesd.service` (Restart=on-failure) + `mde-musicd.service` + the operator-enabled voice units)*
Zero `.service`/`.target`/`.socket`/`.timer`/`.preset` units ship. The binaries are service-shaped
(`mackesd serve` boots a tokio supervisor and blocks on SIGTERM behind the `async-services` feature;
`mde-bus daemon`, `magic-fleet watch`, `mde-musicd serve` are long-running) and several doc-comments
reference a `mackesd.service` / `journalctl -u mde-bus` — but **no unit exists to start any of them at
boot, and nothing restarts `mackesd` if it crashes.**

### Provisioning — **PARTIAL**
- **Node identity:** automatic — `identity.rs:29` `NodeKey::generate()` (Ed25519, OS CSPRNG), node-id =
  SHA-256 of the pubkey; hardware fingerprint from `/etc/machine-id`.
- **Headless/Workstation join:** **good and retry-safe** — `mackesd enroll --token mesh:<id>@<ip>:<port>#<bearer>`
  writes an atomic `pending-enroll.json`, polls 30 s for a signed bundle; the lighthouse auto-signs
  (`nebula_csr_watcher`, 30 s tick) or an operator runs `mackesd ca sign-csr`. Idempotent; re-enroll by
  hardware fingerprint refreshes in place; a timeout returns an actionable error. **Easy for a non-expert.**
- **Lighthouse (first node):** **PARTIAL / not easy** — `mackesd ca mint` creates the CA, but there is
  **no single bootstrap command**; the lighthouse "self-signs its own peer cert" only in prose
  (`nebula_enroll.rs:330`), and the self-enroll-with-overlay-IP step is not wired as a command. No
  "initialize new mesh vs join existing" gesture.
- **Role pinning:** **broken in practice** — `mde-role::pin_at` enforces upgrade-only correctly but is
  **lib-only with no front-end**; nothing in the repo (no installer, no `mackesd role pin`) ever writes
  `/var/lib/mde/role.toml`. Every box therefore runs **unpinned → defaults to Workstation** (`worker_role.rs:69`).
- **Decommission:** **PARTIAL / unsafe-by-surprise** — `mackesd decommission <id>` is a **DB soft-delete
  only**; it does NOT revoke the cert, ban the identity, or tear down local `/etc/nebula/`. Cert/trust
  revocation is a **separate, uncoordinated** `mackesd ca revoke`. A node "decommissioned" but not
  "revoked" keeps full mesh access (flat trust). Local teardown lives in an absent `mde-install`.

### Configuration — **PARTIAL**
Config is file-based + human-readable (`role.toml`, Nebula configs, QNM-Shared JSON). Role config is
validated (upgrade-only, fail-closed load). But: there is a hardcoded lighthouse overlay IP
(`10.42.0.1`, `bin/mackesd.rs:2376`), the role file is never written by anything shipped, and there is
no documented config inventory, backup, or recovery story for non-CA config.

### Testing — **PARTIAL**
~3,900 unit tests across the workspace (strong: mackesd 1,265, mde-workbench 764, mde-bus 325). **But
the only multi-node integration test (`integration_testcontainers.rs`) spins up the RETIRED
Headscale/Tailscale substrate**, is behind an off-by-default `docker-tests` feature, and
`skip_if_no_docker!()` **silently reports PASS** with no daemon. **No CI** (`.github/workflows` absent).
No install / live-Nebula provisioning / upgrade / rollback testing. `failure_scenarios.rs` is in-process
diff-engine logic, not live-node failure simulation.

### Observability — **PARTIAL**
Signals exist (`heartbeat` 10 s → `health_reconciler` 5 s → `nodes.health` + `PeerStateChanged`;
Netdata per-node; `mesh_latency` — admittedly a `ping` **placeholder**) and are surfaced piecemeal via
CLI (`status`, `healthz`, `nodes`, `events`, `peers-why`) and several GUI panels. **No consolidated
fleet-health view** any node can produce (OBS-6 open). The GUI **Logs panel reads dead desktop-era
paths** (`~/.local/share/mackes-shell/mackes.log`, `journalctl -u sway`) — not mackesd's output.

### Security — **FAIL (multiple findings that would fail an enterprise review)**
1. **Enrollment auth is documented but NOT enforced.** `sign_pending_csr` (`nebula_enroll.rs:571`) checks
   the ban list + 8-peer cap, then signs — it **never compares the bearer/passcode against any issued
   allow-list**, though `:67` and `:148` claim it does. There is no issued-bearer table. **Anyone who
   can write a well-formed `pending-enroll.json` into QNM-Shared gets auto-signed a valid overlay cert.**
   The "shared passcode gate" is effectively absent at the enforcement point. *(Critical vulnerability.)*
2. **Flat trust (open-mesh).** Any enrolled cert reaches every peer + every service; no per-service ACL,
   no segmentation (`ca/mod.rs:10`). One compromised node = full blast radius.
3. **Revocation does not evict live nodes.** `ca revoke` marks the DB + ban list + a best-effort bus
   event, but pushes **no CRL/blocklist to the Nebula data plane** — a revoked 365-day cert stays valid
   mesh-wide until expiry.
4. **Backup passphrase + placement.** `MDE_BACKUP_PASSPHRASE` is read from the process env (the header
   tells operators to put it in a systemd `Environment=` line — world-readable via `systemctl show`),
   and the sealed **CA bundle is replicated to every node** via QNM-Shared. Secrecy reduces to that env var.
5. **sshd flat-open** on the overlay to all peers, no ACL, by directive (`sshd_overlay_bind.rs:242`).
6. **Root apply + unsigned revisions.** `magic-fleet` renders `become:true` plays applied as root; revision
   election is newest-wins with **no signature check** of the author. Latent (not yet gossip-auto-wired;
   today via `ansible-pull` from an operator URL), but a RISK the moment gossip-apply lands.
7. **Audit gaps.** The fleet `events` table IS hash-chained + `audit-verify`-able (strong) — but security
   events (enroll/sign/revoke/rotate) go to `tracing` only, the network-state `audit_log.rs` JSONL has no
   chain, and the KDC `.also_log` authz hook is a **no-op stub**.
8. **No per-role hardening.** The Lighthouse (CA root, highest value) carries the same flat trust + exposure
   as any worker node. *(Mitigations that ARE correct: CA key sealing 0600 + owner-check; systemd-creds
   passcode-at-rest design — though unwired; argv-vector shell-outs — no command injection found.)*

### Reliability — **PARTIAL**
Nebula gives genuine peer-direct survival of a lighthouse outage (direct UDP primary, relay fallback;
`nebula.service` preserved on demotion). But the mackesd worker supervisor is a **"Phase A" stub** —
fixed 250 ms back-off, **no max-restarts, no circuit-breaker** (`workers/mod.rs:430`) — and **nothing
restarts `mackesd` itself** (no systemd unit). Auto-reconnect after a blip is delegated entirely to
Nebula; there's no app-level reconnect/health-driven re-handshake. Backup/restore mechanism is decent
(sealed CA + meshfs snapshot, tamper-checked `state-restore`) but **opt-in, single-copy, no DR runbook.**

### User experience — **PARTIAL**
Strong CLI verb surface on `mackesd` (~50 verbs incl. `enroll`, `ca *`, `decommission`, `reenroll`,
`state-restore`, `healthz`, `reconcile`, `revisions`, `nodes`, `events`). **But none of the enterprise
lifecycle gestures exist as a clean named command:** no `install`, `provision`, `doctor`/self-test
(`healthz` is shallow), `join` (it's `enroll`), unified `test connectivity`, `logs`, `repair`, or
`leave`/`uninstall`. `magic-fleet` has no clap help at all. No guided setup; ops documentation is absent.

### Documentation — **FAIL for operations**
`README.md` is build/architecture only. `DISCLAIMER.md` still titled **"Mackes Workstation"** (stale) and
**explicitly says "not for production"** and disclaims recovery/HA. No install guide, operator runbook,
per-node-type setup guide, troubleshooting guide, or DR runbook. Code points at a `docs/help/mesh-recovery.md`
that **does not exist**.

---

## 3. Node-type verification

### Lighthouse node — **PARTIAL → FAIL on operability**
| Need | Verdict | Evidence |
|---|---|---|
| One-command/guided creation | **FAIL** | `ca mint` only; self-cert prose-only; no bootstrap command |
| Identity generation | **PASS** | `identity.rs:29`, `ca/mint.rs:46` |
| Secure bootstrap | **PARTIAL** | CA key sealed 0600; but role never pinned, enrollment gate unenforced |
| Static/discoverable endpoint | **PARTIAL** | overlay IP hardcoded `10.42.0.1` |
| Health check | **PARTIAL** | `healthz` shallow |
| Logs | **PARTIAL** | tracing, no unit/journal wiring, GUI panel reads dead paths |
| Restart behaviour | **FAIL** | no systemd unit; supervisor stub |
| Backup/restore | **PARTIAL** | sealed backup + `state-restore`; opt-in, single-copy |
| Replacement | **FAIL** | uncommanded, undocumented; runbook file missing |
| Multi-lighthouse | **FAIL** | roster supports it; `--lighthouse` flag "future", not implemented |
| Operator docs | **FAIL** | none |

### Headless / Server node — **PARTIAL (best of the three)**
| Need | Verdict | Evidence |
|---|---|---|
| CLI install | **FAIL** | no installer (binary builds only) |
| Non-interactive provisioning | **PARTIAL** | `enroll --token` is scriptable; but role not pinned |
| Secure enrollment | **FAIL (security)** | flow works + retry-safe, but bearer never validated at signing |
| Automatic service start | **FAIL** | no systemd unit |
| Mesh-join verification | **PARTIAL** | enroll returns success/timeout; no `test connectivity` |
| Remote management | **PARTIAL** | sshd-on-overlay + CLI; flat-open, no ACL |
| Logging | **PARTIAL** | tracing only |
| Self-test | **PARTIAL** | `healthz`; no `doctor` |
| Update | **FAIL** | no packaging/update path (PKG open) |
| Decommission | **PARTIAL** | `decommission`+`ca revoke` uncoordinated; no local teardown |

### Workstation node — **PARTIAL**
| Need | Verdict | Evidence |
|---|---|---|
| Simple installer | **FAIL** | none |
| Guided setup | **FAIL** | no first-run chooser (PKG-5 open) |
| Status display | **PARTIAL** | workbench panels (mesh_topology/health_check/drift) |
| Connection visibility | **PARTIAL** | `PeerStateChanged`→home panel |
| Friendly errors | **PARTIAL** | enroll errors are clear; broader flows not surfaced in GUI |
| Local diagnostics | **PARTIAL** | scattered CLI probes; no unified panel |
| User-safe defaults | **PARTIAL** | unpinned→Workstation; flat trust |
| Reconnect/repair/remove cleanly | **FAIL** | no repair/leave; reconnect = Nebula only |

---

## 4. Enterprise readiness scorecard

| Area | Current state | Enterprise expectation | Status | Evidence | Risk | Required fix |
|---|---|---|---|---|---|---|
| Installation | none (cargo build only) | idempotent, prereq-checking installer | **FAIL** | only `lint-mesh-boundary.sh` | Cannot deploy repeatably | PKG-1/2 + an `install`/bootstrap |
| Provisioning | enroll works; role unpinned | guided, role-pinned, retry-safe | **PARTIAL** | `enroll --token` ok; `pin` lib-only | Boxes default to Workstation | Role-pin front-end + chooser |
| Lighthouse creation | `ca mint` only | one bootstrap command + replacement | **FAIL** | self-cert prose-only | No clean first-node / replacement | `mackesd mesh init` + runbook |
| Headless creation | enroll scriptable; no unit | CLI install + auto-start | **PARTIAL** | no installer/unit | Manual, non-bootable | Installer + service |
| Workstation creation | no guided setup | first-run wizard | **FAIL** | PKG-5 open | Non-experts blocked | Cosmic first-run chooser |
| Authentication | Nebula cert | enforced enrollment auth | **FAIL** | bearer never checked (`nebula_enroll.rs:571`) | **Unauthorized enrollment** | Enforce the bearer at signing |
| Authorization | flat trust | least-privilege/segmentation | **FAIL** | `ca/mod.rs:10` | Full blast radius | Per-service scopes (or accept + document risk) |
| Key management | CA key 0600 ok; backup passphrase in env | sealed, least-exposure | **PARTIAL** | `seal.rs` ok; `nebula_ca_backup.rs:152` | Master passphrase leak | systemd-creds for backup passphrase |
| Configuration | file-based, validated | versioned, backed-up, recoverable | **PARTIAL** | role.toml never written | Role gating theoretical | Write role.toml at provision |
| Testing | unit-rich; no CI; retired integ. test | CI + live-Nebula integ. + install/upgrade | **PARTIAL** | OBS-1/2/3 open | Regressions ship | CI + Nebula harness |
| Observability | piecemeal; no fleet view | single fleet-health view | **PARTIAL** | OBS-6 open; `mesh_latency` placeholder | Hard to operate | Mesh Health panel + `fleet status` CLI |
| Logging | text tracing; dead GUI panel | structured, centralized/accessible | **PARTIAL** | `panels/logs.rs` dead paths | Can't diagnose | `logs` command + fix panel + OBS-5 |
| Upgrade | none (no packaging) | safe, tested upgrades | **FAIL** | PKG open | No update path | Packaging + upgrade tests |
| Rollback | revision-level only | tested rollback | **PARTIAL** | `revisions rollback` exists; untested | Risky changes | Rollback tests |
| Recovery | `state-restore`; no runbook | documented DR | **PARTIAL** | runbook missing | Tribal recovery | DR runbook + multi-copy backup |
| Documentation | build/arch only | full ops + user docs | **FAIL** | README only; DISCLAIMER stale | Unoperable by others | Ops + user docs |
| User experience | rich CLI; no lifecycle verbs | obvious operate flow | **PARTIAL** | no doctor/repair/leave | Steep | `meshctl`-style lifecycle UX |
| Security hardening | flat, no per-role | hardened, per-role, revocable | **FAIL** | §2 security list | Multiple review failures | Enrollment auth, CRL eviction, hardening |
| Operational readiness | binaries only | installable, observable, recoverable | **FAIL** | sum of above | Not operable as a product | The ENTERPRISE epic below |

**Pass count: 0 full-Pass areas.** Strong PARTIALs (enrollment flow, audit chain, unit tests, restore
mechanism) sit on top of FAIL-level operability and security-enforcement gaps.

---

## 5. Gaps

### Critical blockers (must fix before "enterprise-grade" is even arguable)
1. **Enrollment authentication is not enforced** (`nebula_enroll.rs:571`). *Risk: unauthorized nodes
   join.* Fix: validate the bearer against an issued-but-unredeemed allow-list at `sign_pending_csr` +
   the auto-signer; single-use bearers. Verify: an enroll with a wrong/replayed bearer is refused (test).
2. **No installer / packaging / systemd units** (PKG-1..10). *Risk: cannot deploy or boot repeatably.*
   Fix: the PKG epic. Verify: `dnf install magic-mesh` → reboot → `mackesd.service` active.
3. **Role is never pinned** (lib-only `pin_at`). *Risk: every node silently runs as Workstation.* Fix: a
   `mackesd role pin` front-end + the install chooser write `role.toml`. Verify: a Headless install gates
   to rank-1 workers.
4. **Revocation doesn't evict live nodes** (`revoke.rs`). *Risk: a compromised cert stays valid up to a
   year.* Fix: push a Nebula `pki.blocklist` to the data plane on revoke + reload. Verify: a revoked
   node can no longer reach peers within N seconds.
5. **No operations / DR documentation** (and `DISCLAIMER.md` says "not for production"). *Risk: not
   operable by anyone but the author; no recovery path.* Fix: install guide + per-role runbook + DR
   runbook. Verify: a new admin provisions all three node types from docs alone.

### High-priority gaps
6. **No crash-restart for `mackesd`** (no unit) + **supervisor is a 250 ms fixed-retry stub** (no
   max-restarts/circuit-breaker, `workers/mod.rs:430`). Fix: ship `mackesd.service` (Restart=on-failure)
   + implement bounded exponential back-off + circuit-breaker.
7. **`decommission` ≠ `ca revoke`** (uncoordinated; soft-delete leaves trust). Fix: a single
   `mackesd leave`/`decommission` that revokes + bans + tears down local state; link the two.
8. **Lighthouse bootstrap + replacement uncommanded/undocumented**; **multi-lighthouse `--lighthouse`
   flag unimplemented**. Fix: `mackesd mesh init` + a replacement runbook + the roster flag.
9. **No CI; the one integration test targets the retired Headscale/Tailscale substrate, off-by-default,
   silent-skip** (OBS-1/3). Fix: retarget to Nebula containers, hard-fail on daemon-absent, GH Actions.
10. **Backup passphrase in env + single-copy replicated bundle.** Fix: systemd-creds for the passphrase;
    multi-copy / off-mesh backup option.
11. **No unified observability** (`fleet status` CLI + Mesh Health panel; `mesh_latency` is a `ping`
    placeholder). Fix: OBS-6 + a `mackesd fleet status` that any node can run.

### Medium-priority
12. **No `doctor`/`test`/`logs`/`repair` lifecycle commands** — scattered probes only. Fix: a `meshctl`-style
    facade (see §8).
13. **GUI Logs panel reads dead desktop paths** (`mackes-shell`/sway). Fix: read mackesd tracing/journald.
14. **Flat trust + sshd flat-open + root apply + unsigned revisions** — accept-and-document, or add
    per-service scopes + revision signing before gossip-apply is wired.
15. **Security-event audit is `tracing`-only** (no chain). Fix: append enroll/sign/revoke/rotate to the
    hash-chained `events` table; wire the KDC `.also_log`.

### Polish / usability
16. `DISCLAIMER.md` stale name ("Mackes Workstation"). 17. `magic-fleet` has no `--help`. 18. Hardcoded
overlay IP `10.42.0.1`. 19. README has no deploy section. 20. Config inventory/backup undocumented.

---

## 6. Acceptance criteria (each testable)

- **Fresh install** — *Req:* one command installs all node software on a clean Fedora-Cosmic box. *Test:*
  `dnf install magic-mesh` (from the GitHub-hosted dnf repo — superseded COPR, operator decision 2026-06-10) on a fresh VM. *Expected:* binaries + `mackesd.service`
  present; `systemctl status mackesd` shows loaded; prerequisites validated or pulled as deps.
- **Lighthouse creation** — *Req:* one guided command stands up a new mesh's first node. *Test:*
  `mackesd mesh init --workgroup acme`. *Expected:* CA minted, role pinned Lighthouse, self peer-cert +
  overlay IP issued, `nebula.service` up, a join token printed.
- **Headless join** — *Test:* `mackesd enroll --token <t>` on a Server box. *Expected:* signed bundle in
  ≤30 s, role pinned Server, rank-1 workers running, `mackesd healthz` green; a **wrong** token is refused.
- **Workstation join** — *Test:* the Cosmic first-run chooser → pick Workstation + paste token.
  *Expected:* enrolled, GUIs launch, mesh status shows connected.
- **Multi-node mesh validation** — *Test:* `mackesd fleet status` on any node. *Expected:* all peers
  Online with versions + leader; a connectivity self-test passes peer-to-peer.
- **Node restart** — *Test:* `kill -9 mackesd`. *Expected:* systemd restarts it ≤ N s; workers resume;
  no manual repair.
- **Node replacement** — *Test:* destroy the Lighthouse, restore on a new box from backup. *Expected:*
  per the DR runbook, CA restored, mesh signing resumes; documented end-to-end.
- **Failed provisioning recovery** — *Test:* interrupt enrollment mid-flight, re-run. *Expected:* idempotent
  completion, no duplicate identity.
- **Security validation** — *Test:* attempt enroll with an invalid/replayed bearer; revoke a node then
  probe peer reachability. *Expected:* enroll refused; revoked node evicted from the data plane ≤ N s.
- **Logging validation** — *Test:* `mackesd logs --since 1h` (or `journalctl -u mackesd`). *Expected:*
  structured, queryable, current logs.
- **Operator documentation** — *Test:* a new admin provisions all three node types using only the docs.
  *Expected:* success without reading source.
- **End-user documentation** — *Test:* a non-expert connects a Workstation from the user guide. *Expected:*
  connected; can see status + reconnect.
- **Uninstall / decommission** — *Test:* `mackesd leave`. *Expected:* cert revoked + banned, node removed
  from roster, local `/etc/nebula/` + keys + role wiped; `dnf remove magic-mesh` clean.

---

## 7. Implementation worklist (lifted to `docs/WORKLIST.md` as the ENTERPRISE epic)

The enterprise gaps **overlap the existing survey epics** (PKG = installation/packaging/role-chooser;
OBS = testing/CI/observability; SEC = parts of security; FLEET-PHASE-G = the control plane). This report
adds the **enterprise-specific tasks the survey did not capture** — see ENT-1..14 in `docs/WORKLIST.md`.
Highlights (full detail + acceptance in the worklist):

- **ENT-1 (CRITICAL): enforce the enrollment bearer** at `sign_pending_csr` + the auto-signer (issued,
  single-use allow-list). *mackesd ca/enroll.* Test: wrong/replayed bearer refused.
- **ENT-2 (CRITICAL): write `role.toml` at provision** — `mackesd role pin` + chooser. Test: Server box
  gates to rank-1.
- **ENT-3 (CRITICAL): revocation evicts the data plane** — push Nebula `pki.blocklist` + reload on revoke.
- **ENT-4: `mackesd mesh init`** — one-command Lighthouse bootstrap (mint + self-cert + role-pin + start).
- **ENT-5: unify `mackesd leave`/`decommission`** — revoke + ban + local teardown in one verb.
- **ENT-6: `mackesd.service` + worker-supervisor hardening** (Restart=on-failure; bounded back-off + circuit-breaker).
- **ENT-7: `mackesd doctor`** — unified self-test (identity, role, nebula, peers, storage, services).
- **ENT-8: `mackesd fleet status`** — any-node whole-fleet view (peers/versions/leader/health).
- **ENT-9: `mackesd logs`** + fix the GUI Logs panel to read mackesd output.
- **ENT-10: connectivity self-test** — `mackesd test connectivity` peer-to-peer.
- **ENT-11: DR runbook + multi-copy/off-mesh backup + systemd-creds passphrase.**
- **ENT-12: operator + end-user documentation** (install, per-role setup, troubleshooting, DR).
- **ENT-13: replace `mesh_latency` ping placeholder** with the real transport probe.
- **ENT-14: security-event audit** (enroll/sign/revoke/rotate → hash-chained `events`; wire KDC `.also_log`).

These compose with the survey epics; the **single biggest unlocks** are the PKG epic (makes it
deployable) and ENT-1/3 (makes it securely enrollable + revocable).

---

## 8. Verification commands (proposed `meshctl`-style facade)

The lifecycle gestures don't exist as named commands today. Recommend a thin facade (a `meshctl` binary
or `mackesd` subcommands) so operators don't memorize ~50 verbs:

```bash
meshctl install --role lighthouse|server|workstation   # ENT/PKG — does not exist
meshctl status                                          # ~ mackesd status (shallow today)
meshctl doctor                                          # ENT-7 — does not exist (closest: healthz)
meshctl provision --role server --token <t>             # ~ mackesd enroll --token (rename/guide)
meshctl join --token <t>                                # ~ mackesd enroll --token
meshctl test connectivity|dns|firewall                  # ENT-10 — scattered probes only today
meshctl logs [--since 1h]                               # ENT-9 — does not exist
meshctl fleet status                                    # ENT-8 — does not exist
meshctl repair                                          # ~ magic-fleet heal / mackesd reconcile
meshctl leave|decommission                              # ENT-5 — split + incomplete today
meshctl mesh init                                       # ENT-4 — does not exist (ca mint only)
```

Today's real equivalents: `mackesd enroll|ca *|decommission|reenroll|state-restore|healthz|reconcile|
revisions|nodes|events|peers-why`, `magic-fleet apply|heal|converge|watch|elect`. The gap is a
*coherent operator UX over them*, not raw capability.

---

## 9. Verdict — **Prototype with enterprise direction**

**Not enterprise-grade yet — and not "nearly."** The platform has a genuinely strong engineering core:
a no-fixed-center Nebula control plane with real crypto (Ed25519/AES-256-GCM/RSA-4096, correct CA-key
sealing), a retry-safe token enrollment flow, a hash-chained verifiable fleet-event audit log, a
tamper-checked backup/restore primitive, ~3,900 unit tests, and a rich CLI. That is well above
hobby-grade. But it fails the enterprise bar on the things enterprise specifically means:

- **It cannot be installed or booted as a product** — no installer, no packaging, no systemd units, no CI.
- **Its primary security control is not implemented** — the enrollment bearer is never checked, so the
  authentication boundary is "can you write to the shared dir," not "do you hold the passcode."
- **It cannot securely off-board** — revocation is bookkeeping that doesn't evict a live node.
- **It cannot be operated by anyone but its author** — role-pinning is never wired, lifecycle commands
  (doctor/logs/repair/leave) don't exist as such, and there is **zero operations/DR documentation** (the
  DISCLAIMER itself says "not for production").

**What must be true before the claim is honest** (minimum bar): (1) the PKG epic — one installable,
self-starting RPM with a working role chooser; (2) ENT-1 — enforce the enrollment bearer; (3) ENT-2 —
actually pin the role at provision; (4) ENT-3 — revocation that evicts the data plane; (5) ENT-5 — a
clean `leave`; (6) ENT-6 — `mackesd.service` + a real supervisor; (7) ENT-7/8/9 — doctor/fleet-status/logs;
(8) OBS CI + a live-Nebula integration test; (9) ENT-12 — operator + DR documentation. Until those land,
"enterprise-grade" is an intention, not a fact.

The good news: the survey already specified most of this (PKG, OBS, SEC, FLEET-PHASE-G), and the
enterprise-specific remainder is the bounded ENT-1..14 list above. The path is clear and the core is
sound — it's a **prototype with a credible, well-scoped route to enterprise-grade**, not a rewrite.

---

## 10. Corrective decisions (locked 2026-06-09)

Ten decision-forks locking *how* the gaps get fixed (→ the ENT tasks + governance §8):

| # | Issue | Decision |
|---|-------|----------|
| C1 | enrollment bearer unenforced (ENT-1) | **single-use issued-bearer allow-list** at signing |
| C2 | revocation doesn't evict (ENT-3) | **nebula `pki.blocklist` + reload** on revoke |
| C3 | unpinned → Workstation (ENT-2) | **refuse to start** until a role is pinned (fail closed) |
| C4 | no lifecycle commands (ENT-7/8/9/10) | a **`meshctl` operator facade** (ENT-15) |
| C5 | no crash-restart + supervisor stub (ENT-6) | **systemd unit + hardened in-process supervisor** |
| C6 | decommission ≠ revoke (ENT-5) | **both** self-service `leave` + operator decommission |
| C7 | flat trust | **keep open-mesh, document the blast radius** (governance §8) |
| C8 | backup passphrase in env (ENT-11) | **systemd-creds** passphrase (keep single QNM copy) |
| C9 | "not for production" | **production workgroup-grade (≤8 peers)** (governance §8) |
| C10 | security events untracked (ENT-14) | **hash-chained `events`** + wire KDC `.also_log` |

The minimum path to honestly claim the standard: **PKG** (deployable) + **ENT-1/2/3** (secure
enroll/role/revoke) + **ENT-5/6/7/8/9** (leave + resilience + operator UX) + **OBS CI** + **ENT-12**
(docs + positioning). The trust model and ≤8-peer envelope are now fixed in `AI_GOVERNANCE.md §8`.

---

*Verification complete: installation, provisioning, all three node roles, configuration, testing,
observability, security, reliability, UX, and documentation reviewed; scorecard, gaps, acceptance
criteria, worklist, verification commands, verdict, and corrective decisions produced. No step was
skipped; nothing was marked Pass without evidence.*
