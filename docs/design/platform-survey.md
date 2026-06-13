# Magic Mesh — Platform Build-Out Survey (100 questions)

**Purpose.** The worklist holds *findings* ("this is incomplete"), not *specs* ("build exactly
this"). This survey turns the open platform decisions — the under-specified worklist items (B1
Mesh SSH, C1 Phase-G control plane, E1 Nebula test harness, H3/H4 wire-vs-remove, H6 Radio, H8
KDC seam) plus the broader direction the repo hasn't pinned — into a set of concrete, mutually-
exclusive choices. Each question locks **one** decision that becomes one or more worklist tasks.

**How to answer.** Reply with the question number and your letter, e.g. `Q3: A, Q4: C`. Skips are
fine (mark `defer`). Where a question offers a "defer / keep stub" option, that's a legitimate
lock too — it just means the finding stays open with an explicit owner.

**What happens next.** Once answered, each lock is written into `docs/design/<epic>.md` and lifted
into `docs/WORKLIST.md` as user-story tasks (per `plan`). Authority order when locks conflict:
Memory > `AI_GOVERNANCE.md` > design docs > worklist body (newest wins).

**Grounding.** Every question was authored against the actual repo state (cited files are real).
Notable realities the survey surfaced: there is **no CI**, **no RPM/packaging**, and **no
cosmic-applet** yet; `mde-theme` ships 2 themes though §4 names 3 (Gray 10/90/100); three
incompatible fleet-revision schemes coexist; notifications have two competing paths (mackesd's FDO
host vs Cosmic); and several "Carbon" a11y/animation levers are plumbed but unread at runtime.

Sections: **§1** Mesh substrate & fleet control plane (Q1–18) · **§2** Security, KDC & crypto
(Q19–34) · **§3** Carbon GUIs & components (Q35–52) · **§4** Mesh SSH, remote access & services
(Q53–70) · **§5** Deployment, roles & packaging (Q71–86) · **§6** Testing/CI, observability &
lifecycle (Q87–100).

---

## §1 — Mesh Substrate & Fleet Control Plane (Phase-G, worklist C1)

### Q1. Which revision-id scheme does Phase G standardise on for the fleet desired-state?
*Three live, mutually-incompatible schemes exist: `magic-fleet::Revision` (monotonic `u64 version` + author tiebreak, YAML-gossiped), `mackesd/src/revisions.rs` (`r-YYYY-MM-DD-NNNN` full-payload snapshots), and `mackesd/src/fleet.rs` `record_push` (SQLite rowid into `desired_config`). Phase G's Bus verbs (`fleet.rs:59`) must elect ONE.*
- A) Adopt `magic-fleet::Revision`'s monotonic `u64 version` as the canonical id; retire the rowid + date-string schemes to derived display fields
- B) Adopt the `r-YYYY-MM-DD-NNNN` string id (operator-readable date inline) as canonical; map it onto the `version` field for gossip ordering
- C) Adopt the `desired_config` SQL rowid as the source of truth; the `u64 version` and date-string are projections the writer stamps
- D) Content-address each revision by a hash of its serialized baseline; `version`/date/rowid all become non-authoritative metadata

### Q2. What is the on-wire revision payload format that gossips peer-to-peer?
*`magic-fleet::Revision::to_yaml` already serializes a full `BaselineSpec` to YAML for gossip, but `mackesd`'s `desired_config.spec_json` stores JSON and `revisions.rs` stores `payload_json`. The transport format must be pinned once.*
- A) YAML (matches `magic-fleet`'s `to_yaml`/`from_yaml` and the operator-authored baseline files)
- B) Canonical JSON (matches `desired_config.spec_json` + `revisions.payload_json`, and `mde-bus` is JSON-native)
- C) A signed envelope (Ed25519 over canonical-JSON body per §3) — format is JSON but the unit of gossip is the signature-wrapped blob

### Q3. How does a revision actually move between peers — what is the gossip/relay transport?
*`magic-fleet elect` reads candidate revisions off the local disk; nothing routes them over Nebula today. §1 says "revisions gossip peer-to-peer (hop-relay via a lighthouse)". The transport is unbuilt.*
- A) A `mackesd` gossip worker that periodically POSTs its newest revision to every known Nebula peer IP, hop-relaying through the lighthouse when a peer is unreachable directly
- B) Lighthouse-anchored pull: peers poll the lighthouse(s) for the newest revision; the lighthouse is a relay/cache, not an authority
- C) Write revisions into LizardFS mesh storage and let LizardFS replication carry them; `mackesd` watches the shared path
- D) Defer the transport; ship Phase G with leader-mirrored `desired_config` over the existing shared-filesystem path and gossip later

### Q4. When two peers author concurrent revisions, how is the conflict resolved?
*`magic-fleet::Revision::supersedes` defines a total order (version, then `at`, then `author`), so two authors at the same `version` silently resolve by lexical author — the lower-author's edits vanish with no signal.*
- A) Keep last-writer-wins by the existing total order (version → at → author); silent loss is acceptable for a config plane
- B) Detect the collision (same parent version, two children) and surface it as a `ManualReview` drift row in the Pending Changes inbox, blocking auto-apply until an operator picks
- C) Three-way field-level merge of the two `BaselineSpec`s; only truly-overlapping keys escalate to manual review
- D) Require revisions to carry a parent-version pointer and reject any revision whose parent isn't the current winner — authors must rebase

### Q5. Does "any node can author" coexist with the leader lockfile, or does the leader gate writes?
*`leader.rs` elects one writer to `desired_config` via a 60s-lease lockfile, but §0/§1 say any node can author fleet state.*
- A) Authoring is leaderless (any node mints a revision and gossips it); the leader lock only guards the local SQLite mirror write, not the right to author
- B) Only the leader may mint new revisions; followers propose to the leader, which serializes them — keeps the lockfile authoritative
- C) Drop the leader lock for the fleet plane entirely; the deterministic newest-wins election replaces leader election for desired-state
- D) Leader mints by default, but any node can `--force`-author (epoch bump, like `force_take`) when partitioned from the leader

### Q6. What are the rollback semantics for the `rollback` Bus verb?
*`revisions.rs` stores full payloads precisely so "rollback is always a copy operation, never a replay." But rolling back on one node while peers hold a newer gossiped revision will just lose the election next tick.*
- A) Rollback mints a NEW revision whose payload copies the target's, with a `version` higher than current — so it wins the election and propagates fleet-wide
- B) Rollback is node-local: it pins this node to an older revision and adds the newer one to its `LocalExceptions`, leaving the fleet untouched
- C) Rollback tombstones every revision newer than the target across the fleet (a gossiped "retract" record); the target becomes the live winner again
- D) Defer rollback as a Phase-G+1 verb; ship push/list/diff first and keep rollback stubbed

### Q7. How does `diff-revisions` compute its diff given nested baseline structure?
*`revisions.rs::diff` walks only top-level JSON keys and stringifies values, so a one-package change inside `packages: [...]` shows as the whole `packages` array changing.*
- A) Keep the flat top-level diff; the GUI renders the changed domain array verbatim (cheap, already built)
- B) Diff per-resource within each `BaselineSpec` domain (package-by-name, service-by-name, file-by-path) so a single-resource edit shows as one row
- C) Structural recursive JSON diff (path-keyed down to leaves), domain-agnostic

### Q8. Where does the authoritative revision history live?
*`record_push` writes `desired_config` + per-peer `fleet_settings_apply_log` rows in each node's local SQLite. With leaderless authoring, every node's DB could diverge.*
- A) Each node's local SQLite is authoritative for what THAT node applied; the gossiped revision set is the shared truth, reconstructed on demand
- B) The leader's SQLite is the canonical log; followers mirror it (matches today's leader-writer model)
- C) An append-only revision log in LizardFS mesh storage is canonical; SQLite is a local cache/index of it
- D) Revisions are content-addressed and self-certifying (Ed25519-signed); "history" is just the set every node can independently verify — no single authoritative store

