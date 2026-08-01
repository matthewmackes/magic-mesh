# Construct Fit-for-Purpose Audit — 2026-07-30

> Evidence and decision record, not a second active worklist. Action ownership
> lives in [`docs/platform/WORKLIST.md`](WORKLIST.md), principally
> `WL-CRIT-006` and the seven existing feature epics.

## Executive finding

Construct fits a small, technically capable, mutually trusted workgroup that
needs an encrypted mesh, headless control-plane services, and VM-based remote
desktops. Its architecture is substantially aligned with that purpose, but the
current release is an engineering preview rather than a production-ready
general desktop or infrastructure platform.

Production promotion is currently conditional because the active worklist still
contains seven unfinished feature epics plus the new cross-cutting readiness
epic, live six-node and hardware evidence is incomplete, the host Browser still
conflicts with the VM-only desktop model, and the project is pre-1.0 with a
rolling release policy.

## Purpose assessment

| Purpose | Verdict | Actionable finding |
|---|---|---|
| Private encrypted mesh for a small trusted workgroup | Conditional fit | Keep Nebula, etcd, Syncthing, CA enrollment, and revocation; require the fixed six-node production gate and explicit trust-boundary diagnostics. |
| Headless lighthouse, relay, CA, and control plane | Conditional fit | Prove automatic lighthouse failover, corrected-forward recovery, and capability-based readiness. |
| No-fixed-center fleet desired state | Conditional fit | Make replicated-state merge rules, provenance, and conflict behavior explicit per domain. |
| VM-based thin-client workstation | Conditional fit | Complete the VM-only Browser transition and make VDI sessions resumable. |
| Integrated files, terminal, editor, media, voice, chat, and bookmarks desktop | Conditional fit | Finish the existing feature epics and control the product surface with capability profiles. |
| Mesh collaboration and clipboard | Not yet fit for production | Complete Mesh Teams and the canonical bounded text clipboard lane. |
| Maps, navigation, and MG90 operations | Not yet fit for production | Replace placeholders with offline routing, typed MG90 state, freshness, provenance, and live adapter proof. |
| Security-sensitive infrastructure for a small flat-trust group | Conditional fit | Retain the crypto floor, but quarantine capabilities and document the flat-trust blast radius. |
| Zero-trust, multi-tenant, hyperscale, or consumer desktop platform | Not a fit | These purposes conflict with the explicit flat-trust, workgroup-scale, no-RBAC, VM-thin-client architecture. |

## Evidence boundary

Static tests, coverage, clippy, policy lints, and package checks establish
implementation evidence; they do not establish live mesh, hardware, recovery,
VDI, or production evidence. A release may be published as an engineering
preview with unavailable live evidence documented. It may not be called
production-ready until every required live gate passes.

The platform's supported production envelope is fixed at three lighthouses and
three workstations. Guests and workloads remain capability-scoped rather than
silently becoming enrolled mesh peers. Existing encrypted backups remain
mandatory during the transition to the selected replicated-live-state recovery
model; they must not be disabled before peer-replication recovery is proven.

## Selected action register

These are the 25 decisions from the operator audit. `AUD-*` identifiers are
stable references for this evidence record; they are not active worklist IDs.

| ID | Selected action | Owner |
|---|---|---|
| AUD-01 | Make GitHub required checks authoritative; use the farm as the heavy self-hosted backend. | WL-CRIT-006 |
| AUD-02 | Require a fixed six-node production baseline: three lighthouses and three workstations. | WL-CRIT-006 |
| AUD-03 | Recover failed nodes by corrected-forward re-enrollment, not required rollback. | WL-CRIT-006 |
| AUD-04 | Add capability quarantine around flat certificate trust. | WL-CRIT-006 |
| AUD-05 | Implement a guided, one-time, scope-bound enrollment ceremony. | WL-CRIT-006 |
| AUD-06 | Expose capability-based readiness: healthy, degraded, stale, unavailable, blocked, recovering. | WL-CRIT-006 |
| AUD-07 | Define deterministic merge and provenance contracts per replicated domain. | WL-CRIT-006 |
| AUD-08 | Complete the VM-only Browser cutover. | WL-ARCH-008 |
| AUD-09 | Give VDI desktops stable workload identity and resumable sessions. | WL-CRIT-006 / WL-ARCH-008 |
| AUD-10 | Use one bounded UTF-8 text clipboard lane; route larger/non-text data to Files/Transfers. | WL-FUNC-016 |
| AUD-11 | Make Mesh Teams the unified collaboration destination. | WL-FUNC-011 |
| AUD-12 | Make This Node the sole durable local-settings authority. | WL-UX-011 |
| AUD-13 | Use typed, allowlisted hardware capability adapters with safety recovery. | WL-UX-011 |
| AUD-14 | Make Maps offline-first with typed MG90 contracts and truthful freshness. | WL-FUNC-017 |
| AUD-15 | Finish one shared Quazar token/component system. | WL-UX-009 |
| AUD-16 | Make the taskbar full-width and Front Door search-first. | WL-UX-012 |
| AUD-17 | Enforce an explicit Fedora compatibility matrix using the oldest supported ABI. | WL-CRIT-006 |
| AUD-18 | Publish signed provenance bundles binding source, artifacts, SBOM, gates, and evidence. | WL-CRIT-006 |
| AUD-19 | Correlate health, audit, worker, transport, workload, and operator events into incident bundles. | WL-CRIT-006 |
| AUD-20 | Target replicated live state for control-plane recovery; retain current backups until migration is proven. | WL-CRIT-006 |
| AUD-21 | Implement automatic multi-lighthouse failover. | WL-CRIT-006 |
| AUD-22 | Add capability and resource-budget workload admission. | WL-CRIT-006 |
| AUD-23 | Minimize durable retention and classify data by replication, TTL, and purge policy. | WL-CRIT-006 |
| AUD-24 | Keep one immutable image but activate bounded capability profiles. | WL-CRIT-006 |
| AUD-25 | Maintain a permanent six-node integration and chaos testbed. | WL-CRIT-006 |

## Required durable interfaces

The implementation must add or formalize:

- versioned release-evidence, topology, capability-profile, and provenance
  schemas;
- enrollment, re-enrollment, recovery, and audit receipts;
- readiness and degraded-capability state enums;
- per-domain merge/conflict contracts with source and revision provenance;
- stable VDI workload/session contracts and the canonical clipboard event;
- typed This Node, hardware, MG90, resource-admission, and retention actions;
- incident timeline/evidence bundles;
- six-node integration fixtures and chaos/recovery scenarios.

## Promotion rule

The next integrated release is an engineering preview until `WL-CRIT-006` and
all selected feature-epic gates pass. GitHub required checks are the release
authority; farm execution supplies the heavy build/test evidence. A missing,
stale, unavailable, or manually asserted gate is a production block, not a
green result.
