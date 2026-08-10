# WL-UX-011 Surface Pro 5/6 first-class runtime checkpoint (r16)

Date: 2026-08-10

## Result

The Surface runtime no longer depends on private daemon/shell wire mirrors or
placeholder reboot, firmware, camera, and fleet paths. Microsoft Surface Pro 6
remains the canonical `Surface` seat. Surface Pro 5 is admitted only from exact
Microsoft DMI plus SKU `Surface_Pro_1796` or `Surface_Pro_1807` and is scheduled
for parity proof after Pro 6.

This is a runtime and acceptance-authority checkpoint, not physical acceptance
or release promotion.

## Implemented boundaries

- Activation and MOK results use a bounded, versioned shared contract with
  exact node, request, Pro 5/6 model/generation, producer, completion time, and
  closed outcome binding. Unknown, duplicate, oversized, stale, future, or
  foreign records fail closed.
- `surface_enable` parses each request once and carries the same admitted
  request identity through authorization, effects, and result publication.
  Its production seam has no reboot method, request arm field, reboot token,
  `RebootArmed`, or serializable private result shape.
- A staged MOK certificate publishes only
  `AwaitingGovernedHostReboot`. The shell emits a zero-sized navigation handoff
  to System / Power & Battery, clears any prior confirmation, and carries no
  token, request body, capability, or confirmation.
- Firmware apply publishes a bounded shared schema-v2 result correlated to the
  exact local request, model, inventory generation, device, release version,
  checksum, fwupd source, and completion time. The shell holds one apply for a
  bounded 45 minutes, covering the provider's ten-minute download plus
  thirty-minute install limits.
- A separately armed camera action requires exact-body local authorization and
  the phrase `PROVE CAMERA`, then runs the Fedora 44 libcamera 0.7.1 one-based
  first camera for one frame directly to `/dev/null`. It clears the environment,
  retains no frame, identifier, metadata, or provider output, kills and reaps at
  eight seconds, and publishes only a closed result.
- The camera action is polled every second within its 30-second capability
  lifetime; ordinary verification remains on its 30-second cadence. The local
  card correlates one request and renders only the closed result.
- Workers Device Inventory now folds every admitted
  `state/hardware/surface/<node>` summary into a read-only fleet view with
  model, enablement percentage, red subsystems, and freshness. Non-Surface
  nodes are excluded, and remote device controls are unavailable at both render
  and dispatch boundaries.
- Shared Surface observations bind exact Pro 5/6 model-generation pairs and
  their expected Kernel or fwupd producer. Private verify/result serde mirrors
  were removed.
- The acceptance collector reads the exact local camera-result Bus lane through
  read-only SQLite, admits only a fresh `Passed` result, and records a
  privacy-safe projection plus the raw-result SHA-256. Physical recording and
  promotion independently bind that hash. Pre-r16 bundles intentionally require
  recollection.
- The promotion verifier binds a ready signed Surface stack, clean exact
  revision and deployment preflight, exact deployed package identities,
  canonical accepted Pro 6 record, optional Pro 5 record chained to Pro 6, and
  explicit camera, audio, power, radio, service, reboot, upgrade, suspend, S0ix,
  and mesh evidence. Atomic publication cannot overwrite an existing decision.

Relevant commits:

- `6ffe2b01` — route staged MOK reboot through host-state navigation.
- `499b4424` — add the read-only Surface fleet rollup and local-only device controls.
- `88cece6a`, `dc7afbc9` — add and expose the privacy-safe camera proof.
- `7a851581`, `b6fe6ca5` — share and consume firmware apply results.
- `1777db69` — share and consume enable/MOK results.
- `5640899f`, `fcdaa3b9` — remove Surface reboot authority and legacy wire state.
- `0583b3f4` — bind observations to exact models and providers.
- `a4cec4cc`, `dce86fa2` — bind camera and physical evidence into fail-closed promotion.

## Verification

Named farm gates completed with no failed Surface test:

- shared Surface contracts: 19/19;
- complete daemon `surface::` suite with `async-services`: 120/120;
- firmware producer: 36/36 and firmware card consumer: 3/3;
- camera verifier/worker: 32/32, shared camera contract: 12/12, focused
  camera card: 5/5, and broad card suite at that checkpoint: 16/16;
- final shared enable card suite: 19/19;
- fleet rollup: 4/4 plus 1/1 remote-authority refusal;
- post-MOK navigation handoff: 1/1;
- shared enable contract: 3/3 plus the focused publication/refusal gates.

The collector, physical recorder, and promotion-verifier hostile self-tests
also pass. The collector includes 14 redaction strings, bounded fwupd fixtures,
and nine hostile camera-result fixtures. Exact-file formatting and diff checks
passed for the owned paths. No live hardware success was inferred from farm or
parser tests.

## Remaining hard gates

- The locked Surface kernel certificate's matching private key is unavailable,
  and the kernel producer requires 45 GiB scratch while current BigBoy capacity
  is below that bound.
- A complete operator release-signed five-package Surface artifact set and real
  pinned Fedora 44 bootc digest have not been supplied or deployed.
- The repository has no approved tracked SSH recovery public key plus exact
  OpenSSH fingerprint; governed SSH to canonical Pro 6 is still refused and its
  overlay path is unavailable.
- Pro 6 still needs direct signed-candidate installation, MOK firmware screen,
  reboot, camera/privacy indicator, touch/pen/Type Cover, SAM/IIO, DRM native/HD,
  audio/mic, firmware, suspend/S0ix, network/mesh return, upgrade, and corrected-
  forward proof.
- Pro 5 requires the same accepted record after the hash-bound canonical Pro 6
  record. The operator has stated that a Pro 5 can be added when this physical
  window is ready.

Therefore WL-UX-011, WL-CRIT-006, WL-CRIT-007, WL-UX-009/012, and the Surface
audio row in WL-FUNC-021 remain open. No first-class physical acceptance or
production promotion is claimed.