### Q9. How does `magic-fleet` reconcile a fleet baseline against node-local settings the `desired_config` plane also manages?
*Two desired-state surfaces exist: `magic-fleet`'s OS `BaselineSpec` and `mackesd`'s `desired_config` settings revisions (`theme.accent` etc. via `plan_push`) — separate stores, separate apply paths.*
- A) Keep them fully separate: `magic-fleet` owns OS state, `desired_config` owns app/settings state; a revision references both but they apply independently
- B) Fold settings into `BaselineSpec` as a new domain so one revision = one playbook = one apply path
- C) Make `desired_config` the envelope: a revision carries a `BaselineSpec` blob that `mackesd` hands to `magic-fleet`, unifying on the Bus verbs

### Q10. How is `magic-fleet`'s apply actually Podman-isolated, as governance §1 requires?
*§1 says the Automation Mesh is "Podman-isolated", but `magic-fleet::apply` lays a private-data-dir under `std::env::temp_dir()` and shells `ansible-runner` directly on the host with `become: true` — no container.*
- A) Run `ansible-runner` inside a Podman container (`--process-isolation`), bind-mounting only the data-dir and the host paths the baseline touches
- B) Keep host-local apply (Ansible needs root host access anyway); "Podman-isolated" describes the daemon, and this lock is reinterpreted
- C) Two-tier: render + dry-run in Podman for validation, then apply on the host after the dry-run passes
- D) Defer isolation; ship host-local apply now and containerise in a follow-up task

### Q11. Which node owns the LizardFS master, and how is that role assigned?
*`meshfs/snapshot.rs` records `CS-LIST` + a floating VIP (`10.42.0.1`) and `mackesd` "owns the mount" (§1), but nothing assigns the LizardFS master/metadata-server role across the fleet.*
- A) The mackesd leader (per `leader.rs`) co-locates the LizardFS master; master follows leadership, anchored at the VIP
- B) The master is pinned to Lighthouse-role nodes (per the §5 role hierarchy), independent of the mackesd leader lease
- C) A fleet revision declares the master assignment explicitly (operator-authored); no implicit coupling to leader or role

### Q12. What replication goal does mesh-storage default to, and who sets it?
*`snapshot.rs` reads `goal` via `mfsgetgoal` and the restore worker re-sets it, but no code establishes the live default goal at bootstrap; headroom assumes 1.5× overhead.*
- A) Goal 2 (one replica) fleet-wide as the default; operators raise it per-directory via a fleet revision
- B) Goal scales with fleet size (e.g. min(3, chunkserver count)); `mackesd` recomputes on membership change
- C) Goal is a desired-state field in the fleet revision (operator-declared), with goal 2 as the seed; no automatic scaling

### Q13. Which XDG directories does the mesh-storage mount actually own at apply time?
*`headroom.rs::default_xdg_dirs` lists Documents/Pictures/Music/Videos/Downloads as the five the FUSE mount owns once MESHFS-3.3 lands; §1 says "`~/Local/` is never mesh-mounted." The wiring is unbuilt.*
- A) Mount exactly those five XDG dirs onto LizardFS, bind-redirecting each to a per-user subtree; `~/Local/` stays local
- B) Mount one mesh root (`/mnt/mesh-storage`) and symlink the five XDG dirs into it; simpler, but symlinks leak the mount path
- C) Make the owned-dir set a fleet-revision field (operator picks which XDG dirs replicate per node); the five are the default

### Q14. How does a node signal apply success back so the lifecycle FSM can advance to Verified?
*`reconcile/mod.rs` defines `Deploying → Applied → Verified` and `magic-fleet` produces an `ApplyReport`/`AuditRecord` JSONL locally, but there's no path back from the node's apply result to the authoring side.*
- A) Each node gossips an apply-ack (revision id + `DriftStatus`) back; the author advances to Verified when a quorum/all-targeted peers ack
- B) The leader polls each peer's `fleet_settings_apply_log`/audit JSONL via the Bus and aggregates; Verified is the leader's call
- C) Fire-and-forget: a revision is Applied once gossiped; "Verified" is dropped from the fleet FSM since there's no central observer

### Q15. What is the event topic + emitter for revision-apply signals on the Bus?
*`ipc/fleet.rs` notes the old `revision_applied` D-Bus signal retired and "Phase G adds `event/fleet/signals` + a worker emitter + the Workbench subscription." The shape and trigger are unspecified.*
- A) `event/fleet/signals` carrying `{revision_id, peer, status}`, emitted by the reconcile worker on every lifecycle transition
- B) Per-verb topics (`event/fleet/applied`, `event/fleet/rolled-back`) so subscribers filter cheaply without parsing a status field
- C) Reuse the drift-watch audit JSONL as the event source — a tailer worker republishes new audit lines onto one `event/fleet/audit` topic

### Q16. What does `list-revisions` return, and scoped to what?
*Today `home.rs probe_fleet_revision` calls `list-revisions` and gets the stub "No revisions pushed yet." With leaderless gossip, a node may hold revisions it authored, applied, and merely heard about.*
- A) Return only revisions this node has applied (its `desired_config`/audit history) — local truth, fast
- B) Return the full gossiped set this node currently holds (applied + pending + superseded), tagged with which is the elected winner
- C) Return the elected-winner revision plus a count of held candidates; full list is a separate verb

### Q17. How are gossiped revisions authenticated so a peer can't inject a forged desired-state?
*§3 pins Ed25519 node identity, and Nebula authenticates the transport, but `magic-fleet::Revision` carries a plaintext `author` string with no signature.*
- A) Rely on Nebula's transport auth alone (only enrolled peers can gossip); the `author` field is advisory, not a security boundary
- B) Ed25519-sign each revision with the author's node key; peers verify against the enrolled cert before electing it (author becomes load-bearing)
- C) Sign + add an authoring-authority policy (a revision is only valid if its author is in a fleet-authorised author set), gossiped as part of desired-state

### Q18. How does a newly-joined or long-partitioned node converge — full state or catch-up?
*`elect_revision` picks the newest from whatever set a node holds, and `revisions.rs` stores full payloads (no deltas), so a cold node that hears only the latest revision applies complete desired-state but has no history for `diff`/`rollback`.*
- A) Cold nodes apply the single newest revision (full payload) immediately; history back-fills lazily over gossip — fast convergence, late diff/rollback
- B) A joining node pulls the full revision log from a lighthouse/peer before applying, so diff/rollback work from the first tick
- C) Snapshot + tail: pull the current winner as a snapshot plus the last N revisions for history; older history stays remote

---

## §2 — Security, KDC, Enrollment & Crypto

### Q19. What lifecycle should Nebula peer certs have between epoch rotations?
*`ca/sign.rs` mints peer certs with a `cert_lifetime_days` default of `365`; `epoch::bump_epoch` re-signs the whole roster on rotation, but nothing auto-renews a cert mid-epoch as it nears expiry.*
- A) Add a renewal worker that re-signs any peer cert within N days of `expires_at` under the current epoch (no rotation needed)
- B) Tie cert lifetime to epoch only — make certs effectively non-expiring (100-year) and rely on rotation/revocation for turnover
- C) Shorten the default to a 90-day lifetime and force a roster-wide `bump_epoch` as the sole renewal mechanism
- D) Defer — 365-day certs + manual `mackesd ca rotate` is acceptable until a peer actually expires in the field

### Q20. Who is allowed to trigger a `mackesd ca rotate` (CA epoch bump) on a no-fixed-center mesh?
*`epoch::bump_epoch` is called from `leader.rs` on leader-election win OR the `mackesd ca rotate` CLI. With no fixed center, two nodes could both believe they lead.*
- A) Only the current lockfile-leader (§1) may rotate; the CLI on a non-leader refuses and points at the leader
- B) Any node may rotate locally; the highest-epoch CA wins deterministically and gossips out (mirrors the revision-election model)
- C) Rotation requires an explicit operator passphrase/confirmation regardless of leader status (never automatic on promotion)
- D) Defer — keep leader-on-promotion + CLI as-is until multi-author rotation conflicts are observed

