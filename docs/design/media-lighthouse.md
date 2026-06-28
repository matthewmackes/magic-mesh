# MEDIA-LIGHTHOUSE — Navidrome music service on `Lighthouse_Media` nodes

**Status:** design locked 2026-06-20 (10-question operator survey).
**Epic prefix:** `MEDIA-` in `docs/WORKLIST.md`.

A mesh-native, hot-redundant **music service**: a Subsonic server runs in a
Podman container on a new class of lighthouse, reads a shared 100 GB
DigitalOcean Spaces bucket, and is published as the mesh's **Auto-Configuration
host** for the existing Music System (`mde-music` GUI + `mde-musicd`). A node
that enrolls gets working music with zero manual setup; losing any one media
lighthouse leaves music reachable.

## Locks (the 10-question survey)

| # | Decision | Lock | Why |
|---|----------|------|-----|
| 1 | Container image | **Navidrome** (not Airsonic-Advanced) | Go single ~50 MB binary, low RAM, built-in SQLite, full Subsonic API `mde-musicd` already speaks. Airsonic-Advanced is JVM-heavy. |
| 2 | Shared 100 GB store | **DigitalOcean Spaces** (S3 object store) | Lighthouses are DO droplets; DO-managed durability; one bucket every instance reads. Mounted as a POSIX path (Navidrome needs a filesystem, not native S3). |
| 3 | Library state model | **Per-instance scan + local SQLite** | Stateless readers of the shared bucket; no shared-DB-over-network footgun. Playlists/state replicated so any instance serves them. |
| 4 | Redundancy topology | **Active-active, all serve** | Every media lighthouse runs a live instance; clients fail over. Natural fit for stateless readers; true hot redundancy. |
| 5 | Client endpoint | **mesh-DNS `music.mesh`** | `mesh_dns` resolves to every media-lighthouse overlay IP (round-robin + failover). No VIP. The current creds default already uses a `.mesh` host. |
| 6 | Auto-config delivery | **Birthright at enroll + service registry** | `mackesd` writes `airsonic-creds.json` (→ `music.mesh` + shared account) on enroll; already-enrolled nodes pick it up from the registry. The "Auto-Configuration host" core. |
| 7 | Account model | **One shared service account** | Simplest under the ≤8-peer flat-trust envelope (§8); playlists are mesh-wide. |
| 8 | Published-service registration | **Mesh registry + mesh DNS** (internal) | Shows in the Workbench published-services surface + `music.mesh`. No public exposure by default. |
| 9 | Host gating | **New `Lighthouse_Media` role class** | A dedicated, adequately-resourced lighthouse subclass hosts the container — NOT a RAM-gate on the tiny stock lighthouses (the 947 MB master OOM-thrashed on netdata). |
| 10 | Content / source of truth | **Operator upload, bucket = source of truth** | Operator adds music to the Spaces bucket (rclone / a mesh Files surface); a rescan picks up changes. |

## Architecture

```
            DigitalOcean Spaces  (one 100 GB bucket, S3)
              ▲            ▲            ▲
        rclone mount  rclone mount  rclone mount   (read-mostly /music)
              │            │            │
        ┌─────┴────┐ ┌─────┴────┐ ┌─────┴────┐
        │ Navidrome│ │ Navidrome│ │ Navidrome│      Podman, MemoryMax/CPUQuota
        │ (podman) │ │ (podman) │ │ (podman) │      local SQLite scan each
        └─────┬────┘ └─────┬────┘ └─────┬────┘
        Lighthouse_Media  Lighthouse_Media  Lighthouse_Media   (new role class)
              │            │            │
              └──────── overlay ────────┘
                         │
                 mesh_dns: music.mesh → all media-LH overlay IPs (A-records, failover)
                         │
       ┌─────────────────┴───────────────────┐
   mde-musicd (every node) ── airsonic-creds.json written at ENROLL
       → server_url=http://music.mesh:4533, shared service account
       → mde-music GUI streams; published in the service registry + Workbench
```

- **`Lighthouse_Media` role class** — extends the Lighthouse tier; provisioned
  with enough RAM/disk to host Navidrome. The container worker gates to this
  class (stock lighthouses and peers skip it). Identified the same way other
  roles are (role marker / Nebula membership, per LIGHTHOUSE-9).
- **DO Spaces** — one bucket, S3 creds held as a **leader-managed mesh secret**
  (encrypted on Mesh-Sync, the XCP-7 / VOIP-GW secret pattern). Provisioned via
  the DO API (`doctl`/`s3cmd`), 100 GB.
- **Navidrome container** — `podman` (reuse the `compute_*`/lifecycle plumbing
  or a dedicated `media_service` worker), mounts the bucket via `rclone mount`
  (or `s3fs`) at `/music` read-mostly, scans into a container-local SQLite,
  hard-capped (`MemoryMax`, `CPUQuota`). Default port **4533**.
- **Active-active + `music.mesh`** — `mesh_dns` publishes an A-record set of all
  media-lighthouse overlay IPs; the `mde-musicd` browse retry (MUSIC-RESPONSIVE-1)
  + client reconnect handle a dead instance.
- **Shared service account** — auto-provisioned in Navidrome at first start;
  password is a leader-managed secret distributed inside the birthright creds.
