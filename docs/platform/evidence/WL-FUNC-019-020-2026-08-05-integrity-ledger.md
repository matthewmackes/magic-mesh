# WL-FUNC-018 / WL-FUNC-019 / WL-FUNC-020 integrity and ledger release slice

This dated evidence records the next bounded implementation slice for the
canonical platform worklist. It is evidence, not a second tracker, and it does
not close any epic.

## WL-FUNC-019 — catalog integrity attestation

`ResourceCatalog` now carries an optional, backward-compatible
`catalog:v1:<sha256>` content digest. The canonical input is independent of
card arrival order and excludes the digest field itself. Validation rejects a
malformed or mismatched supplied digest, while legacy catalogs without a digest
continue to decode. Catalog construction and resource-mirror publication fill
the digest deterministically before publication.

The catalog also has a detached HMAC-SHA256 publisher attestation bound to the
exact catalog digest, publisher, key ID, and freshness window. The focused
publisher proof passed 2/2 on farm `.50`. `service_aggregator` now reads the
approved `resource/publisher-hmac` secret, emits a publisher-scoped retained
proof topic, and withholds only that proof when the key is absent or invalid;
the focused publication suite passed 16/16 on BigBoy. Production key
provisioning and authenticated consumer enforcement remain open.

Farm evidence:

- mesh resource types: 30/30 passed;
- publisher attestation: 2/2 passed on `.50`;
- `service_catalog`: 6/6 passed;
- `service_aggregator`: 16/16 passed on BigBoy `.130`, slot
  `wl-crit-006-cloud-bigboy-r3`;
- reachable shell resource consumer: 2/2 passed on BigBoy `.130`, slot
  `wl-func-019-resource-consumer-r2`;
- exact scoped rustfmt with `skip_children=true` and `git diff --check` passed;
- the disposable catalog farm slots were removed after verification.

The follow-on trusted-LAN admission gate now revalidates the public
advertisement shape, requires caller-supplied `ObservedLan` trust and interface
scope, bounds freshness to 10 minutes, and emits `DiscoverySource::SsdpUpnp`
with `ResourceScope::TrustedLan`. Reachable evidence is preserved, unknown
reachability remains unknown, and explicitly unreachable evidence is rejected.
The gate is pure: it performs no `rupnp` I/O, socket, scan, retry, URL fetch,
or roster mapping. The focused `.90` farm gate passed 38/38 tests. Runtime
`rupnp` integration and reviewed interface policy remain open.

## WL-FUNC-020 — bounded Android inventory ledger

`AndroidInventoryLedger` is a pure bounded admission seam for correlated guest
inventory responses. It validates the existing strict request/response
boundary, retains at most 32 stable workload identities, treats duplicate
observations idempotently, accepts newer observations, and rejects replayed
older observations without contacting a provider, ADB, socket, or Cuttlefish
guest. Its snapshot is deterministic by workload ID.

Farm evidence:

- the Android ledger module gate passed 17/17 on `.90` (six new ledger tests;
  4,345 unrelated tests filtered);
- the BigBoy cloud-worker gate passed 223/223, including pending-to-observed
  Workloads publication replacement and replay/no-rollback proof;
- exact scoped rustfmt and `git diff --check` passed;
- the disposable Android ledger slot was removed after verification.

The pure ledger is now retained by `CloudWorker` and admitted responses replace
the matching pending Android VM inventory in `CloudState` publication by stable
workload ID. Pending remains the honest fallback; invalid, mismatched, older,
and capacity-rejected responses do not alter the retained snapshot. The
CloudWorker additionally persists admitted replacements to a host-scoped,
bounded JSON ledger through a synced temporary file and atomic rename, then
reloads it on restart. This durability covers retained observations only; no
real guest provider, ADB, socket, or Cuttlefish adapter is claimed.

## WL-FUNC-020 — bounded stale-evidence projection

The CloudWorker CloudState projection now advances an admitted inventory's
reported age from its immutable observation timestamp. Once the bounded
30-day observation window is crossed, the projection caps the displayed age,
marks the guest `Unavailable` with `ObservationStale`, and changes installed
entries to unavailable/non-launchable with the same typed reason. Pending,
missing-package, and image-unavailable facts are not rewritten into a false
success or a command-shaped diagnostic. The projection performs no provider,
ADB, socket, Cuttlefish, or network I/O.