### Q21. How should the CSR auto-signer gate new node enrollment?
*`nebula_csr_watcher` polls `pending-enroll.json` every 30s and calls `sign_pending_csr` automatically; the only gate is the shared 16-char passcode plus the ban list.*
- A) Keep auto-sign on valid passcode (trust-on-first-use): the passcode is the single "on the mesh" gate per the open-mesh directive
- B) Require explicit operator approval per CSR — the watcher only surfaces pending requests; a human runs `mackesd ca approve <node-id>`
- C) Hybrid: auto-sign known hardware-fingerprints (re-enroll), but hold first-time fingerprints for explicit approval
- D) Defer — passcode + ban-list gate stands until a hostile-enrollment incident

### Q22. What is the "max key complexity" target §3 calls for, for the enrollment passcode?
*`enrollment.rs` hands peers a 16-char URL-safe passcode validated only by `looks_valid`; §3 mandates "Enrollment uses max key complexity," but length/charset isn't pinned by a config test.*
- A) Pin a config test: ≥16 chars from the full URL-safe alphabet (~95 bits), asserted the way `RSA_MODULUS_BITS` is
- B) Raise to a 256-bit (43-char) base64url passcode and pin that as the floor
- C) Replace the human-typed passcode with a QR/file-delivered 256-bit token (no manual typing, max entropy)
- D) Defer — 16 URL-safe chars is "max enough"; leave `looks_valid` as the only check

### Q23. The KDC device identity is now RSA-4096, but §3 also pins Ed25519. Should the KDC identity stay RSA?
*`keygen.rs` notes RSA-4096 is "the strongest RSA the KDE-Connect-compatible protocol interops with"; the mesh node identity is already Ed25519. Two identity systems, two algorithms.*
- A) Keep RSA-4096 for the KDC host — stock KDE-Connect interop requires RSA; the divergence is intentional and locked
- B) Add an Ed25519 KDC identity path for MDE-to-MDE peers, falling back to RSA-4096 only for stock-client interop
- C) Migrate the whole KDC identity to Ed25519 and drop stock KDE-Connect interop entirely
- D) Defer — document the RSA/Ed25519 split as a known, accepted §3 carve-out

### Q24. How should first-pair trust be established in the KDC pinning model?
*`tls.rs FirstPairVerifier::verify_server_cert` returns OK for any cert; the SHA-256 fingerprint is recorded post-handshake — trust-on-first-use with no out-of-band check.*
- A) Keep TOFU — matches upstream KDE Connect; the operator visually confirms the device in the pairing surface
- B) Require the operator to confirm the fingerprint out-of-band (compare a short hash on both devices) before recording the pin
- C) Add a pairing PIN/passcode exchanged during first-pair, bound into the recorded record alongside the fingerprint
- D) Defer — first-pair accept-any stays until a LAN MITM scenario is demonstrated

### Q25. How should first-pair pinning actually happen, given the inbound listener refuses unpinned devices?
*`lan.rs handle_inbound` rejects when `pinned.is_empty()`, deferring first-pair to "the pairing flow, not this listener" — but no module currently records the fingerprint into the empty pin.*
- A) Build an explicit outbound first-pair flow (operator-initiated) that completes the handshake via `FirstPairVerifier` and writes the fingerprint into `DeviceRecord`
- B) Let the inbound listener record-and-pin on first contact for a paired-but-unpinned device (TOFU on the inbound path)
- C) Pin out-of-band at pair time: capture the peer's advertised cert fingerprint from its UDP/mDNS announce before any TLS
- D) Defer — leave the unpinned-refusal seam until the pairing surface lands

### Q26. The KDC2-4 mesh-shunt seam (`SyntheticAnnounce`/`inject_synthetic`, worklist H8) is unconsumed. What do we do with it?
*`discovery.rs` ships `SyntheticAnnounce` + `inject_synthetic` with full tests, but no worker calls it; H8 says "land the KDC2-4 worker that consumes it, or drop the pub seam."*
- A) Build the KDC2-4.3 mesh-shunt worker that relays neighbors' `phones.json` into `inject_synthetic` (off-LAN phones reachable mesh-wide)
- B) Drop the `SyntheticAnnounce`/`inject_synthetic` pub surface now (§7 dead-code) and re-add when KDC2-4 is scheduled
- C) Keep the data model but gate it behind a feature flag so it's not "dead" but also not shipped
- D) Defer — leave the soft seam as-is (H8 is marked soft)

### Q27. How should the receiver of a `SyntheticAnnounce` decide whether to trust the relayer?
*`SyntheticAnnounce.relayed_by` carries the relayer id, but `inject_synthetic` does no trust check; the comment claims "the trust model (cert fingerprint pinning) is the same either way."*
- A) Accept any relayer — fingerprint pinning at connect time is the real gate, so a hostile relay can at worst surface an unreachable peer
- B) Only accept synthetic announces relayed by a node that is itself a paired/CA-signed mesh peer
- C) Require the relayer to sign the `SyntheticAnnounce` (wire up the "signature placeholder") and verify before injection
- D) Defer — decide relayer trust when the mesh-shunt worker (H8) is built

### Q28. How should CA revocation propagate across the no-fixed-center mesh?
*`revoke::revoke_peer` marks the DB row, writes the node-id to the local ban list (relying on filesystem replication), and fires a best-effort `ca/revoke/<node-id>` Bus event. No active mesh-wide push.*
- A) Keep ban-list-via-filesystem-replication as the durable channel + best-effort Bus event for fast local convergence (current design)
- B) Gossip a signed revocation record peer-to-peer (like fleet revisions) so every node converges without depending on shared-storage replication
- C) Make revocation effective only at the next CA epoch rotation (revoked nodes simply aren't re-signed)
- D) Defer — current three-step revoke is adequate until a propagation gap is observed

### Q29. Where should the authoritative ban list live?
*`ca::ban_list` writes a per-node ban file under QNM-Shared keyed by `self_node_id`; each peer has its own list and relies on replication to converge — no single mesh-wide list.*
- A) Keep per-node ban files + filesystem replication (no central authority, matches §0)
- B) Fold the ban list into the gossiped fleet revision so it's versioned and elected like other mesh state
- C) Store bans in each node's SQLite and reconcile via the mesh, dropping the shared-file dependency
- D) Defer — per-node ban files stand

### Q30. Should the Nebula CA key be encrypted at rest?
*`ca/seal.rs` enforces mode-0600 + owner-uid only; the bytes are unencrypted PEM on disk. The encrypted form (XChaCha20-Poly1305) exists only in the off-cluster backup.*
- A) Keep filesystem-permission sealing — 0600 + owner-uid is the bar; full-disk encryption is the operator's job
- B) Encrypt the CA key at rest with an operator passphrase (reuse the backup's Argon2id + XChaCha20-Poly1305), unsealed into memory on daemon start
- C) Move the CA key into an OS keyring / TPM-backed store and keep only a handle on disk
- D) Defer — mode-0600 sealing is sufficient for now

### Q31. How should CA backup be enabled, given it silently no-ops without `MDE_BACKUP_PASSPHRASE`?
*`nebula_ca_backup.rs` logs once then no-ops when the env var is unset, so a lighthouse with no passphrase silently never backs up its CA — a single-point-of-loss risk.*
- A) Keep opt-in-via-env — backups require an operator-chosen passphrase; no passphrase is a deliberate choice
- B) Make backup mandatory on lighthouse role: refuse to fully start (or loudly warn every tick) until a passphrase is configured
- C) Auto-generate a passphrase sealed into the mesh secrets store so backup is always on without operator action
- D) Defer — silent no-op stands; document the requirement in the runbook

