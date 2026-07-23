# Privileged Bus consumer inventory

Checked: 2026-07-23. This is an evidence ledger, not a second work tracker.
The only authoritative remaining work is **WL-SEC-007** in
`docs/platform/WORKLIST.md`; dispositions below describe observed code state and
feed that epic.

## Root and classification rules

The shipped service sets `MDE_BUS_ROOT=/run/mde-bus`. `mde_bus::default_data_dir`
therefore opens that sticky `1777` spool with its cross-UID SQLite index. A
consumer on that root is reachable by any local Bus writer and must treat the
topic as transport, not authority. “Private root” below means the consumer still
uses `dirs::data_dir()/mde/bus` directly (normally
`/root/.local/share/mde/bus`) and is not reachable from the shipped shared spool;
that split is a functional routing defect, not an authorization control.

Auth classes:

- **HMAC v1**: exact canonical body, closed verb/node/target, schema 1, at most
  30 seconds remaining lifetime, and a durable single-use nonce.
- **Typed/confirm only**: parsing, allow-lists, typed echoes, or confirmation,
  but no proof that the shared-spool writer is the privileged shell.
- **Open**: no caller-authentication boundary before the privileged side effect.
- **Open read**: deliberately available list/status/plan/refresh operation.

## Capability-gated or retired

| Exact topic | Production consumer and root | Privileged effect | Current auth class | Legitimate publisher | Disposition |
|---|---|---|---|---|---|
| `action/exec/request` | `workers/action.rs`; shared `/run/mde-bus` | Runs a closed set of administrative lifecycle actions | HMAC v1 | administrative action surface | Covered before this audit; keep hostile no-run tests. |
| `action/cloud/{provision,configure,destroy,set-desired,image-build,container-deploy,console-attach,android-provision,instance-start,instance-stop,instance-reboot,instance-delete,instance-start-all,instance-stop-all,instance-reboot-all}` | `workers/cloud`; **private root** | OpenTofu/Ansible, image/container, console, and cloud lifecycle mutations | HMAC v1 | root Workloads/IaC shell | Auth covered, but migrate the action read root to the shared resolver; reads `list`, `list-instances`, `list-instances-local`, `status`, `inventory`, `output`, and `plan` stay open. |
| `action/container/lifecycle` (`run`, `stop`, `remove`; `list`/`info` are reads) | `workers/container.rs`; **private root** | Podman lifecycle, including host bind mounts | HMAC v1 | root shell container surface | Auth covered; root split still makes the shipped shared publisher unable to reach it. |
| `action/vm/lifecycle` (mutating lifecycle operations; `list` is a read) | `workers/vm_lifecycle.rs`; **private root** | libvirt VM create/start/stop/reboot/remove | HMAC v1 | root Fleet surface | Auth covered; migrate the action read root to the shared resolver. |
| `action/host/<node>/verb`; forwarded `action/host/local/apply` | `workers/host_state.rs`; **private root** | Seat volume/display/session/power control | HMAC v1 plus host interlocks | remote host-control surface; local shell consumes the approved apply lane | Auth covered; root split remains. The forwarded lane is intentionally internal but must move with the same root deliberately. |
| `action/pty/<peer>` (`open`, `write`, `resize`, `close`, `detach`, `reattach`) | `workers/pty_broker.rs`; shared `/run/mde-bus` | Opens and drives a root-owned SSH login shell | HMAC v1 via shared `ipc/action_auth.rs` | `mde-term-egui` when hosted by the root shell | Gated in the 2026-07-22 tranche. `heartbeat` and `list` remain open harmless nudges/reads. |
| `action/mesh-mount/<host>` (`mount`, `escalate`, `unmount`) | `workers/mesh_mount.rs`; shared `/run/mde-bus` | Mounts a peer home or full `/` over sshfs; unmounts it | HMAC v1 via shared `ipc/action_auth.rs` | `mde-files-egui` when hosted by the root shell | Gated in the 2026-07-22 tranche. |
| `action/storage/<node>` `apply` | `workers/storage.rs`; shared `/run/mde-bus` (explicitly honors `MDE_BUS_ROOT`) | Applies physical partition/filesystem queues through UDisks/tools | HMAC v1 via shared `ipc/action_auth.rs`, in addition to existing disk walls/typed echo | root shell Storage surface | Gated in the 2026-07-22 tranche; `refresh` remains open. |
| `action/storage/<node>/virtual` `apply` | `workers/virtual_storage.rs`, owned by Storage worker on shared root | Runs `qemu-img`/Podman virtual-storage queues | HMAC v1 via shared `ipc/action_auth.rs` | No production publisher found | Consumer is fail-closed; `refresh` remains open. Add a root publisher only with this envelope. |
| `action/apps/uninstall` | No consumer after the uninstall arm was deleted from `ipc/apps.rs`; shared responder returns unknown verb | Formerly root package removal | Retired | No production publisher found | Deleted rather than carrying an unauthenticated dormant package-removal path; regression test pins its absence. |
| `action/jobs/launch` | `ipc/jobs.rs`; root responder on shared root | Materializes and launches a fleet job | HMAC v1 via shared `ipc/action_auth.rs` | No Bus publisher found; `cli/remediate.rs` calls `build_reply` directly and bypasses the Bus | Bus launch gated in the 2026-07-22 tranche; `list-templates`, `runs`, and `run-results` remain open reads. Audit the direct CLI authority separately. |
| `action/fleet/{push-revision,rollback,nudge}` | `ipc/fleet.rs`; root responder on shared root | Mutates replicated baseline revisions or triggers convergence | HMAC v1 via shared `ipc/action_auth.rs` | No production Bus publisher found | Gated in the 2026-07-22 tranche; `list-revisions` and `diff-revisions` remain open reads. The direct revisions CLI is a separate authority path. |
| `action/dc/{tofu-apply,tofu-destroy}` | `ipc/tofu.rs`; root responder on shared `/run/mde-bus` | Unattended OpenTofu apply/destroy in allow-listed workspaces | HMAC v1 via shared `ipc/action_auth.rs`, in addition to existing workspace allow-list/confirm checks | No Rust publisher found | Gated in the 2026-07-22 tranche; 14/14 focused farm tests pass. `tofu-plan` and `tofu-state` remain open reads. |
| `action/dc/{host-power,gateway-reboot,dr-backup,lighthouse-restart,lighthouse-promote,host-vlan-create,router-seal-cred,farm-scale,testbed-up,testbed-down}` | `ipc/host_ops.rs`; root responder on shared root | Dom0 power/evacuation, gateway reboot/backup, lighthouse operations, VLAN/credential changes, desired farm shape/plan mutation, and test VM creation/deletion | HMAC v1 via shared `ipc/action_auth.rs`, in addition to existing typed/confirm interlocks | Datacenter shell (where present); several have no Rust publisher | Gated in the 2026-07-22 tranche; 47/47 focused farm tests pass. `host-impact`, `host-pool`, `gateway-status`, `host-net`, `gateway-dhcp`, and `testbed-list` remain open reads. |
| `action/vpn/{add-tunnel,remove-tunnel,tunnel-up,tunnel-down,setup-provider,set-route,clear-route}` | `ipc/vpn_gw.rs`; root responder on shared root | Writes decrypted VPN configs, invokes tunnel tools, changes egress routing | HMAC v1 via shared `ipc/action_auth.rs`, before config/secret-store/backend access | VPN shell surface | Gated in the 2026-07-23 tranche; 26/26 focused farm tests pass, including all seven unsigned mutations and authorized replay. `list-tunnels`, `tunnel-status`, `list-providers`, `list-routes`, `route-status`, `verify-egress`, and `egress-health` stay open. |

