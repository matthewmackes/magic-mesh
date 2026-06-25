# SUBSTRATE-V2 — replace LizardFS with etcd (coordination) + Syncthing (files)

> **DONE — LizardFS is REMOVED (SUBSTRATE-6, the LizardFS rip-out).** This was the
> migration design; it has now fully landed. The live substrate is **etcd**
> (coordination) + **Syncthing** (files, on a plain `/mnt/mesh-storage` dir, no
> FUSE). LizardFS/MooseFS/Gluster/Ceph are retired and forbidden. The fs-path
> fallbacks and the "held until after cutover" rollback notes below describe the
> transition that is now complete — they are historical, not a live plane.

**Status:** locked via a 12-Q operator survey, 2026-06-19.
**Trigger:** Operator — "Remove the current filesystem-sync technology and replace
with Syncthing for file synchronization, and decouple coordination from storage.
Deeply investigate all implications and integrations while planning this move.
Update all informational panels, status outputs, alerts, boot processes, etc."

**Why:** the QNM-Shared LizardFS mount is a single substrate carrying *both* mesh
coordination (leader election, the peer directory, health) *and* bulk files. Two
reboots in two days proved the failure mode: when the mount is unhealthy the
whole mesh is down, because leader/directory live as lockfiles + JSON on the
mount. The fix is to split the two concerns onto purpose-built substrates:
**etcd** for strongly-consistent coordination (leader/directory/health) and
**Syncthing** for eventually-consistent file sync. LizardFS — a stagnant MooseFS
fork with a master SPOF, a FUSE-2 dependency on a FUSE-3 distro, and a fragile
overlay mount — is removed.

## Locked decisions

| # | Question | Lock |
|---|----------|------|
| **File plane (Syncthing)** | | |
| 1 | What Syncthing syncs | **Shared mesh folders only** — the document/file shares; coordination leaves the FS entirely. |
| 2 | Topology | **Full mesh** — every node ↔ every node shares the folder. |
| 3 | Folder path | **Keep `/mnt/mesh-storage`** — now a plain local dir (NO FUSE), so every existing reader works unchanged. |
| 4 | Conflicts/versioning | **Simple trash-can versioning** (`.stversions`); newest wins, replaced/deleted files recoverable. |
| **Coordination plane (etcd)** | | |
| 5 | etcd quorum | **All server + lighthouse nodes are members**; workstations are etcd **clients** only. |
| 6 | etcd scope | **Leader election + peer directory + health** ONLY (the strong-consistency core). Revisions/acks, capability tags, compute-inventory, favorites, and the alert-mirror stay **file-based** and ride Syncthing (their per-node-write / read-union pattern tolerates eventual consistency). |
| 7 | RPC strategy | **Keep the `action/*` RPC interfaces, swap the backend** to etcd — panels that use the RPCs need zero change; only the few direct-FS readers switch. |
| 8 | Boot ordering | **`mackesd After=etcd` (not any mount); `etcd After=nebula`.** Syncthing + `/mnt/mesh-storage` are NON-critical to mesh liveness. |
| **Cross-cutting** | | |
| 9 | Migration | **Big-bang at a version** — one release flips the substrate; nodes re-enroll/convert onto etcd + Syncthing. |
| 10 | LizardFS | **Fully removed** — units, `mfsmount` loop, BIRTHRIGHT fetch, bundled F43 client RPM, `fuse-libs`/`fuse` Requires, `lizardfs-adm`, and the §1–§3 substrate-lock entry. |
| 11 | Security/discovery | **Overlay-only, TLS optional** — Syncthing global/relay/local discovery OFF + static device IDs over Nebula; etcd bound to the overlay IP, client-cert TLS deferred (the overlay already encrypts). |
| 12 | Naming | **Rename** the share from "QNM-Shared" → **"Mesh Sync" (MCNF-Sync)**. The path stays `/mnt/mesh-storage`; the user-facing name + brand change. |

## Architecture

### Coordination plane — etcd
- A small **etcd cluster on every server+lighthouse node** (overlay-bound,
  `:2379` client / `:2380` peer on the Nebula IP). Workstations are clients that
  target the anchor endpoints. Cluster bootstrap (initial-cluster) is provisioned
  at enroll from the directory of anchors.
- `mackesd` embeds an etcd client (`etcd-client` crate). Coordination moves to
  keys:
  - **Leader** → `/mesh/leader` via the etcd lease + election API (a campaign on
    a 60 s lease; replaces `.mackesd-leader.lock` + `fs2` advisory lock).
  - **Peer directory** → `/mesh/peers/<hostname>` = the `PeerRecord` JSON, written
    by the heartbeat under a **keepalive lease (~90 s TTL)** so liveness is the
    lease, not a `last_seen_ms` staleness check. `read_peers` → an etcd range get
    on `/mesh/peers/`.
  - **Health/heartbeat** → folded into the peer-record lease + a `/mesh/health/<node>`
    key (or the existing `health` field on the peer record).
  - **Syncthing device registry** → `/mesh/syncthing/<hostname>` = the node's
    Syncthing device ID, so each node auto-configures the full-mesh share from
    etcd (closes the discovery loop without public discovery).
