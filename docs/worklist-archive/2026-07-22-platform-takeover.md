# Platform takeover closures — 2026-07-22

> **NOT AN ACTIVE TRACKER.** The active platform worklist is
> `docs/platform/WORKLIST.md`.

### WL-DOC-004 - Reconcile durable governance with the zero-OpenStack architecture

- Disposition: **DONE 2026-07-22.** The highest repository authority now locks
  provider-neutral Workloads over OpenTofu, Ansible, local libvirt/KVM,
  NetworkManager/nmstate, Podman/Quadlet, bootc/osbuild, etcd, and mde-seal, and
  explicitly forbids restoring the retired control plane
  (`AI_GOVERNANCE.md:69,145,255,278`). Current README, architecture, and Workloads
  help agree; superseded OpenStack design/runbook documents carry top banners.
- Verification: focused current-doc terminology search found no production
  OpenStack mandate; brand, doc-supersession self-test and real-tree lint, the
  combined policy suite, and `git diff --check` passed. The Workloads help names
  the remaining pre-release correctness work honestly
  (`docs/help/cloud-self-service.md:71,78`).
- Priority: P1
- Complexity: Small
- Origin or merged source IDs: corrective follow-up to WL-ARCH-001;
  2026-07-22 Codex platform takeover governance audit.

### WL-FUNC-013 - Maps & Location world-class and built for purpose

- Disposition: **DONE 2026-07-22.** Sparse-data honesty, the one-tap
  Construct/Car toggle, offline FTS5 address search, Web-Mercator MBTiles
  basemap, progressive navigation, and floating actions landed in `8b91003c`,
  `4bc193d7`, `a49818ba`, and `08086639`. Current source retains the real raster
  and geocoder modules (`crates/desktop/mde-maps-location-egui/src/basemap.rs:1`,
  `geocode.rs:1`) and the direct profile toggle
  (`crates/desktop/mde-shell-egui/src/main.rs:313,2536`).
- Verification: focused maps farm suite passed 95/95 during the takeover. The
  existing live `.15` proof installed the F44 DRM shell with zero restarts and
  queried the deployed East-Texas bundle: 3,396 raster tiles and 10,654
  gazetteer rows, including Athens/Lake Athens results. The operator explicitly
  removed visual signoff as a gate; later Valhalla routing was never part of the
  required outcome and remains a future enhancement, not hidden unfinished work.
- Priority: P1
- Complexity: Epic
- Origin or merged source IDs: operator world-class Maps goal and 2026-07-22
  Advanced/floating-actions/solve-all directives.

### WL-RUN-008 - Carry real traffic over the HTTPS fallback transport

- Disposition: **DONE 2026-07-22.** The router now owns a framed bidirectional
  TLS 1.3 fallback stream, forwards bounded mesh payloads in both directions,
  clears Active on loss, reconnects, and bridges production Nebula UDP only
  through exact expected local source addresses. Relay identity is signed by an
  enrollment-distributed authority pinned outside mutable replicated state;
  the network-enrollment steady-state writer redacts private CA and relay-authority
  material. Handshakes and sessions are bounded, and the new relay-authority
  persistence introduced by this slice is atomic, durable, and explicitly mode
  `0600`. The takeover's subsequent whole-enrollment audit found older file-bundle,
  endpoint-key, and Nebula subprocess persistence that does not meet that invariant;
  those broader pre-existing paths are active under WL-SEC-006 rather than being
  misrepresented as part of this transport closure.
- Verification: isolated BigBoy farm proof passed `mackes-transport` 56/56,
  HTTPS tunnel 49/49, HTTPS transport 18/18, mesh router 26/26, bundle 13/13,
  enroll client 5/5, supervisor 30/30, listener 6/6, seal 5/5, node key 2/2,
  and mesh init 2/2. The forced-fallback fixture exercised UDP -> TLS ->
  production-lighthouse demux, bidirectional payloads, two sessions, reconnect,
  wrong-certificate rejection, and forged UDP-source rejection on both sides.
  An integrated shared-tree `cargo check -p mackesd --all-targets` also passed;
  `git diff --check` was clean.
- Priority: P1
- Complexity: Large
- Origin or merged source IDs: 2026-07-22 Codex platform takeover runtime audit.
