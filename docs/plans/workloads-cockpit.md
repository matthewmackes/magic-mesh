# Workloads cockpit — build plan (WL-ARCH-006)

Reenvision the `iac/` surface into the **Workloads** cockpit (delivery-type × mesh placement) over the WL-ARCH-001 OpenTofu+Ansible+libvirt backend. This is the in-repo copy of the locked plan for farm fan-out agents.

## Central metaphor
The surface is **not** a raw Tofu cockpit. It presents **five delivery-type views**, each placeable on an explicit mesh node (local or remote peer over Nebula), with Tofu/Ansible/libvirt/Podman as the execution substrate. Five views: **Desktop VM** (native VDI), **Service VM** (headless), **App-only VM** (apps forwarded via VDI app-mode/`session_broker`), **Android VM** (Cuttlefish/AVD), **Service Container** (Podman/Quadlet).

## Key files
- Surface (rewritten behind the kept 4-symbol seam): `crates/desktop/mde-shell-egui/src/iac/{mod,menubar,tests}.rs`; **DELETE** `cloud_plane.rs` + `Plane::Cloud` (U20 atomic).
- Backend: `crates/mesh/mackesd/src/workers/cloud/` (U2 split it into mod/verbs/gate/runner/reconcile.rs); `crates/mesh/mackes-mesh-types/src/cloud.rs` (U1a landed the wire types).
- Infra: `infra/tofu/cloud/` (per-node state key/workspace; tfvars.json from etcd; per-delivery-type shapes); `automation/ansible/roles/` (real per-type roles); `automation/ansible/inventory/mesh.py`.

## Wire contract (U1a — LANDED in mackes-mesh-types/src/cloud.rs)
New verb tokens (`VERB_*`): `set-desired`, `plan`, `inventory`, `output`, `image-build`, `container-deploy`, `console-attach`, `android-provision` (atop existing provision/configure/destroy/instance-*/list/status).
`WorkloadSpec { name, delivery_type: DeliveryType(DesktopVm|ServiceVm|AppVm|AndroidVm|ServiceContainer), node, vcpu, memory_mb, disk_gb, image, network_isolation, raw_hcl:Option<String> }`.
Reply payloads: `PlanCounts{add,change,destroy}`, `AnsibleSummary{ok,changed,failed,unreachable,changed_tasks}`, `InventoryHost{id,node,groups,reachable}`, `TofuOutput{name,value,sensitive}`, `ImageRow`, `ConsoleProto(Spice|Vnc|WebRtc)`+`ConsoleEndpoint{proto,uri,ticket}`. Mirror types: `DriftFlag(InSync|Drift|Unknown)`, `WorkloadRow`, `DriftSummary`, `NodeCapacity`. `CloudReply` enriched (+plan/outputs/ansible/inventory/images/console/desired/raw_log). `CloudState` enriched (+workloads/drift_summary/node_capacity).

**Arming (U2 — LANDED, replaces MDE_CLOUD_APPLY):** every mutation body carries `armed_token: Option<String>` (mesh-identity-signed capability: nonce+expiry+verb+node+sig, HMAC over `MDE_CLOUD_ARM_KEY`); destroy also carries `typed_name` == workload name. Gate = `cloud/gate.rs`. **Placement-routing (U2 — LANDED):** every node drains `action/cloud/*` but handles a MUTATION iff `body.node == self.host`; reads (list/status/inventory) stay local; offline target → honest `gated`.

## Per-node-apply reconciliation
`set-desired` writes `/mcnf/cloud/desired/<node>/<name>`; worker on node N reads its `<N>/*` slice; `reconcile.rs` renders `terraform.tfvars.json` with only node-N's workloads + `libvirt_uri="qemu:///system"`; `for_each` iterates that slice; per-node state key `/tofu/state/cloud/<node>` (+ lock) via `gen-backend-config.sh` (or `tofu workspace`); drift = periodic `tofu plan` on a throttled cadence decoupled from the state heartbeat → `WorkloadRow.drift` + `drift_summary`.