- The `action/mesh/directory`, `action/shell/healthz`, and related responders
  keep their exact wire shape — only the backend reads etcd. **All RPC-based
  panels are unchanged.**

### File plane — Syncthing
- A `syncthing` daemon per node (system service, overlay-only): global/relay/local
  discovery OFF, GUI bound to localhost (or off), the **`/mnt/mesh-storage`**
  folder shared **full-mesh** with every node's device ID (from the etcd
  registry), **trash-can versioning** (`.stversions`).
- `/mnt/mesh-storage` becomes a **plain directory** (no FUSE mount) → the apps
  aggregator, compute-inventory, probe-inventory, fleet revisions/acks, tags,
  favorites, and the alert-mirror all keep their existing file paths and Just
  Work, now synced by Syncthing instead of replicated by LizardFS.

### Boot model (fixes the failure class)
- `nebula.service` → `etcd.service` (anchors) + `syncthing.service` (all).
- `mackesd.service` `After=etcd.service` (Wants, with retry) — **never** ordered
  after a filesystem mount. A Syncthing hiccup or a slow first sync **cannot**
  stall the mesh; only file access degrades.
- `qnm-shared.service` (the LizardFS mount loop) is **deleted**.

## Integration inventory (from the deep exploration)

### Moves to etcd (coordination — backend swap)
- `crates/mesh/mackesd/src/leader.rs` + `workers/leader_election.rs` — lease →
  etcd election. (`.mackesd-leader.lock` retired.)
- `crates/mesh/mackes-mesh-types/src/peers.rs` — `write_peer_record`/`read_peers`/
  `peers_dir` → etcd `/mesh/peers/` put/range. `PeerRecord` schema unchanged.
- `crates/mesh/mackesd/src/telemetry/mod.rs` — heartbeat writes the peer record to
  etcd under a keepalive lease (replaces the FS heartbeat + `last_seen_ms`).
- `crates/mesh/mackesd/src/ipc/directory.rs` — `build_directory` reads etcd; RPC
  shape unchanged.
- `crates/mesh/mackesd/src/workers/health_reconciler.rs` — peer enumeration from
  etcd; the lease IS liveness.
- `crates/mesh/mackesd/src/ipc/shell.rs` (healthz) — leader + peer counts from etcd.

### Stays file-based → rides Syncthing (eventual consistency OK)
- Fleet revisions/acks/nudges (`magic-fleet/src/store.rs`), capability tags
  (`cap_tags.rs`), compute-inventory (`compute_registry.rs`/`ipc/apps.rs`),
  apps-favorites (`ipc/apps.rs`), the alert-mirror (`mde-notify/src/lib.rs`),
  probe-inventory, per-node sidecars. All keep their `/mnt/mesh-storage` paths.

### Panels (mostly unchanged via the RPC swap)
- **No change** (use `action/mesh/directory`/`healthz`/`fleet/*`): peers,
  fleet_rollup, home, config_apply (revisions still files), compute, network_hosts.
- **Direct-FS readers to switch to the RPC/etcd** (`read_peers`/lock reads):
  `panels/lighthouses.rs` (read_peers + mesh-status lighthouse_ips),
  `panels/service_publishing.rs` (read_peers), `panels/mesh_control.rs`
  (`.mackesd-leader.lock` direct read), `bin/mde-notify-center.rs` (read_peers).

### Status outputs
- `install-helpers/mesh-status-snapshot.sh` — peers + leader now from etcd
  (via a `mackesd` query or `etcdctl`) instead of the `peers/*.json` glob +
  `.mackesd-leader.lock`. `mesh-welcome.py`, the bell applet, the apps applet all
  consume `/run/mde/mesh-status.json` and need **no change** (snapshot generation
  changes, not the consumers). The apps applet's "QNM-Shared" usage label →
  "Mesh Sync".

### Alerts
- NO-LEADER / leader-change / peer-down alerts source from etcd watches instead
  of the lease file. The "QNM-Shared not mounted" alert/guard is replaced by
  "etcd unreachable" + "Syncthing down / folder out of sync" alerts.

### Boot / systemd
- New: `etcd.service` (anchors), `syncthing.service` (all), `setup-syncthing.sh`,
  `setup-etcd.sh` (replace `setup-qnm-shared.sh`).
- Removed: `qnm-shared.service`, the `mackesd` `20-qnm.conf` `After=qnm-shared`
  drop-in, the ONBOARD-6 mount guard (`shared_root_writable` / canonical-mount
  poison check — no FUSE mount to poison now).