- **Auto-config (the headline)** — `mackesd` writes `airsonic-creds.json`
  (`music.mesh:4533` + shared account) at enroll, and the service is registered
  so already-enrolled nodes self-configure. mde-musicd then uses it by default;
  the manual connect form becomes an override, not a requirement.
- **Published service** — registered in the mesh service registry (Workbench
  published-services surface) + `music.mesh`.

## Acceptance (epic-level, runtime-observable)

- A fresh node enrolls → `mde-music` opens and browses the shared library with
  **no manual connect** (creds auto-written, pointing at `music.mesh`).
- ≥2 `Lighthouse_Media` nodes each run a live Navidrome reading the same bucket;
  powering one off leaves `music.mesh` resolving + streaming from the others.
- The service appears in the Workbench published-services surface + resolves via
  `music.mesh`.
- Music uploaded to the Spaces bucket appears after a rescan on every instance.
- The container is absent on stock lighthouses / the tiny master and on peers.

## Risks / watch-items

- **`rclone mount` over S3 latency** — first-scan + cover-art reads can be slow;
  cache locally (rclone VFS cache) and bound it. Validate scan time on a real
  bucket.
- **Tiny-node protection** — the whole reason for `Lighthouse_Media`; never let
  the container land on the 947 MB master (netdata-thrash memory).
- **Spaces S3 creds** — must be a leader-managed encrypted secret, never in
  `ps`/logs/env (mirror EFF-21 + XCP-7).
- **DNS failover sharpness** — A-record round-robin fails over only as fast as
  clients retry; the MUSIC-RESPONSIVE-1 retry + reconnect cover it, but verify
  the user-visible gap on an instance kill.
- **Shared-account write contention** — playlists written from two instances
  concurrently; per-instance SQLite means playlist writes must land on the
  shared store or a designated writer. Resolve in MEDIA-6/MEDIA-3.

## Out of scope (this epic)

- Public/off-mesh streaming via the Caddy ingress (internal-only lock; opt-in later).
- Per-user / per-node accounts (single shared account locked).
- A shared Postgres / replicated DB (per-instance scan locked).
- Transcoding profiles / mobile apps beyond what Navidrome ships by default.

## Operating the service (implemented surfaces)

The epic's code (the non-bucket-dependent half) is landed in `mde-role` +
`mackesd`:

- **`Lighthouse_Media` role (MEDIA-1)** — `mde_role::Role::LighthouseMedia`
  (canonical slug `lighthouse-media`), selectable at install (the role chooser)
  and enroll (`mackesd join --role lighthouse-media`). It is a lateral, rank-0
  *media-capable lighthouse*: `mackes_mesh_types::lighthouse::is_lighthouse`
  still counts it (HA/roster/quorum), `is_media_lighthouse` isolates the
  subclass, and the heartbeat stamps `role="lighthouse-media"` into the
  replicated directory so a node's class is identifiable mesh-wide. The
  media-only worker tier gates on the orthogonal capability
  (`worker_role::node_serves_media`), so the container is provably absent off the
  subclass.
- **Navidrome supervisor (MEDIA-3)** — `workers::media_navidrome` ADOPTS +
  self-heals the `mcnf-navidrome.service` unit (restart on down), media-gated.
- **`music.mesh` (MEDIA-5)** — `workers::mesh_dns::build_records_with_music`
  emits the active-active A-record set of every live `Lighthouse_Media` overlay
  IP; membership tracks join/leave automatically off the directory.
- **Shared account (MEDIA-6)** — the single Navidrome account's password is a
  leader-managed mesh secret (`ipc::secret_store`, the XCP-7 pattern), minted
  once + distributed into the root-only creds env file (`ND_ADMIN_*`) without
  clobbering the operator's `DO_SPACES_*`.
- **Published service (MEDIA-7)** — the `music` row is registered in the mesh
  service registry (`action/nebula/published-services`) only while a node serves
  it, with per-instance health; it de-registers (no stale entry) on teardown.
- **Birthright (MEDIA-8)** — at enroll, `mackesd` writes
  `~/.local/share/mde/airsonic-creds.json` → `http://music.mesh:4533` + the
  shared account, so `mde-music` browses with no manual connect.

### Content ingestion (MEDIA-9)

The operator owns the library; the bucket is the source of truth. The helper
`install-helpers/mcnf-music-ingest.sh` wraps the two verbs:

```sh
# add music to the shared bucket (rclone copy; idempotent, secrets off-argv)
mcnf-music-ingest.sh upload /path/to/album            # → bucket root
mcnf-music-ingest.sh upload /path/to/album Artists/X  # → a sub-path

# re-index every live instance (Subsonic startScan over the music.mesh A-set)
mcnf-music-ingest.sh rescan
```

It reuses the same root-only creds env file (`/etc/mackesd/media-spaces.env`)
the leader-managed secret path writes, resolves the `music.mesh` A-set to reach
every instance, and never puts a secret on `argv`. A live bucket (MEDIA-2,
operator-blocked on Spaces keys) is needed only to exercise it end-to-end; the
path itself is landed.

## Related work

Builds on: the `compute_*`/podman lifecycle workers, `mesh_dns`, the service
registry, the leader-managed-secret pattern ([[XCP-7]], VOIP-GW), the DO
live-test-bed workflow, LIGHTHOUSE-9 (role identification), and the Music System
(`mde-musicd` creds, MUSIC-RESPONSIVE-1 retry).