Consolidated farm evidence for the current tree is green: the shared authorizer
filter passed 2/2, and the cross-consumer `hostile_` filter passed 13/13,
including PTY, mesh-mount, physical/virtual storage, Jobs, and Fleet no-backend
and replay cases.

## Shared-spool privileged consumers still feeding WL-SEC-007

| Exact topic | Production consumer and root | Privileged effect | Current auth class | Legitimate publisher | Disposition |
|---|---|---|---|---|---|
| `action/dc/{vm-power,vm-snapshot,vm-clone,vm-delete,vm-suspend,vm-migrate,vm-resize,vm-create,vm-snapshot-revert,vm-snapshot-delete,lighthouse-create,genesis-write,sr-create,vdi-create,vdi-attach,vdi-detach,sr-snapshot}` | `ipc/datacenter.rs` plus `ipc/storage_ops.rs`; root responder on shared root | Remote `xe` VM/storage lifecycle and writes to live IaC source | HMAC v1 via shared `ipc/action_auth.rs`, before op-lock or SSH/filesystem/backend calls | shell Datacenter panels | Gated in the 2026-07-23 tranche; Datacenter focused farm suite is 75/75, including unsigned refusal before backend validation and authorized replay refusal. `vm-snapshots`, `do-regions`, `genesis-plan`, and `backoffice-plan` are open reads; review `vm-console` separately because it creates access material. |
| `action/dc/{wol,ipmi-power,idle-policy}` | `ipc/dc_power.rs`; root responder on shared root | Wakes or power-controls hardware and changes idle policy | Open/typed only | Datacenter Energy surface | Gate mutations; 30/30 focused farm tests pass (unsigned/tampered/replayed/future-schema refusal); `wake-eta` is an open read. |
| `action/connect/{set-policy,expose,unexpose,set-template,apply-template}` | `ipc/connect.rs`; root responder on shared root | Changes public exposure and ingress policy | Open/typed only | Connect shell surface | Gate mutations. `list-services`, `list-candidates`, and `list-templates` stay open. |
| `action/clipboard/{pin,unpin,delete,clear}` | `ipc/clipboard.rs`; root responder on shared root | Mutates/deletes replicated clipboard history | Open | No active first-class clipboard surface found in parity audit | Decide whether this is privileged shell state; if retained, authenticate destructive verbs. `list` stays open. |
| `action/settings/{set,restore}` | `ipc/settings.rs`; root responder on shared root | Applies host settings or restores a snapshot | Open/typed only | Settings surface | Gate mutations. `get`, `list-keys`, and `snapshot` stay open. |
| `action/voip/{set-gateway,clear-gateway}` | `ipc/voip.rs`; root responder on shared root | Writes/removes replicated VoIP credentials/config | Open/typed only | voice/communications administration surface | Gate mutations and avoid returning secrets. `get-gateway` requires a separate secret-exposure review even though it is a read. |
| `action/seat/remote-input` | `workers/seat_remote_input.rs`; shared root | Invokes the root/uinput helper to inject keyboard and pointer events | Open shape checks; upstream phone pairing is not cryptographically bound to this local handoff | `workers/kdc_host` | Critical remaining gap: bind the handoff to an authenticated producer/session and enforce freshness; `remote-input-arm` is not a substitute for writer authentication. |
| `action/federation/{accept,revoke,refuse-mint}` | `workers/federation_enforcer.rs`; shared root | Changes federation trust/certificate state | Open/typed only | federation UI/control path | Gate each mutation; no read verb shares these topics. |
| `action/voice/{provision,did-route,failover,shared-config}` | `workers/voice_provision.rs`; shared root | Provider provisioning and telephony routing/config convergence | Open/typed only; portions integration-gated | Communications voice administration | Gate before provider/runner/config effects; integration gating is not caller authorization. |
| `action/vehicle/reboot` | `workers/vehicle.rs`; shared root when a gateway is configured | Reboots the attached vehicle gateway over privileged SSH | Typed ESN arming only | vehicle control surface | Add HMAC authority before the SSH probe; `action/vehicle/get-config` stays open read. |

