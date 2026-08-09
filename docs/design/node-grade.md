# System and Mesh Health authority

Operator lock updated 2026-08-03. The historical dock grade list, blinking D/F
rows, per-workspace health summaries, and numeric weighted score are superseded.
The only platform/system/mesh issue presentation is the centered **System and
Mesh Health** modal, opened by the shared taskbar health icon or by health and
diagnostics search terms.

## Authority and contract

`mackes-mesh-types::health` owns the versioned wire contract:

- `NodeHealthState` is the node-owned publication.
- `SystemMeshHealthSnapshot` is an observer's current-roster fold, including
  current grades, active and resolved conditions, freshness, and mesh summary.
- `HealthCondition` is the sole actionable-condition ledger. It carries a stable
  identity, node or mesh scope, component, source, severity, evidence, lifecycle,
  acknowledgement/snooze state, and offered remediation.
- `HealthActionRequest` and `HealthActionResult` carry typed, audited remediation.
  `HealthAction` is a closed allowlist; arbitrary service names, paths, and shell
  commands cannot cross the UI boundary.

The daemon publishes these records on typed Bus topics and writes replicated
node rows below `system-mesh-health/nodes/`. Each observer folds rows below the
current canonical roster into `system-mesh-health/snapshots/<observer>.json`.
Sync-conflict files, aliases, non-roster publishers, duplicate publishers,
mismatched roster revisions, mismatched grade owners, and expired rows are
rejected. Missing current publishers become one mesh-scoped evidence condition.
Critical transitions use the existing `event/notify/` path that mesh Chat folds;
there is no second durable alert ledger.

Node publications remain valid for 120 seconds. This deliberately covers two
normal 60-second Syncthing fallback scans on mounts where filesystem watcher
events are unavailable; the exact observation timestamp remains visible, and
an absent publisher still becomes actionable after that bounded window instead
of oscillating healthy/stale during ordinary replication.

## Grade invariant

A–F is the sole overall grade vocabulary and is condition-backed. The shared
health type applies this policy for both node publications and mesh folds:

- any active critical condition produces F;
- otherwise two or more distinct active required warning identities produce E;
- otherwise one active required warning produces D;
- with no active actionable condition, headroom/capability distinguishes A, B,
  and C only; a condition-free node can never grade D, E, or F.

Optional, informational, resolved, wrong-node, and duplicate condition
identities do not contribute to escalation. For a repeated identity, the
strongest admitted severity wins. This is the only E production policy;
renderers carry the authority's exact grade and never regrade presentation.

CPU, memory, disk, system, mesh, and device factors are evidence fields, not a
second severity score. CPU and memory require three consecutive threshold
breaches. Disk policy is warning at 85% and critical at 95%. Ordinary headroom
only changes A–C. Missing required evidence is actionable.

Service expectations are role/capability driven. Workstations require the shell,
daemon, Bus, DNS, sync, KDC, Nebula, and current `mm` user-session audio proof.
Voice and music are evaluated only when assigned and are otherwise optional.
The mesh factor uses `/run/mde/mesh-status.json`, including current lighthouse
reachability, rather than the retired peer-directory interpretation.

Audio health is boot-bound: the guided restore action enables the three `mm`
user services, performs direct PipeWire playback and capture, and records the
successful boot identity in `/var/lib/mackesd/health/audio-proof.json`. A service
graph without that current-boot playback/capture proof remains actionable.

Device inventory is neutral. PCI `enable == 0` is not administrative-disable
evidence, an absent driver binding is `unknown`, and only explicit platform
policy or corroborated kernel errors become health conditions. Device conditions
carry a deep link to the affected neutral inventory record.

## Shell presentation

Both taskbar layouts render the same logical `HealthStatus` icon. Its accessible
label and badge report the exact active, required, unacknowledged, unsnoozed
condition count. A zero state is calm and reads `0 active issues`; A–F appears
only inside the modal.

The modal's locked matrix order is Seat 15, Dell, Eagle, T480, Surface, then
Mesh-wide. Columns are Node, Grade, System, Mesh, Resources, Devices, and
Freshness. Selecting a row reveals evidence, provider/observation timestamps,
resolved history, acknowledgement and snooze, expected remediation impact,
confirmation, and inventory deep links. Guided mutations are offered only on
the target seat and are authorized again by the daemon against the expected
snapshot generation and active condition.

A newly observed critical condition auto-opens once per occurrence. Opening is
queued while the seat is locked, an immersive guest is focused, or conflicting
confirmation/chrome is active. The same identity cannot reopen until it resolves
and later recurs.

## Verification lock

Acceptance requires farm unit/integration gates, dark/light/narrow/desktop/
large-text render proof, and direct polling on all five seats. Each seat must
prove current package identity, required providers, device inventory, resource
thresholds, `mm` user-session playback and capture, exact badge count, modal
rendering, guided-action audit, and critical auto-open behavior. Completion is
not claimed until the five direct polls show zero warning/critical conditions
for two publisher freshness cycles plus reboot/resume checks.