### Q32. Is co-mingling CA keys with mesh-state snapshots in one backup bundle the right scope?
*`state-backup.enc` (schema v3) folds the CA payload + an optional LizardFS topology snapshot into one XChaCha20-Poly1305 bundle.*
- A) Keep one combined bundle — single passphrase, single restore step, atomic
- B) Split CA-key backup (high-sensitivity) from mesh-state snapshot (operational) into separate files with separate passphrases
- C) Keep one file but encrypt the CA-key section under a second, stronger passphrase layer (defense-in-depth within the bundle)
- D) Defer — combined v3 bundle stands

### Q33. Which symmetric cipher should the KDC session layer use?
*`mde-kdc-proto/crypto.rs` seals session packets with AES-256-GCM (ring) + a caller-owned monotonic nonce; §3 pins both AES-256-GCM and ChaCha20-Poly1305 as acceptable.*
- A) Keep AES-256-GCM with the caller-supplied monotonic nonce (current; matches stock-client expectations)
- B) Switch the KDC session to ChaCha20-Poly1305 for nonce-misuse margin and a single AEAD shared with the rest of the fabric
- C) Negotiate per-session (AES-GCM with stock clients, ChaCha20-Poly1305 between MDE peers)
- D) Defer — AES-256-GCM session encryption stands

### Q34. Should KDC session keys stay in-memory only?
*`pairing.rs` delegates session keys to an in-memory `RingKeyStore` (zeroized on drop); a daemon restart drops all live sessions, forcing re-handshake.*
- A) Keep in-memory-only session keys (re-handshake on restart) — minimizes key-at-rest exposure
- B) Persist session keys encrypted-at-rest so links survive a daemon restart without a full re-pair
- C) Add a PQ-ready persisted store behind the `KeyStore` trait seam while keeping default in-memory
- D) Defer — ephemeral in-memory session keys stand

---

## §3 — Carbon GUIs, UX & Component Library (worklist H3, H4)

### Q35. Where does the missing **Gray 90** middle theme tier go?
*§4 names three switchable grays (10/90/100), but `mde-theme`'s `Theme` enum only has `Dark`/`Light` and `Palette` ships only `dark()` (Gray 100) + `light()` (Gray 10); the workbench hardcodes `Palette::dark()`.*
- A) Add a third `Theme::Gray90` variant + `Palette::gray_90()` — make the enum the canonical 3-theme set the lock describes
- B) Keep two enum variants but add Gray 90 as a density/contrast modifier on the dark palette, not a first-class theme
- C) Treat Gray 90 as dark-mode's `surface` layer only (as now) and amend §4 to ship just Gray 10 + Gray 100

### Q36. How should theme selection actually take effect at runtime?
*The workbench and mde-files `theme()` both hardcode the dark palette — neither reads a persisted theme preference.*
- A) Thread the resolved `Tokens`/`Palette` through `App` state so a theme change repaints live (no restart)
- B) Read the persisted theme once at boot and require an app relaunch to change it
- C) Defer theme entirely to Cosmic's system dark/light setting and drop in-app theme state

### Q37. What replaces the drifted Themes panel (still offering retired presets via gsettings)?
*`panels/themes.rs` offers `chromeos-classic-dark`, `ableton-12-dark` and applies via a `gsettings` shell-out — both retired under strictly-Carbon §4.*
- A) Rewrite the panel to offer exactly Gray 10/90/100, writing to the `mde-theme` preference store (no gsettings, no presets)
- B) Keep the gsettings bridge but collapse options to the three Carbon grays so Cosmic stays source of truth
- C) Delete the Themes panel and route theme choice to Cosmic Appearance settings

### Q38. What happens to `mde-card`'s dead `migration` module (H3)?
*`migration` (`migrate`/`MigrationError`/`SCHEMA_VERSION`, re-exported) has zero refs; `MIGRATIONS` is empty and `migrate()` is a pass-through, while `Card`/`probe` ARE used live in `mackesd/probe_nmap.rs`.*
- A) Remove the `migration` module + `SCHEMA_VERSION`; keep a plain `schema_version` field and lean on the `metadata` bucket for drift
- B) Wire `migrate()` into the LizardFS card-load path in `mackesd` so every card read upgrades on ingest (justifies keeping it)
- C) Keep the module but make it crate-private (drop the `pub use`) until a v2 schema lands

### Q39. What is the fate of `mde-card`'s dead `render_mode::RenderMode` (H3)?
*The 6 render modes describe Portal-era surfaces (Dock breadcrumb, lock widget) that don't exist in the Cosmic build; the live renderer is `object_card` using `CardSize`, not `RenderMode`.*
- A) Delete `RenderMode` outright — `object_card` + `CardSize` cover the live render needs
- B) Wire `RenderMode` into `object_card` as the layout selector (replacing `CardSize`), rendering at least ListRow + Hero in a real surface
- C) Keep only the modes a Cosmic surface needs (CascadeCard, ListRow); delete the four Portal-only variants

### Q40. What replaces `mde-card`'s dead `TemplateSpec` (H3), which models a retired sway workspace-launch?
*`TemplateSpec` + `CardKind::Template` serialize `workspace: i32` + `apps` straight into `sway exec` calls that A1/A2 just deleted.*
- A) Delete `TemplateSpec` + `CardKind::Template` — workspace templates were a sway feature the pivot retired
- B) Re-target `TemplateSpec` to a Cosmic workspace/launch mechanism and wire a real "Save as template" surface
- C) Keep `CardKind::Template` as an opaque saved-app-set (drop the sway `workspace` field) for a future launcher

### Q41. Where does the H4 `motion` module belong under Carbon §4?
*`fade_in_alpha`/`slide_in_offset`/`theme_crossfade`/`SelectionSlider`/`shimmer_alpha` are consumed only by tests + the one `context_menu_surface`; §4 lists "motion" as a token but no live surface drives these.*
- A) Remove the unused public motion helpers; keep only what `context_menu_surface` needs, inlined
- B) Wire a real motion surface (list-item stagger / selection slide in mde-files or the music hub) on a frame subscription, justifying the API
- C) Keep the module behind a `motion` cargo feature, off by default until a surface drives it

### Q42. Do loading skeletons (`skeleton_shimmer`, H4) belong anywhere?
*Every panel has a load-in-flight gap (e.g. workbench `on_panel_navigated` async loads) but none shows a shimmer skeleton.*
- A) Remove `skeleton_shimmer` — the existing empty-state + in-flight text covers loading
- B) Wire skeletons into the highest-latency surfaces (mde-files dir load, music library load), driving `shimmer_alpha` on a tick
- C) Replace it with a single static Carbon "loading" placeholder (no animation) and delete the shimmer math

### Q43. Who renders transient notifications — the dead `toast_chip`, mackesd's FDO host, or Cosmic?
*Two competing paths: the dead in-app `toast_chip` (H4) and mackesd's FDO `org.freedesktop.Notifications` server; §5 says notifications go "via Cosmic."*
- A) Delete `toast_chip` — notifications render through Cosmic's daemon; mackesd only forwards to FDO/Cosmic
- B) Keep `toast_chip` for *in-app* ephemeral feedback (op-complete, copy-done) only, wired into mde-files' operation drawer
- C) Make `toast_chip` the canonical renderer and have the mackesd FDO host draw it (Magic Mesh owns its toasts)

### Q44. What is `elevation_container`'s (H4) role?
*The tiered shadow/radius surface (Inline/PopoverMenu/Floating/Modal) is dead; live surfaces (`object_card`, panel chrome) hand-roll shadows inline.*
- A) Remove it; inline per-surface styling is sufficient
- B) Adopt it as the mandatory elevation primitive — refactor `object_card`, popovers, modals to call it (single-sources Carbon elevation)
- C) Keep it only for the modal/popover tier; delete the Inline/Floating variants