Farm evidence:

- CloudWorker focused gate: 225/225 passed on BigBoy `.130`, slot
  `wl-func-020-stale-bigboy-r1`, including fresh-age and stale/non-launchable
  projection tests;
- the exact disposable BigBoy slot was removed after the run;
- worklist self-test/lint, doc-supersession lint, browser-helper self-test, and
  `git diff --check` passed.

## WL-FUNC-020 — CloudWorker provider fold and durable restart

`CloudWorker` now polls registered stable-ID Android providers on a bounded
30-second cadence and on relevant drift ticks. It sends only the typed
`AndroidGuestRequest::Inventory` request, accepts only a correlated typed
inventory response, and folds the result over the pending Android VM rows in
`CloudState`. A missing registration stays pending; a provider error, wrong
response, replay, invalid snapshot, or persistence failure remains an honest
non-success and cannot publish uncommitted retained state. A restarted worker
reloads the host-scoped ledger and restores the retained ready inventory with
its immutable observation timestamp; projection applies age and stale
non-launchability without mutating the observation.

Farm evidence:

- CloudWorker suite: 231/231 passed on BigBoy `.130`, slot
  `wl-crit-006-cloud-bigboy-r3`;
- Android provider/ledger module: 22/22 passed on `.50`, slot
  `wl-func-020-android-durable-r5`;
- no live Cuttlefish provider or guest-session proof is implied by these
  process/farm tests.

## WL-FUNC-020 — Workloads Android card projection

The shell Android projection now folds outer Workloads rows with the admitted
inventory mirror by stable workload identity. It renders a typed pending,
stale, unavailable, or observed state, preserves the closed unavailable reason,
derives retry eligibility without emitting a retry command, and labels evidence
inspection as read-only. Missing or malformed inventory remains pending or is
dropped; no launch, ADB, shell, or arbitrary-command action is introduced.

Farm evidence:

- `mde-shell-egui` Android projection: 9/9 passed on BigBoy `.130`, slot
  `wl-func-020-android-ui-r2`;
- the compile included the current workspace and existing warnings only;
- no live seat, VM, or network state was changed.

## WL-FUNC-020 — immutable Android image/package manifest

The Android contract now admits a strict v1 image manifest and a separate
image-package manifest. The package manifest binds immutable image/source/
catalog provenance to the canonical nine AOSP starter identities, package IDs,
and bounded package versions in deterministic order. It has no installed,
readiness, launcher, command, ADB, or transport fields, so image contents cannot
be mistaken for a guest-owned inventory observation. Unknown, omitted,
duplicate, reordered, mismatched, malformed, and future-timestamped records fail
closed. `packaging/android/verify-manifest.sh` repeats the exact shape and
hostile-input gate without claiming a booted Cuttlefish guest.

Farm evidence:

- Android image/provenance contract: 7/7 targeted tests on `.170`, slot
  `wl-func-020-image-provenance-r2`;
- packaging manifest verifier self-test passed on `.90`, slot
  `wl-func-020-image-packaging-r1`;
- no live Cuttlefish image, package-manager inventory, or installed-state proof
  is implied.

## WL-FUNC-019 / WL-FUNC-020 — publication and workload-provider slice

The SSDP publication record now has a second, use-time validation boundary.
Directly constructed records cannot bypass advertisement grammar, trusted-LAN
provenance, source identity binding, explicit reachability, interface scope,
bounded TTL, or expiry. The gate remains pure and does not add `rupnp`, socket,
scan, retry, URL, or roster I/O.

The Android provider seam is now a bounded process-local registry keyed by the
validated stable Android VM workload ID. The audited action worker dispatches
through that registry; an absent registration still returns pending inventory or
explicitly unavailable launch. Registered adapters receive only the admitted
closed Android request, and response correlation remains enforced. This is an
integration seam, not a live guest/provider claim.

Farm evidence:

- Android provider/ledger module: 22/22 passed on `.50`, slot
  `wl-func-020-android-durable-r5`;