### Health / tests / lint
- `install-helpers/mesh-health-check.sh` — watchdog checks etcd quorum health +
  Syncthing folder health instead of `mountpoint /mnt/mesh-storage`.
- `install-helpers/lint-shared-substrate.sh` — rewrite to assert the etcd
  readiness guard + the multi-node etcd test exist.
- `crates/mesh/mackesd/tests/mesh_shared_state.rs` — rewrite the leader/peer-
  visibility test against an **etcd testcontainer** instead of a shared tempdir.

### Birthright / packaging
- RPM: add `etcd` + `syncthing` (Fedora repos / pinned fetch); drop `fuse-libs`,
  `fuse`, the bundled `lizardfs-client` F43 RPM, `lizardfs-adm`. Remove the
  LizardFS BIRTHRIGHT air-gapped fetch.

### Naming
- "QNM-Shared" → "Mesh Sync" across UI strings (the Mesh Storage panel label, the
  apps-applet usage bar, the welcome banner, docs). Env var `MDE_WORKGROUP_ROOT`
  + the `/mnt/mesh-storage` path stay (back-compat); only the brand changes.

## AI_GOVERNANCE updates (§1–§3 substrate locks)
- Replace the "LizardFS is the mesh filesystem" lock with "**etcd** (coordination)
  + **Syncthing** (file sync) over Nebula; LizardFS/MooseFS/Gluster/Ceph are
  retired/forbidden." Keep the Nebula-only transport + Ed25519/AES-256-GCM crypto
  locks. Note coordination ≠ storage (the SUBSTRATE-V2 split).

## Acceptance (high level; per-task bullets in the worklist)
- A node boots: nebula → etcd (anchors) / etcd-client (workstations) → mackesd
  elects/joins via etcd in seconds, **independent of any file mount**. A reboot
  with Syncthing slow/down still brings the mesh fully up.
- `action/mesh/directory` + healthz return the live peer set + leader from etcd;
  every existing panel renders unchanged.
- `/mnt/mesh-storage` is a Syncthing folder: a file written on node A appears on
  node B; deletes/replacements land in `.stversions`.
- LizardFS is gone (no `qnm-shared.service`, no `mfsmount`, no fuse Requires).
- Status/welcome/applet/alerts reflect etcd + Syncthing health, not a mount.

## Risks / notes
- **etcd on the 947 MB shadow lighthouse** — etcd is light but quorum write
  latency over the overlay matters; tune heartbeat/election timeouts for WAN RTT
  (the DO nodes are ~15 ms). Validate memory headroom on the thin anchor.
- **Big-bang** has no live fallback — stage + rehearse on the VM bed before the
  fleet roll; keep a one-release rollback RPM.
- **Leaderless revision minting on Syncthing** — eventual consistency means two
  nodes could mint the same version concurrently; keep the append-only
  fail-if-exists guard + accept a rare re-mint (or move revisions to etcd later).
- **etcd auth deferred** — overlay-only is the security boundary initially; add
  client-cert TLS as a follow-on before any non-overlay exposure.

## Cutover runbook (SUBSTRATE-14)

The substrate code (SUBSTRATE-1..13) ships as **dormant bridges**: every reader is
`etcd-when-/etc/mackesd/etcd-endpoints-present, else the LizardFS fs path`, so the
live fleet is byte-for-byte unchanged until a node is provisioned onto etcd. The
cutover is the operator running `cutover-substrate-v2.sh` per node — writing the
endpoints file is what flips that node's coordination onto etcd.

1. **Rehearse on the VM bed first.** On the founding anchor:
   `cutover-substrate-v2 --init --listen <ip>`; each other anchor:
   `--join <founder-ip> --listen <ip>`; workstations: `--client-only --anchors <csv>`.
   Then drill: `etcdctl … endpoint health` shows quorum; `… get --prefix /mesh/peers/`
   shows the directory; a file written on A appears on B; **reboot a node** + a
   **disconnect** drill — the mesh must re-elect/rejoin in seconds, independent of
   any mount.
2. **Roll the fleet** at the 11.0 version (anchors first, then workstations).
3. **Then** land SUBSTRATE-6 (remove LizardFS) in the *next* release once the bed +
   fleet are proven on etcd+Syncthing — never before, so the fs fallback stays as
   the safety net through the transition.

**Rollback (one release):** `rm /etc/mackesd/etcd-endpoints` (bridges fall back to
the LizardFS fs path) + `systemctl disable --now etcd syncthing`; or reinstall the
prior NEVRA (still carries `setup-qnm-shared` + the LizardFS Requires until
SUBSTRATE-6 lands). This is why SUBSTRATE-6 is **held until after** the cutover is
proven — removing LizardFS removes the rollback path.

## Out of scope
- Moving revisions/acks/tags/compute/favorites/alert-mirror to etcd (they ride
  Syncthing per the lock; a later epic can promote any that need stronger
  consistency).
- Multi-cluster / cross-site etcd federation.