### Q45. Does the `icon_fill_morph` (H4) animation survive?
*The outline→filled Material-Symbols cross-fade is dead; the live card path already does a static outline/filled swap via `IconState` on selection.*
- A) Delete it — the static `IconState` swap is Carbon-correct (Carbon icons don't animate fill)
- B) Wire it into nav/selection icons (workbench sidebar, music hub) on a frame tick so selection animates
- C) Keep only its non-animated `NeverFill`/`AlwaysFill` branches, removing the `OnActive` cross-fade

### Q46. How do we build the §5 cosmic-applet (Bus→Cosmic bridge)?
*No applet crate exists and nothing depends on `libcosmic` (only `mde-files` is forked from cosmic-files); the nearest surface is the workbench "Panel Apps" `panel.toml` editor — a retired-MDE-panel vestige.*
- A) Create a new `mde-cosmic-applet` crate using `libcosmic`'s applet API, subscribing to `mde-bus` and rendering status in the Cosmic panel
- B) Ship the bridge as a plain Cosmic-panel `.desktop`/status-icon driven by mackesd over FDO, no `libcosmic` dependency
- C) Defer the applet — surface mesh status only inside the Workbench window for now

### Q47. What is the cosmic-applet's scope?
*The Workbench is the full control surface; the applet is glue per §6 ("glue, not reimplementation").*
- A) Status-only: a single health pip (mesh up/leader/peers) + click-through that opens the Workbench at the relevant panel
- B) Status + quick actions: pip plus join/leave, DnD toggle, active-transfer count, with deep links into Workbench
- C) Mini control surface: applet hosts peer list + notifications inline, duplicating Workbench panels in the popover

### Q48. How is accessibility reconciled with the strictly-Carbon lock?
*`accessibility.rs` swaps Carbon Blue 60 for a non-Carbon ColorBrewer green (`#4daf4a`) under colorblind mode — a raw non-Carbon hex.*
- A) Drop `colorblind_safe`'s non-Carbon accent; keep only Carbon-native a11y levers (high-contrast Gray 100 + 2px borders + reduce-motion)
- B) Re-pick the colorblind accent from the Carbon ramp (e.g. a Carbon Purple/Teal step) so a11y stays inside §4
- C) Defer to Cosmic's system accessibility settings and remove `mde-theme`'s `A11y` overrides

### Q49. How is reduced-motion actually honored?
*`A11y::reduce_motion` exists but nothing reads it; the motion helpers take a `reduce` arg no live caller passes.*
- A) Wire reduce-motion from the persisted/Cosmic a11y preference into every motion call site once motion is used
- B) Source it from Cosmic's reduced-motion setting via the applet/FDO and thread it into the GUIs
- C) Drop runtime reduced-motion plumbing until a real animated surface ships (paired with removing the `motion` module)

### Q50. What happens to `Density` (Compact/Comfortable/Spacious)?
*`Density` scales spacing tokens, but the workbench `view()` hardcodes `Comfortable` and offers no live switch (the themes panel writes `theme.density`, never read).*
- A) Thread `Density` through `App` state + a Settings control so spacing re-resolves live across panels
- B) Read density once at boot and apply app-wide, requiring relaunch to change
- C) Drop user-facing density — ship one Comfortable density and remove the Compact/Spacious variants

### Q51. Which surfaces are Cosmic-native vs in-app iced in the v1 cutover?
*Workbench + mde-files are iced apps Cosmic decorates; notifications + applet are the contested boundary.*
- A) Iced for Workbench + mde-files only; notifications + applet are Cosmic-native (Cosmic daemon + libcosmic applet)
- B) Magic Mesh draws its own everywhere — keep the in-app FDO notification path + an iced status surface, minimizing Cosmic coupling
- C) Maximize Cosmic-native: also reskin mde-files' chrome to libcosmic and host the panel via Cosmic

### Q52. How are `palette.rs`'s two hex sources reconciled with the new `carbon` ramp?
*`palette.rs` re-types Carbon hex (`0x16,…`) that also live as named steps in `carbon.rs` — two sources, pinned by separate tests.*
- A) Refactor `Palette::dark()/light()` to reference `carbon::GRAY_*`/`BLUE_60` so `carbon.rs` is the sole hex source
- B) Leave palette literals as-is (call-site stability) and rely on pinning tests to keep both in sync
- C) Invert it — derive the `carbon` ramp from the palette, making `palette.rs` the source and `carbon.rs` a view

---

## §4 — Mesh SSH, Remote Access & Services (worklist B1, H6)

### Q53. What product *is* "Mesh SSH" — the core definition of the unbuilt `mesh_ssh` panel (B1)?
*The nav entry exists under Network but lands on the "not ready" catch-all; `remmina_sync.rs` already emits per-peer `.remmina` SSH profiles and `remote_desktop.rs` shells `remmina` for RDP/VNC.*
- A) A per-peer SSH status board + launcher: live peer list with port-22 reachability, click → terminal SSH'd to that peer (the SSH analogue of Remote Desktop)
- B) A known-hosts + mesh-key manager: surface/rotate `~/.ssh/mackes_mesh_ed25519`, distribute pubkeys to peers' `authorized_keys`, manage `known_hosts` trust
- C) An in-app embedded terminal/console to peers (PTY rendered inside the Workbench)
- D) An `sshd_overlay_bind` config surface: read/edit this peer's sshd overlay drop-in + reachability, not a connect tool

### Q54. If Mesh SSH is the per-peer launcher, how should it open the session?
*`remote_desktop.rs` shells `remmina -c rdp://…`; there is no in-app PTY anywhere in the workspace.*
- A) Shell out to the system default terminal (`cosmic-term`/`foot`) running `ssh <peer>` — pure launcher, zero new TTY code
- B) Launch the matching `.remmina` SSH profile that `remmina_sync` already maintains
- C) Embed a real PTY widget inside the Workbench (no external app)

### Q55. Should Mesh SSH be its own panel, or fold into Remote Desktop?
*`remote_desktop.rs` already owns the per-peer RDP/VNC launch surface and reads the same roster.*
- A) Keep a separate `mesh_ssh` panel scoped to SSH only
- B) Merge SSH into `remote_desktop` as a third protocol (one "Remote Access" panel for SSH+RDP+VNC); drop the `mesh_ssh` nav entry
- C) Drop `mesh_ssh` entirely and rely on `remmina_sync`'s auto-generated `.remmina` SSH entries

### Q56. Where should the Mesh SSH surface live in the sidebar?
*The entry sits in `Network` (hidden from the sidebar per E4.15, deep-link reachable only); `remote_desktop` is also under Network.*
- A) Keep it under Network alongside the other `mesh_*` panels and Remote Desktop
- B) Promote it to Devices next to "Connected Devices" / Remote Desktop
- C) Surface it under Fleet as an ops action (SSH into fleet nodes)

### Q57. How should an SSH connect target resolve a peer's address?
*`remmina_sync.rs` targets the bare `hostname` (mesh DNS) and notes a future hostname → Nebula overlay-IP mapping via the lighthouse roster.*
- A) Peer hostname via mesh DNS (today's behavior) — simplest, already works
- B) The peer's Nebula overlay IP from the lighthouse roster (matches where `sshd_overlay_bind` binds)
- C) Operator chooses per-connect (hostname or overlay IP), overlay IP default

### Q58. Should the SSH surface expose any access control, given `sshd_overlay_bind` has none?
*`render_dropin_body` writes `ListenAddress <overlay>` with an explicit "no per-service ACLs here" comment (open-mesh).*
- A) No ACL surface — honor flat-trust; the panel is status/launch only
- B) Add a per-peer allow/deny layer (writes `AllowUsers`/`Match`), breaking flat-trust deliberately
- C) Expose overlay-only vs all-interfaces as the single sshd toggle, no per-peer ACLs

