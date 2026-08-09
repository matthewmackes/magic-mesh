# Workload authority inventory

This is the checked inventory for `WL-ARCH-010` S1. It describes the live
authority graph; it is evidence for the canonical worklist, not another
worklist. Repository paths are the durable owner identifiers.

## Live authorities

| Concern | Sole authority | Producers / readers | Boundary |
| --- | --- | --- | --- |
| Operation contract | `mackes-mesh-types::workloads` | Shell `workload_api`, onboarding, and daemon action adapters construct the same bounded type | `action/workload/operation` only |
| Operation consumer, journal, reconciliation | `mackesd::workers::workload_compute` | no second consumer | persisted request precedes every side effect |
| VM lifecycle adapter | `workload_compute::SystemWorkloadActuator` | libvirt / `virtqemud` via bounded `virsh` commands | only the reconciler invokes workload or cold-migration lifecycle effects; `compute_migrate` can submit bounded commands but has no adapter |
| Container lifecycle adapter | `workload_compute::SystemWorkloadActuator` | rootful Quadlet / systemd and approved Podman image materialization | only the reconciler installs or removes a Workload Quadlet unit |
| Runtime/readiness projection | `workload_compute::publish_projection` | shell `workload_api`, datacenter, desktop sources, and daemon IPC are read-only consumers | one bounded `state/workloads/<node>` snapshot; peer heartbeats carry no VM/container roster |
| Native presentation lease | `workload_compute` Display1 attachment runtime | shell `display1_client` consumes one-use local leases | lease metadata may be projected; descriptors stay on the authenticated Unix socket and never enter the Bus |
| Session semantics | `session_broker` | chooser/session rail publish session intent; roaming reads session state | owns user/session focus only; cannot actuate a VM/container or mint a console endpoint |
| Placement proposals | `scheduler` | publishes placement events | cannot publish Workload operations or invoke a runtime adapter |
| Storage observation/provisioning | `storage` | read-only runtime probes and managed host virtual-storage setup | cannot change an individual Workload power state |

## Typed caller inventory

- Browser, Workloads/IaC, Datacenter, Explorer, Front Door, chooser, first
  desktop onboarding, and daemon action adapters publish the shared
  `WorkloadOperationRequest` contract.
- `Open` and `StartAndAttach` request a declared `WorkloadAttachmentProtocol`;
  the shell never requests or decodes a raw console `host:port` record.
- Cloud VM/container day-two verbs and the former cloud console verb are
  unclassified and fail before authorization or backend dispatch.
- Direct/manual RDP, VNC, and SPICE endpoints remain user-selected transport
  inputs. They are not VM lifecycle or attachment authorities.

## Retired reachability map

| Retired topic or symbol | Replacement | Negative proof |
| --- | --- | --- |
| `action/vm/lifecycle` | `action/workload/operation` | authority lint scans production shell sources and daemon spawn wiring |
| `action/container/lifecycle` | `action/workload/operation` | authority lint scans production shell sources and daemon spawn wiring |
| `VmPowerRequest` / `LIFECYCLE_TOPIC` | `WorkloadOperationRequest` | authority lint rejects either shell symbol |
| cloud `console-attach` dispatch | typed Workload `Open` / `StartAndAttach` | cloud hostile test requires unknown-verb refusal before backend dispatch |
| `console_broker` worker and `state/vdi/console` | authenticated Workload Display1 lease | source modules and shell reader were deleted; authority lint rejects either file/module/topic |
| Browser transport attach JSON schema/example/verifier | Workload attachment lease contract | obsolete package artifacts were deleted; package contract no longer invokes the retired verifier |
| Console `podman ps` / `virsh list` inventory shortcuts | `Surface::InfraCode` backed by `state/workloads/<node>` | authority lint rejects raw Podman/libvirt command literals in production shell sources |
| Nova domain-name heuristic and Cloud-managed badge | typed backend and power dimensions in `WorkloadOperationStatus` | provider-specific detector, badge, and warning path were deleted from Datacenter |
| Heartbeat `podman ps` / `virsh list` probes and `ServiceDescriptors::{containers,vms}` | local and replicated `state/workloads/<node>` projections | probe functions and peer fields were deleted; authority lint rejects their commands, fields, and desktop-source readers |
| Replicated `compute-inventory.json` VM roster used as probe targets | enrolled peer identity bundles plus bounded LAN/operator targets | production resolver and legacy reader were deleted; authority lint rejects the retired file/symbol |
| Datacenter `action/dc/vm-*` responders and `event/dc/vm/*` roster | typed Workload operations and `state/workloads/<node>` | VM verbs and XAPI VM sampling were deleted; retained VM topics are ignored and cannot be republished |
| XCP `action/provision/*`, `compute/xcp-host/*`, `xcp_provision`, and `xcp_host` | typed Workload operations and backend-specific HostCapacity admission | both workers and the runtime `mackes-xcp` crate were deleted; authority lint rejects their files, modules, registrations, and topics |
| `compute/create/*`, `compute/create-ack/*`, `compute_provision`, and its producerless certificate responder | typed Workload create operations, canonical sealed-CA enrollment, and `state/workloads/<node>` status | the orphan worker directly ran `virt-install`; it and the obsolete responder had no production publishers, were deleted, and are rejected by authority lint |
| cloud `provision`, direct instance lifecycle, and shell Provision Apply | typed Workload operations | OpenTofu apply and direct libvirt lifecycle methods were deleted; the retained provision wire verb refuses without consuming authorization or contacting a backend, and authority lint rejects restoration |

## Known non-lifecycle runtime tools

Repository-wide `virsh`, Podman, and systemd searches include storage probes,
host pool provisioning, migration, and unrelated service supervision. They are
not automatically lifecycle authorities. Each is classified by effect:

- the desktop shell offers no curated raw `virsh` or Podman command; operators
  enter Workloads for authoritative VM/container inventory and lifecycle;
- peer heartbeats probe only non-Workload services; remote desktop VM cards
  fold the serving peer's validated typed Workload snapshot;
- network discovery accepts overlay targets only from enrolled peer identity;
  stale retired compute inventories cannot inject VM addresses into scans;
- storage/runtime-probe calls are read-only;
- service supervisors operate their named host services, not Workload units;
- `compute_migrate` retains the distributed cold-migration protocol and bounded
  `rsync` disk transfer, but has no libvirt adapter. Capture, shutdown,
  observation, define/start, rollback, and relinquish commands cross an
  in-process bounded command/reply channel and execute only when
  `WorkloadComputeWorker` drains them through its owned actuator. Each command
  is atomically journaled as `Pending` before the actuator, then `Applied`
  before cleanup; restart recovery replays only pending idempotent commands.

The command boundary and distributed protocol are now fail-closed, bounded,
duplicate-key-safe, and restart-recoverable. `compute_migrate` atomically owns
its four Bus cursors, admitted source/target/acknowledgement jobs, retained
definition, publish state, wall-clock deadline, and relinquish/rollback retry
phase before each external effect. Broader live libvirt crash injection and
adapter/attachment proof remain ARCH-010 work.
