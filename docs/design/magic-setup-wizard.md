# Magic Setup Wizard — full-lifecycle TUI (SETUP epic, 2026-06-14)

A single verbose ratatui wizard, `magic-setup`, that takes a freshly-`dnf
install`ed Fedora Server 43/44 node from zero to a fully-running mesh member —
create or join a mesh, pick a role, stand up + keep running **all** services,
and add/remove peers — narrating every step in real time. Fedora-idiomatic
(systemd first-run + MOTD, Ansible for steady-state convergence).

## Locks (2026-06-14 survey + operator directives)

| # | Decision | Lock |
|---|----------|------|
| 1 | TUI shape | **One `magic-setup` binary** (grown from the ONBOARD-5 `mde-enroll` ratatui code). Top menu: Create mesh / Join mesh / Manage peers / Status. `mde-enroll` becomes the join-only shim that calls into it. |
| 2 | First-run | **Console first-run unit + MOTD.** `magic-setup.service` launches the wizard on the console when the node is **unconfigured** (Fedora initial-setup/firstboot pattern), self-disables once a role is pinned; an MOTD/login banner shows `sudo magic-setup` as the fallback. |
| 3 | Config method | **Hybrid: imperative verbs bootstrap, Ansible converges.** First bring-up uses the live verbs/scripts (`found`/`join`/`setup-qnm-shared`/`systemctl` — they need interactive IO + fp pinning). The wizard then emits `/etc/mackesd/site.yml` and runs it via the platform's `ansible_pull` for reproducible steady-state convergence + every later change. |
| 4 | Lighthouses | **Up to 3** (overrides §8 single-lighthouse). LH1 = founding CA holder + LizardFS master + `/enroll`. LH2/LH3 = additional **public** lighthouses for NAT relay/discovery redundancy + LizardFS metalogger + QNM-Shared replica. One CA (on LH1) keeps ≤8-peer signing simple; the bundle roster lists all live lighthouses. |
| 5 | Mesh identity | **By lighthouse IP.** Join keys on a lighthouse IP (the v3 token carries `mesh:<id>@<ip>:<port>#<bearer>?fp=`); a peer may join via **any** of the 1–3 lighthouse IPs. The mesh-id + CA fingerprint disambiguate. |
| 6 | NAT posture | **All Full + most Headless nodes are behind hostile NAT.** Only lighthouses need public IPs. Peers dial lighthouses **outbound** (`/enroll` over TLS, then nebula UDP); nebula hole-punches + relays via the lighthouses (`am_relay`), so 2–3 lighthouses materially raise reachability. |

## Roles (install-time, superset chain — §5 unchanged)
- **Lighthouse** — public IP, no desktop; nebula lighthouse + relay, `/enroll`
  endpoint, LizardFS master (LH1) or metalogger (LH2/3), QNM-Shared replica.
- **Headless (Server)** — behind NAT; full mackesd workers, QNM-Shared client,
  no desktop GUIs.
- **Full (Workstation)** — behind NAT; everything Headless has + the Cosmic
  desktop GUIs (workbench/files/applet/wallpaper).

## Wizard flow (each screen verbose, real-time log pane)
1. **Detect state** — unconfigured (no role pinned) vs configured (show Status).
2. **Create a new mesh** (founds LH1): pick mesh-id → detect/confirm public IP →
   `mackesd found` (CA + self-sign + `/enroll` cert) → `setup-qnm-shared
   --master --chunkserver --client` → `enable --now` every service → print the
   peer + lighthouse join lines → emit + run `site.yml`. Live log every step.
3. **Join an existing mesh**: enter a **lighthouse IP** + paste token (or scan) →
   choose role (Lighthouse adds LH2/3 / Headless / Full) → `mackesd join`
   (fp-pinned network enroll) → if Lighthouse: `setup-qnm-shared --chunkserver
   --client` + register as nebula lighthouse in the roster → `enable --now` all
   → converge via Ansible.
4. **Manage peers** (lighthouse only): list directory; **Add peer** mints a
   single-use v3 token (shows the `magic-setup`/`mde-enroll` join line + a
   copy-paste block); **Remove peer** runs `decommission` + cert revoke + ban;
   **Add lighthouse** mints a lighthouse-join token (up to 3).
5. **Status / services**: leader, node_count, per-service health (mackesd,
   nebula, qnm-shared, lizardfs, mesh-health), overlay reachability; restart
   any; re-run convergence.

## Service set the wizard guarantees running + boot-durable (ONBOARD-9 manager)
`nebula`, `mackesd`, `mesh-health.timer`, `qnm-shared` (+ `lizardfs-master`
/`-chunkserver`/metalogger per role), all `enable --now` + auto-recovery +
failure alerts. "Keep them running" = the existing manager (Restart + watchdog).

## Ansible (platform standard, hybrid)
The wizard generates `/etc/mackesd/site.yml` (role, mesh-id, lighthouse roster,
service set, QNM-Shared params) from the bootstrap result and runs it via
`ansible_pull` (the existing worker). Bootstrap stays imperative (interactive
token/fp); **all subsequent config is Ansible-converged**. Ships an
`ansible/` role with tasks mirroring the verbs for idempotent re-apply.

## Acceptance
- `dnf install magic-mesh` on a fresh Fedora Server 43/44 → next console login
  shows the MOTD + the wizard auto-launches (unconfigured); pick Create/Join →
  fully-running member, no manual `systemctl`.
- A node behind NAT joins a 1–3-lighthouse mesh by lighthouse IP + token; overlay
  forms via hole-punch/relay; survives reboot (all services boot-durable).
- Up to 3 lighthouses: founding + 2 added; peers list all 3; losing one keeps the
  overlay + QNM-Shared (metalogger/replica) up.
- Add/remove peer from the wizard works end-to-end; `mackesd peers` reflects it.
- Steady-state changes go through the generated Ansible `site.yml`.

## Risks / out-of-scope
- Multi-lighthouse **CA replication** is out (one CA on LH1; LH2/3 are
  discovery/relay/storage redundancy + can forward enroll to LH1). Full CA HA is
  a later epic if a lighthouse loss must not block new enrollments.
- LizardFS shadow-master auto-promotion (metalogger → master on LH1 loss) is
  manual in v1 (documented runbook); auto-failover is a follow-up.
- The console first-run unit must never hijack an SSH session (console/tty only).