### Q59. Should the panel show this peer's own sshd state (am I reachable over SSH)?
*The worker publishes `TickOutcome` (NoOverlayYet/Idle/Wrote) and the overlay IP is at `/var/lib/mackesd/nebula/overlay-ip`, but nothing surfaces it.*
- A) Show a "this peer's sshd" status row (overlay IP bound, last-written, reload result) over the Bus
- B) Show only remote peers; local sshd state belongs in Mesh Services / health
- C) Show both — a local "you are reachable at <overlay>:22" header above the launch list

### Q60. How should SSH key trust be established between mesh peers?
*`remmina_sync` hardcodes `~/.ssh/mackes_mesh_ed25519` pubkey auth; nothing distributes that pubkey to peers' `authorized_keys`.*
- A) A `mackesd` worker that gossips each peer's mesh pubkey into every peer's `authorized_keys` (auto-trust within the enrolled mesh)
- B) Manual — the panel shows your pubkey + copy button; operator distributes it
- C) Lean on Nebula's transport trust only — TOFU host keys since the overlay is already mutually authenticated

### Q61. How does Mesh SSH compose with `remmina_sync`'s :22/:3389/:5900 probing?
*The worker is the only producer of per-peer SSH entries today, keyed off live TCP probes every 60s.*
- A) Mesh SSH reuses `remmina_sync`'s probe results — no second prober
- B) Mesh SSH runs its own lightweight :22 probe, decoupled from the RDP/VNC sync
- C) Retire `remmina_sync`'s SSH protocol (RDP/VNC only) and make Mesh SSH the sole SSH path

### Q62. What is the minimum shippable scope for B1's first slice?
*B1 must be runtime-reachable and observably work per §7 so it stops landing on "not ready".*
- A) Read-only status: per-peer :22 reachability list + this peer's overlay bind state — no connect action yet
- B) Status + launcher: the reachability list with a working "Open terminal to peer" button
- C) Full: status + launcher + key/known-hosts management in one landing

### Q63. What happens to the mde-music Radio card (H6)?
*`HubCard::Radio` renders but `verb_for(Radio)` returns `None`, there's no `list-radio` verb in `BROWSE_VERBS`, and `airsonic.rs` has no `getInternetRadioStations` method.*
- A) Add an Airsonic `getInternetRadioStations` client method + `list-radio` verb + `verb_for(Radio)` mapping; the card lists internet radio
- B) Repurpose the card to surface something already backed (e.g. Now Playing/queue), keeping a 7-card hub
- C) Drop `HubCard::Radio` and ship a 6-card hub

### Q64. If `list-radio` is added, what does clicking a station do?
*The play flow streams Airsonic song ids through the AIR-5 engine; radio stations are stream URLs, not song ids.*
- A) Enqueue the station's stream URL as a pseudo-track and play it through the existing engine
- B) Hand the URL to an external player (mpv/browser) — radio is out-of-band from the queue
- C) List-only for the first slice (playback deferred)

### Q65. What is the voice/SIP HUD's platform status under Cosmic?
*`mde-voice-hud` ships a working dialer + real PJSIP REGISTER/INVITE/BYE, but the headless `--agent` autostart doc still references labwc.*
- A) Keep + promote: voice is a first-class mesh service — wire the Cosmic autostart for `--agent` and surface presence in the Workbench
- B) Keep as-is (functional) but fix only the stale labwc docs; no new investment
- C) Defer/feature-gate the voice HUD (build it but don't autostart) until a SIP server bench exists

### Q66. How should live SIP registration/presence flow on the mesh?
*VOIP-28 bridges a SIP agent thread to the UI; presence is currently per-host, and the HUD publishes `state/voice/status` to the Bus.*
- A) Bus-native presence: every peer's voice agent publishes `state/voice/status`; the HUD subscribes to build the mesh roster's presence
- B) SIP-server presence (PJSIP SUBSCRIBE/NOTIFY) — presence is the registrar's job, not the Bus
- C) Hybrid: registration via SIP, peer-to-peer presence via the Bus state topics

### Q67. Which remote-access surface is canonical for "reach another machine's files"?
*mde-files browses peer folders (`View::Peer`), LizardFS mesh-home, SMB shares (GVfs), and KDC devices (`cloud:` backend).*
- A) Mesh peers (over the Nebula overlay) is canonical; SMB + KDC are legacy/edge bridges
- B) Keep all three co-equal (mesh, SMB, KDC) — different networks need different bridges
- C) Drop the SMB/Network bridge (retire `smbclient`/GVfs) and keep mesh + KDC only

### Q68. What is the KDE-Connect integration's product scope going forward?
*`crates/kdc/` ships telephony/sms/share/run_command/notification plugins; `discovery.rs` carries the `SyntheticAnnounce`/H8 seam awaiting KDC2-4.*
- A) Full phone hub: land the KDC2-4 mesh-shunt worker so paired phones ride the Nebula overlay; keep all plugins
- B) File-transfer only: keep `share`, drop/park telephony/sms/run_command as out-of-scope
- C) Park KDC: drop the `SyntheticAnnounce` seam (H8) and freeze KDC at local-LAN scope

### Q69. Should phone-side actions (SMS/telephony/run_command) be exposed in the Workbench?
*The plugins exist in `mde-kdc-proto`; the standalone "KDE Connect" Workbench panel was retired (KDC2-5.8) in favor of `mde-peer-card` phone sections.*
- A) Peer-card only — keep the retired-panel decision; phone actions live on the device card
- B) Add a dedicated Workbench phone panel (SMS/call/run-command) back under Devices
- C) Notifications + share only in the GUI; SMS/telephony/run_command stay protocol-level

### Q70. What is the unifying "Services" deployment-role policy (§5: Lighthouse ⊂ Server ⊂ Workstation)?
*mde-music, mde-voice-hud, mde-files, KDC, and sshd_overlay_bind run on different node classes.*
- A) Workstation-only for all user-facing services (music/voice/files/KDC); Servers/Lighthouses run sshd_overlay_bind + plumbing only
- B) Per-service role tags — each service declares its minimum role and the role table gates them individually
- C) All services on all roles, degrading gracefully when hardware/config is absent (no role gating)

---

## §5 — Deployment, Roles & Packaging

### Q71. Which RPM build tooling produces the single Magic Mesh package?
*No `.spec` and no `[package.metadata.generate-rpm]` exist anywhere; §5 calls for "one RPM" but nothing builds it.*
- A) `cargo-generate-rpm` reading `[package.metadata.generate-rpm]` on a top-level packaging crate (pure-cargo, no spec)
- B) A hand-written `.spec` driven by `rpmbuild`/`mock`, invoking `cargo build --release --workspace` in `%build`
- C) `cargo-rpm` (cargo subcommand scaffolding its own template + spec)
- D) `cargo-deb`-style metadata fed through a thin `rpkg`/tito wrapper

### Q72. How are the workspace's 8 binaries split across RPM packages?
*The workspace builds `mackesd`, `mde-bus`, `magic-fleet`, `mde-workbench`, `mde-files`, `mde-music`, `mde-musicd`, `mde-voice-hud`; §5 says "one RPM + install-time role chooser."*
- A) One monolithic `magic-mesh` RPM carrying every binary; the role chooser only decides which units enable
- B) A base `magic-mesh` RPM + role subpackages (`-server`, `-workstation`) pulling tier-specific bins
- C) Per-binary subpackages with a `magic-mesh` metapackage the chooser fills in via weak deps

### Q73. Where does the install-time role chooser actually run?
*`mde-role` writes `/var/lib/mde/role.toml` "chosen once at install time", but no chooser binary, kickstart, or unit exists.*
- A) A kickstart `%post` scriptlet that prompts/parses and calls the pin logic
- B) A systemd first-boot oneshot (`ConditionPathExists=!/var/lib/mde/role.toml`) running a TUI
- C) A standalone TUI binary launched from the live ISO before reboot
- D) A Cosmic first-run GUI app (iced/Carbon) that pins the role on first desktop login