## Build units (Tier 0 LANDED: U1a, U2, U3-building)
**Tier 1 — backend verb fan-out (disjoint `cloud/verbs/*` after U2; each unit fills its honest-not-yet-wired handler skeleton):**
- **U4 · desired+plan** — `set-desired` writes the etcd desired doc; `plan` renders tfvars from it → `PlanCounts`. (Shares render helpers with U5 → `cloud/render.rs`.)
- **U5 · reconcile-engine** — per-node slicing + workspace/state-key selection + the drift loop. (Shares `cloud/render.rs` with U4 + `backend.tf`/`gen-backend-config.sh` with U11.)
- **U6 · image-build** — build+list+promote per-type golden images (bootc/osbuild); preserve SHA256 Syncthing airgap lane.
- **U7 · container-deploy** — GUI form → Quadlet `.container` unit → Ansible installs as systemd; rootless-default.
- **U8 · console-attach** — reuse `console_broker.rs` SPICE/VNC → `ConsoleEndpoint`.
- **U9 · android-provision** — Cuttlefish two-layer: `AndroidVm` reuses the normal VM path via `modules/android` wrapper (`cpu host-passthrough` + nesting ≥8G/≥4vcpu/≥80G) over a Debian base; `cuttlefish_host` role runs `cvd start --start_vnc_server`; console = VNC/WebRTC. Fallback Android-x86-in-KVM if `.15` lacks nested-KVM.
- **U10 · inventory+output** — `ansible-inventory --list` → `InventoryHost`; `tofu output -json` → `TofuOutput`.

**Tier 2 — infra fan-out:**
- **U11 · tofu-root-reshape** — `infra/tofu/cloud/{main,variables,outputs}.tf`, `modules/vm`, `modules/network`; per-delivery-type `var.vms` shape, per-node tfvars.json, workspace/state-key. (Shares `backend.tf`+`gen-backend-config.sh` with U5.)
- **U12 · tofu container+android modules** — new `modules/{container,android}`.
- **U13 · ansible-roles** — new `roles/{desktop_seat,service,app_forward,cuttlefish_host,container_host}`; base `cloud_vm` stays. (`site.yml`+`inventory/mesh.py` shared → single agent.)

**Tier 3 — surface fan-out (disjoint `iac/*` after U3):** U14 placement-picker · U15 provision-form+HCL · U16 delivery-views (5 disjoint `views/*.rs`) · U17 configure+inventory · U18 status+metrics · U19 images+containers panels.

**Tier 4 — cross-cutting (end):** U20 delete-cloud_plane (ATOMIC: `workbench.rs` Plane enum/ALL 5→4/dispatch/tests + `main.rs` field/nav + `toast_bridge.rs`) · U21 consumers-rewire (`kdc_host/cloud.rs` phone verbs+presets+quotas, `unit_aggregator/sources.rs` drop OPENSTACK_TOPIC_PREFIX, `session_broker` VDI placement → unified path).

## Farm collision map (serialize these)
`mackes-mesh-types/cloud.rs`→U1 · `workers/cloud/` shared files (mod.rs/verbs.rs dispatch) → touch minimally; each unit owns its disjoint `cloud/verbs/<unit>.rs` handler · `cloud/render.rs`→U4+U5 · `backend.tf`+gen-script→U5+U11 · `site.yml`+`mesh.py`→U13 · `iac/mod.rs`→U3 · `workbench.rs`/`main.rs`/`toast_bridge.rs`→U20 atomic. Distinct `MCNF_BUILD_SLOT`/node per concurrent worker; heaviest (U5/U16) → BigBoy.

## Verification
Per-mode egui fixtures; libvirt-fake `CloudRunner` tests per verb; `inventory/mesh.py` selftest; `/audit` OpenStack-terminology grep = 0; live `.15` smoke (provision a Service VM → tfvars → apply on placement node → configure → `state/cloud` with metrics + `*.mesh` name → console via VDI → destroy preview+typed-arm; SSH-verify `virsh list` + state JSON + mackesd journal).