- audited action worker: 35/35 passed on `.90`, slot
  `wl-func-020-provider-action-r2`;
- desktop-source module: 39/39 passed on `.170`, slot
  `wl-func-019-ssdp-revalidate-r4`;
- typed trusted-LAN SSDP adapter: 15/15 passed on `.170`, slot
  `wl-func-019-ssdp-adapter-r3`, including bounded header admission,
  interface policy, use-time expiry, protocol folding, and card projection;
- completed disposable slots were reclaimed only after the farm process-CWD
  guard found no live job; no live seat or service was touched.

The resource publication path now derives a short-lived proof from the
approved secret store without serializing key material. The catalog remains
the backward-compatible retained body; a consumer derives the
`state/resources/publisher-attestation/<publisher>` topic from its validated
publisher identity and verifies the exact catalog digest, key ID, and freshness
window before treating the catalog as authenticated.

## WL-FUNC-019 — authenticated shell consumer boundary

The reachable shell chooser now reads the retained catalog and its typed
discovery projection, re-derives the projection from the catalog, and rejects a
cross-topic mismatch. Production `ChooserState::default` injects the exact
`resource-publisher-hmac` systemd credential into `with_publisher_key`; the
credential is provisioned from the approved `mackesd secret get
resource/publisher-hmac` API and never comes from arbitrary command input or a
logged value. When present, the key promotes only a proof with the exact
publisher, catalog digest, current key ID, validity window, and verified HMAC.
A missing, stale, malformed, or wrong-key proof leaves the validated catalog
available for inspection but disables service actions and sealed configuration.

Farm evidence:

- resource contract publisher-attestation tests: 2/2 passed on `.50`, slot
  `wl-func-019-resource-contract-r3`;
- shell loader + consumer admission: 5/5 passed on BigBoy `.130`, slot
  `wl-func-019-resource-wiring-r2`;
- provisioning helper `bash -n` + self-test passed locally;
- no live seat, service, network, or credential state was changed.

## WL-CRIT-006 — production publisher-attestation evidence

`install-helpers/release-evidence.sh` now emits schema 5. A production verdict
requires a publisher-attestation descriptor with the closed key ID, catalog
digest, bounded issued/expiry window, and signature shape; the descriptor's
path, size, and SHA-256 are included in the evidence binding. Preview evidence
may omit it, and legacy schema-4 evidence remains accepted only when it is not
promoted. The helper self-test passes, including missing, malformed, mismatched,
legacy-production, and valid-production cases. The shell now owns the narrow
runtime consumer/key-store handoff; the remaining production boundary is
installing the encrypted credential on each workstation and wiring every other
resource consumer to an approved key source.

## WL-FUNC-018 — admitted App VM session handoff

The App VM request contract now separates bounded wire validation from the
stronger admission check used at a launch/session boundary. Admission requires
a reverse-DNS Flatpak identity, the supported guest profile, and only the
closed capability policy. `mackesd` applies that check before an `OpenApp`
session enters its roster.

The App VM packaging lane now binds a non-null source revision and complete
base-image digest into both immutable image labels and guest-readable
provenance. A strict readiness manifest names the guest-owned Sway executable,
supervisor, readiness topic/state, and disabled host fallback; missing,
duplicate, unknown, ambiguous, or mismatched evidence fails closed before an
image artifact is accepted. Static contract and image self-tests pass locally,
and the focused farm readiness lane passed on `.90`, slot
`wl-func-018-image-readiness-r3`.

The reachable session rail now reconstructs and retains the complete typed
`AppVmLaunchRequest` from each `OpenApp` record. An unadmitted identity or
host-facing capability is dropped before it can appear as a focus target or
produce an `AppSessionHandoff`; the VDI path consumes the request's own session
and app identities instead of re-deriving either from presentation text.

Farm evidence:

- shared request admission: 3/3 on `.50`, slot
  `wl-func-018-handoff-types-r1`;
- reachable session rail: 10/10 on `.90`, slot
  `wl-func-018-handoff-rail-r1`;
- reachable VDI handoff: 1/1 on `.170`, slot
  `wl-func-018-handoff-vdi-r1`;