### Q74. What surfaces the role pin to the operator at chooser time?
*`mde-role::pin_at` is library-only with no CLI front-end.*
- A) A new `mde-role` binary subcommand (`mde-role pin <role>` / `choose`) wrapping the lib
- B) A `mackesd role pin <role>` subcommand (extend the daemon CLI that already has `role-workers`)
- C) A dedicated `magic-mesh-setup` binary added to the workspace
- D) The chooser writes `role.toml` directly and never shells through a Rust entrypoint

### Q75. How are the role-gated `mackesd` workers actually started per role?
*`worker_role.rs` gates the workers in-process by reading `role.toml`, but there are zero systemd units in the repo.*
- A) A single `mackesd.service`; the daemon self-gates all workers via `resolve_rank()` (no per-role units)
- B) Per-tier systemd target (`-lighthouse.target` ⊂ `-server.target` ⊂ `-workstation.target`) wanted-by the pinned role
- C) Templated units (`mackes-worker@.service`) enabled/disabled by the chooser from `workers_for_rank()`

### Q76. Which binaries/units does each role install vs merely leave inactive?
*The role is a strict superset; the bins exist but nothing decides whether a Lighthouse even has `mde-workbench` on disk.*
- A) All bins ship on every role; the role only enables/disables units (smallest packaging, largest disk on a Lighthouse)
- B) Role subpackages: Lighthouse gets `mackesd`+`mde-bus`+`magic-fleet`; Server adds meshfs; Workstation adds the Cosmic surfaces
- C) Base bins always; the desktop surfaces are `Recommends:` weak deps the chooser materializes only for Workstation

### Q77. How does a role upgrade reach the box's installed package set?
*`pin_at` enforces upgrade-only at the `role.toml` level but nothing pulls the newly-unlocked bins/units after an upgrade.*
- A) `dnf install magic-mesh-<higher-role>` (subpackage), then re-run the chooser to re-pin and enable new units
- B) A `mackesd role upgrade <role>` verb that re-pins and triggers the unit enable in one step
- C) Upgrade is unit-only: all bins are already present, so re-pinning + a daemon reload is the whole upgrade

### Q78. How is the upgrade-only / refuse-downgrade invariant enforced at the packaging layer?
*The Rust `pin_at` refuses a downgrade, but a `%post` scriptlet could still attempt a lower role.*
- A) The chooser always routes writes through `mde_role::pin`, inheriting the lib-level refusal (single enforcement point)
- B) The RPM scriptlet reads the current `role.toml` rank itself and aborts the transaction on a lower request
- C) Both: scriptlet pre-check for a clean error, plus `pin` as the authoritative refusal

### Q79. How is the signed COPR repository's package signing keyed?
*(Superseded 2026-06-10: distribution moved to a GitHub-hosted dnf repo — Releases asset + gh-pages createrepo tree, signed with the project GPG key. The COPR options below are historical.)*
*§5 requires "a signed COPR"; the repo has no GPG handling, `.repo` file, or COPR config.*
- A) COPR's built-in per-project GPG signing; ship the pubkey + a `magic-mesh-release.rpm` carrying the `.repo` + key
- B) Self-managed detached GPG signing of every RPM before upload, with the pubkey pinned in the repo
- C) Sigstore/`rpm-sequoia` keyless signing tied to the GitHub release identity

### Q80. How does the "Magic-on-Cosmic ISO" get built?
*§5 calls for the ISO; no kickstart, lorax template, or image-builder config exists.*
- A) A Fedora-Cosmic kickstart (`.ks`) installing the COPR + `magic-mesh`, built with `livemedia-creator`
- B) A custom `lorax`/pungi template producing a respun Cosmic spin
- C) `osbuild`/image-builder blueprint pinning the COPR repo
- D) A bespoke script layering the RPM onto an upstream Fedora-Cosmic ISO

### Q81. When does the ISO run the role chooser relative to OS install?
*The boundary between Anaconda install and first boot is unspecified.*
- A) Inline during Anaconda via kickstart `%post` (role decided before first reboot)
- B) Deferred to a systemd first-boot oneshot after reboot (fresh box boots unpinned, then prompts)
- C) On first Cosmic login via the GUI chooser (desktop-first; headless roles use a TUI fallback)

### Q82. How does the DISCLAIMER.md pre-flight gate bind to the build/install?
*`mde-disclaimer` documents an "E8 RPM pre-flight gate" (DISCLAIMER.md non-empty before build), but nothing enforces it; `/release` references it too.*
- A) Build-time gate only: the RPM build fails (release script + `build.rs`) if `is_present()` is false — no runtime prompt
- B) Install-time accept gate: a `%pretrans`/chooser screen shows `mde_disclaimer::TEXT` and requires acceptance before pinning the role
- C) Both: build refuses to package without the text, and the chooser shows it as a mandatory pre-flight screen

### Q83. Where does Nebula enrollment happen in the install flow?
*`nebula_enroll.rs` implements `mesh:<id>@<ip>:<port>#<bearer>` via `mackesd enroll --token`, but nothing ties enrollment to install.*
- A) Defer to post-install: the box boots unenrolled; the operator runs `mackesd enroll --token` later
- B) The chooser prompts for a join token at install and shells `mackesd enroll --token` as the final step
- C) A first-boot enrollment unit watches for a token dropped via kickstart/cloud-init and enrolls headlessly

### Q84. How is a Lighthouse (CA root) bootstrapped vs a joining peer at install?
*Enrollment is asymmetric (peers CSR, lighthouse signs), but the chooser has no notion of "first node" vs "joining node."*
- A) The Lighthouse role install runs CA init automatically; Server/Workstation always expect a join token
- B) A separate "initialize new mesh" vs "join existing mesh" prompt, orthogonal to the role choice
- C) Always join-by-token; a brand-new mesh is bootstrapped manually post-install via `mackesd ca`

### Q85. Where does the packaging metadata physically live in the tree?
*Only `install-helpers/lint-mesh-boundary.sh` exists; there is no `packaging/`, `dist/`, or `units/` dir.*
- A) A new top-level `packaging/` dir (spec/metadata, units, `.ks`, `.repo`) — non-crate, tracked separately
- B) Co-located per crate (each ships its own units + `[package.metadata]`), assembled at build time
- C) A dedicated `magic-mesh-packaging` workspace crate owning the metadata + generated artifacts

### Q86. How does the RPM map roles to the systemd units it enables at install?
*`workers_for_rank()` is the authoritative role→worker mapping in Rust; `%post` `systemctl enable` lists would duplicate it and risk drift.*
- A) The chooser asks `mackesd`/`mde-role` for the unit list (`workers_for_rank`) and enables exactly those — single source of truth
- B) Static per-role `.preset` files shipped in the RPM, hand-maintained alongside the worker table
- C) One `mackesd.service` enabled unconditionally; the daemon self-gates, so the RPM enables nothing role-specific

---

## §6 — Testing/CI, Observability & Lifecycle (worklist E1)

### Q87. What should the Nebula integration harness exercise, and against what (E1)?
*`tests/integration_testcontainers.rs` spins real `headscale` + `tailscale` containers (the §1-retired substrate); E1 says retarget to Nebula or delete.*
- A) Stand up a real `nebula-lighthouse` + 2 `nebula` peer containers (testcontainers) and assert overlay reachability + cert handshake end-to-end
- B) Run the real `mackesd` binary against a lightweight mock lighthouse (Rust fixture serving gossip/relay) — no real Nebula process
- C) Keep only binary-level CLI/store/leader assertions (enroll, migrate, reconcile, leader-lock); drop overlay-transport testing
- D) Delete the Tailscale tests outright — no container integration tests; rely on unit/in-process tests only

### Q88. How should the Nebula container fixture be defined, if we keep container tests?
*Today the harness uses the `testcontainers` crate behind a `docker-tests` feature.*
- A) Stay on the `testcontainers` Rust crate (programmatic `GenericImage`), just swap to Nebula images
- B) Move to a checked-in `compose.yaml` (podman-compose) the test shells out to, so operators can run the same topology by hand
- C) Define the fixture as a Nix flake/NixOS test for hermetic reproducibility
- D) Build the Nebula overlay in-process from the existing Rust CA crate (no containers) — a pure-Rust loopback overlay