## Production-root splits and dormant/private consumers

These do not reduce the threat of the shared responders above. They explain why
some apparently implemented surfaces cannot reach their worker in the shipped
service and must not be mistaken for a security boundary.

| Exact topic | Private consumer | Effect/auth | Publisher/reachability | Disposition |
|---|---|---|---|---|
| `action/container/lifecycle` | `ContainerWorker` direct `dirs::data_dir` | HMAC v1 | shell publishes shared root; normally unreachable | Move reads to `mde_bus::default_data_dir`; preserve gate. |
| `action/vm/lifecycle` | `VmLifecycleWorker` direct `dirs::data_dir` | HMAC v1 | Fleet publishes shared root; normally unreachable | Move reads to shared resolver; preserve gate. |
| `action/cloud/*` | `CloudWorker` direct `dirs::data_dir` | HMAC v1 | Workloads publishes shared root; normally unreachable | Move reads to shared resolver; preserve gate. |
| `action/host/<node>/verb` | `HostStateWorker` direct `dirs::data_dir` | HMAC v1 | remote host surface targets shared root; normally unreachable | Move both request and approved local lane deliberately. |
| `action/connect/{pair,pair-device,unpair,ring,sms,clipboard,share,sftp,mesh-enroll-token}` | `workers/kdc_host`; direct per-root data dir in its responder helpers | Pairing/store mutation, outbound phone actions, and invite minting; no HMAC seam | shell publishers use the shared root, so parts are dormant/unreachable in production | First repair the single-root contract, then authenticate mutations; keep `version`, `list`, `get`, `devices`, and `browse` open reads. |

## Evidence anchors

- Shared-root contract: `crates/platform/mde-bus/src/lib.rs`.
- Root responder registration and reachability: `crates/mesh/mackesd/src/bin/mackesd/spawn.rs`.
- Common verifier: `crates/mesh/mackesd/src/ipc/action_auth.rs`.
- Capability implementation and durable replay ledger:
  `crates/mesh/mackesd/src/workers/cloud/gate.rs`.
- Consumer source paths and topic constants are named in each table row; the
  apps-uninstall retirement is pinned by `ipc/apps.rs` tests.

This ledger must be rechecked whenever an `action/*` consumer gains a backend,
changes its Bus-root resolver, or adds/removes a verb. Any new work item belongs
under WL-SEC-007, not in this file.
