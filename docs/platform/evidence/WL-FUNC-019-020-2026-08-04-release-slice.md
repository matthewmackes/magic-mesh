# WL-FUNC-019 / WL-FUNC-020 bounded release slice

This dated evidence records one implementation slice for the canonical
platform worklist. It is not a second tracker and does not close either epic.

## WL-FUNC-019 — desktop sources enter the universal catalog

The retained `state/desktops/sources` row is now consumed by the universal
resource catalog path:

- a missing retained row is an honest pre-discovery state and adds no desktop
  cards;
- the retained wire is bounded and strict (`deny_unknown_fields`), with
  invalid rows failing closed for both resource mirrors while the service-state
  mirror still publishes;
- valid desktop sources are projected through the existing typed
  `ResourceCard` adapter, deduplicated by stable resource identity, and
  validated before catalog/discovery publication.

Farm evidence:

- `172.20.0.170`, slot `wl-func-021-catalog`: 13/13
  `service_aggregator` tests and 5/5 `service_catalog` tests passed;
- the scoped farm rustfmt check and `git diff --check` passed;
- prior desktop adapter evidence remains 28/28 on BigBoy, and the capability
  registry gate remains 10/10 on `.170`.

The next discovery seam is intentionally pure and publication-gated. It accepts
only the closed MCNF RDP/VNC/Spice service vocabulary, bounded identity/host/port
fields, and explicit caller trust/reachability. It rejects `LOCATION`/URL,
command/path-shaped values, conflicting duplicates, malformed headers, and
unsupported services. It performs no socket, scan, retry, or launch operation.
The focused desktop-source farm gate passed 34/34 on `.90`.

## WL-FUNC-020 — Workloads families and Android evidence

The App VM Workloads view now presents three separate, typed family summaries:

- `browser-vm` — dedicated Chromium VM lifecycle and session gate;
- `android_vm` — governed AOSP starter applications with explicit inventory
  pending state;
- `app_vm` — guest-owned Flatpak applications with admitted catalog/session
  gating.

The family index is display-only and does not invent actions or readiness. The
reachable Workloads Run table now wires `Open app` only for an App VM row with a
valid typed `AppVmLaunchRequest` and reverse-DNS Flatpak identity; Plan rows and
malformed or absent declarations stay disabled, with no host fallback. The
focused BigBoy gate passed 1/1 for this action boundary; the earlier family
projection gate passed 5/5.

The Android guest inventory is now schema v2 and keyed by stable VM identity.
It carries bounded image provenance, package version, launcher resolvability,
guest boot state, observation age, and closed unavailable reasons. Unknown
fields, duplicate apps, malformed/oversized evidence, and command-shaped
payloads remain rejected.

The optional `CloudState.android_inventories` mirror is backward-compatible and
strictly validates each record. Until a real guest provider is wired, the cloud
worker emits one explicit pending record per admitted Android VM workload and
none for other delivery types. Workloads matches records by stable workload ID,
selects duplicates deterministically, renders package/provenance/boot/age/reason
evidence, and keeps every launcher disabled.

Farm evidence:

- `.50`, Android contract slot: 20/20 mesh Android tests and 18/18 mackesd
  Android-provider tests passed;
- `.90`, current focused provider gate: 11/11 Android provider tests passed;
- `.170`, pending Android mirror gate: 1/1; `.50`, CloudState contract gate:
  37/37; BigBoy, Workloads projection gate: 8/8;
- exact-file rustfmt and `git diff --check` passed.

## Release boundary

This slice made no live seat, VM, image, guest, audio, network, or firewall
mutation. T480 remains the fixed wired bench host; its separate Browser VM
acceptance evidence is still rejected below the 27 FPS floor and is recorded
in `WL-ARCH-008-2026-08-04-t480-r13-acceptance.md`. Live Android inventory,
Cuttlefish/image proof, session handoff, and per-app launch proof remain open.