### Q89. Should container-dependent integration tests run in CI at all?
*Every test calls `skip_if_no_docker!()` and reports **passed** when no daemon is reachable — a silent skip that masks breakage.*
- A) Run them on a daemon-equipped runner and treat a daemon-absent skip as a hard failure (no silent pass)
- B) Keep container tests developer-local only, never in CI; CI runs the Docker-free suite exclusively
- C) Run them in CI as a non-gating, allowed-to-fail informational job
- D) Run them only on a nightly/scheduled CI job, not per-PR

### Q90. Which CI provider/runner should Magic Mesh standardize on?
*There is no `.github/workflows` or any CI config in the repo today.*
- A) GitHub Actions on GitHub-hosted runners
- B) GitHub Actions on a self-hosted Fedora-Cosmic runner (matches the target substrate + podman + a display for `/preview`)
- C) GitLab CI / Forgejo Actions on self-hosted infrastructure (off GitHub)
- D) A local-first `just ci` / `cargo-make` script the COPR build invokes — no hosted CI service

### Q91. How do we integration-test the no-fixed-center gossip + reconcile loops?
*The loops are file-driven over QNM-Shared (newest-pointer / leader-lock); unit tests drive `tick_once` against a tempdir today.*
- A) A multi-process harness: spawn N real `mackesd` binaries sharing one tempdir QNM root and assert convergence (newest wins, one leader)
- B) A single-process simulated fleet: instantiate N worker structs against one temp root and step their ticks with an injected clock
- C) A property/model-based test over the pure decision fns (`latest_aggregator`, `barrier_should_fire`, …) — no process orchestration
- D) Keep it at the current per-worker `tick_once` unit level; add no cross-worker convergence test

### Q92. What is the visual-regression testing policy for the Carbon GUIs?
*`/preview` launches the real binary to eyeball Gray 10/90/100; §7 requires visual changes confirmed against the Carbon reference; D1 deferred a preview pass (headless env).*
- A) Golden-image snapshot tests in CI (render to PNG on a headless wgpu/Xvfb runner, diff against baselines, fail on pixel delta)
- B) A scripted `/preview` capture posting screenshots as CI artifacts for human review — no automated pixel gate
- C) Token-level assertion tests only (widgets resolve to `mde-theme` Carbon tokens) — no rendered-pixel testing
- D) Manual `/preview` on the operator's workstation before release, no CI involvement

### Q93. What coverage/gating policy should the CI quality bar enforce?
*§7 already demands build clean, tests green, clippy + fmt clean; there's no coverage tooling today.*
- A) Hard line-coverage floor (`cargo-llvm-cov` ≥ 80%) that fails the build below threshold
- B) Coverage measured + reported (tracked over time, PR comment) but never a merge gate
- C) No coverage metric; gate on green tests + clippy `-D warnings` + fmt + the existing lint gates (boundary, Carbon-token, Nebula-substrate)
- D) Gate on a mutation-testing score (cargo-mutants) for the pure decision fns instead of line coverage

### Q94. What logging/tracing backend should `mackesd` workers emit to as the default?
*Workers use `tracing` with structured fields and fail-soft levels; there's no stated sink/format lock.*
- A) `tracing-subscriber` to stdout/journald as structured JSON, harvested by journald
- B) Human-readable `tracing` to journald only; structured fields via `journalctl -o json` on demand
- C) Ship spans to an OpenTelemetry/OTLP collector on the aggregator peer (distributed tracing)
- D) Mesh-replicate a per-peer structured log file into QNM-Shared so any peer can read any peer's recent trace

### Q95. How should fleet-wide health/metrics be aggregated under no-fixed-center?
*`netdata_aggregator` leader-elects one peer and rewrites every peer's Netdata `[stream]` to point at it; fail-soft to local-only.*
- A) Keep the single leader-elected Netdata aggregator (current) — one parent, children stream, falls back to local
- B) Full mesh: every peer scrapes every peer (no aggregator role), the Workbench unions all endpoints
- C) Replace Netdata streaming with pull-based Prometheus + per-peer exporter, federated at query time
- D) No central aggregation — each peer is local-only; the operator opens whichever peer they're investigating

### Q96. How should an operator observe whole-fleet state with no fixed center to query?
*Peer health is reconciled from QNM-Shared heartbeats into `nodes.health` and surfaced via `NebulaSignal::PeerStateChanged`; no single "fleet status" endpoint.*
- A) The Workbench Overview is canonical — any peer's `mackesd` projects the full roster from its local SQLite mirror
- B) A `mackesd fleet status` CLI any peer can run, printing the union (heartbeats + versions + leader) for headless boxes
- C) A dedicated Mesh Health GUI panel aggregating Netdata + alerts + reachability
- D) Lean on Netdata's own dashboard as the fleet view; `mackesd` owns only per-peer reachability signals

### Q97. How should the fleet upgrade flow be observed and gated while it runs?
*`upgrade_intent_watcher` drives `dnf upgrade` → quorum/grace barrier → `mde-install` autonomously, +4h grace; progress lives only in the intent JSON's ack maps.*
- A) Surface live upgrade progress in the Workbench (per-peer ready/failed/complete + barrier countdown) — observe-only, no manual gate
- B) Add an operator hold/approve gate: the barrier won't fire until an operator confirms, even after quorum+grace
- C) Emit each upgrade-state transition as a desktop alert (reuse `alert_relay`) so operators are pushed progress
- D) Keep it fully autonomous and silent (current); operators inspect `journalctl` / the intent JSON only on a stall

### Q98. What is the backup scope and cadence for `nebula_ca_backup`?
*It seals the Nebula CA + a LizardFS topology snapshot into `state-backup.enc` per peer in QNM-Shared every 24h, passphrase-gated, lighthouse-only.*
- A) Keep current scope (CA + topology snapshot), 24h, lighthouse-only, opt-in via passphrase
- B) Expand to full mackesd state (SQLite + desired-config revisions + CA + topology) so a peer reconstitutes from one bundle
- C) Make it event-driven (re-seal on every CA mint / topology change) instead of a 24h tick
- D) Narrow to CA-only and track the LizardFS snapshot as a separate, independently-restorable artifact

### Q99. Where should sealed backups live so the fleet can restore after losing the lighthouse?
*Backups are written to `QNM-Shared/<self>/mackesd/state-backup.enc` — onto the same LizardFS storage whose topology they snapshot.*
- A) Keep them in QNM-Shared and rely on LizardFS replication — every peer holds a copy, any can restore
- B) Additionally push each sealed bundle to off-mesh operator-held storage (USB/object store)
- C) Have every peer seal+publish its own bundle (not just the lighthouse) so the freshest CA backup is never single-homed
- D) Keep QNM-Shared primary but add a periodic operator-runbook export (`mackesd ca export`) as the durable copy

### Q100. How should alert urgency map to Cosmic notifications, and what is the delivery contract?
*`alert_relay` polls `~/.local/share/mde/alerts/*.json` every 2s and shells `notify-send --urgency`; best-effort, silently degrades when headless.*
- A) Keep `notify-send` + the three-level urgency map, best-effort, headless-tolerant (current)
- B) Deliver via the `mde-bus` → cosmic-applet path (FDO host) instead of `notify-send`, so it works without `notify-send` on PATH
- C) Add a guaranteed-delivery tier for `critical`: persist + re-fire until acknowledged; keep best-effort for warn/info
- D) Route alerts fleet-wide — any peer's critical alert notifies every operator desktop, not just the host that fired it

---

*100 questions · authored 2026-06-09 · grounded against the live repo. Answer with `Q#: <letter>`;
each lock becomes a `docs/design/<epic>.md` entry and a `docs/WORKLIST.md` user-story task.*