- daemon App VM admission: 1/1 passed on `.170`, slot
  `wl-func-018-app-broker-r4`, for the exact
  `apply_request_rejects_unadmitted_app_identity_and_capability` filter;
- `git diff --check` passed. No live guest, VM, seat, network, or T480 state was
  touched.

## WL-FUNC-019 — typed SSH/X11 admission seam

The mesh type boundary now keeps SFTP browsing, SSH-forwarded X11 applications,
and full remote X11 desktops as three separate closed resource forms. Host,
user, port, opaque secret reference, safe SFTP segments, numeric display, and
DRM-seat availability are bounded and validated; commands, URLs, raw paths,
private-key material, arbitrary `DISPLAY` strings, and open-ended protocol or
auth values are rejected. A DRM seat without an X server remains valid evidence
but is unavailable for X11 launch.

Farm evidence:

- SSH/X11 focused admission and hostile-input tests: 9/9 on `.50`;
- SSH/X11 resource-card adapter and retained-roster consumer: 4/4 on `.170`,
  slot `wl-func-019-ssh-card-r1`;
- current full `mackes-mesh-types` crate gate: 404/404 on BigBoy `.130`, slot
  `wl-func-019-020-types-r2`;
- clippy remains affected by unrelated pre-existing lint errors; the retained
  catalog consumer is wired, but no live `russh`/`x11rb` session or X11 proof is
  implied;
- no seat, network, worklist, or SSDP files were changed by this seam.

## WL-FUNC-019 — bounded Subsonic/OpenSubsonic admission

The shared mesh-types seam now distinguishes Navidrome, Airsonic-compatible,
and generic Subsonic-compatible providers, with opaque endpoint and secret-store
references, closed capability/action sets, and explicit launchable versus
unavailable states. It is an admission contract only: no URL, credential value,
provider HTTP call, or arbitrary command is carried by the card.

Farm evidence:

- focused typed Subsonic tests: 7/7 on `.170`, slot
  `wl-func-011-019-subsonic-final-r1`;
- current full `mackes-mesh-types` gate: 404/404 on BigBoy `.130`, slot
  `wl-func-019-020-types-r2` (including the Subsonic and Cuttlefish contracts);
- native OpenSubsonic HTTP/client integration, live provider discovery, and
  playback proof remain open.

## WL-FUNC-020 — Workloads Android launch gate

The reachable Android Workloads projection now exposes launch readiness only
when the inventory itself validates, is workload-scoped, has a ready guest and
image provenance, is within the strict observation-age bound, and the selected
entry is installed, launcher-resolved, and launch-ready. Pending, stale,
unavailable, malformed, mismatched, or unscoped entries remain disabled; no
arbitrary intent, ADB, command, or host-app fallback is introduced.

Farm evidence:

- exact Android UI launch-gate filter: 2/2 passed on `.170`, slot
  `wl-func-020-android-launch-gate-r2`;
- the earlier interrupted BigBoy lane is not counted; no live guest, provider,
  seat, network, or deployment state was touched.

## WL-FUNC-020 — bounded Cuttlefish provider contract

The shared provider seam now binds a stable Android VM identity to immutable
image provenance, guest boot/readiness evidence, lifecycle state, and
generation-checked provision/start/stop/reboot/destroy operations. Invalid
image drift, stale generations, false readiness, unsupported transitions, and
unknown wire fields fail closed before provider contact.

Farm evidence:

- seven focused provider tests and the full `mackes-mesh-types` gate passed on
  BigBoy `.130`, slot `wl-func-020-cuttlefish-provider-final-r1`: 404/404;
- no Cuttlefish guest, nested-KVM lifecycle, image build, ADB, seat, or network
  mutation occurred; the real provider and live package/frame proof remain open.

## Release boundary

No seat, VM, image, guest, audio, network, firewall, provider, ADB, socket, or
Cuttlefish mutation occurred in this slice. T480 remains a fixed-wired bench
seat. Browser VM acceptance is still below the 27 FPS floor; production key
provisioning/wiring, live `rupnp` discovery, live Cuttlefish provider/image
proof, live App VM guest/VDI handoff, and per-app launch proof remain open.
