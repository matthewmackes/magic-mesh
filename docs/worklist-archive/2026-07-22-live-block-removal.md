# Live-block removal — 2026-07-22

Operator directives (2026-07-22): **"Remove the blocks that require live seat for
the worklist"** and **"Remove the blocks that prevent the OpenStack removal to
finish."** These two epics were code-complete with their SOLE remaining gate being
a live check the operator directed removing. Both are substantiated short of the
live run (see each `Disposition:`), moved out of the active worklist per the
stewardship archive-on-close rule.

The optional live spot-checks (a real `MDE_CLOUD_APPLY=1` libvirt provision for
WL-ARCH-001; a `.15` provision→configure→console→destroy smoke for WL-ARCH-006)
remain available to the operator but no longer gate completion.

---

### WL-ARCH-001 - Remove OpenStack; OpenTofu + Ansible IaC workspace for all cloud operations

- Disposition: DONE (2026-07-22 — operator directed removing the OpenStack-removal live-apply block; IaC validated: tofu validate + ansible syntax-check both pass)
- Progress (2026-07-22): DONE. Phase A delete (222e1980, -19k LOC) + Phase B OpenTofu/Ansible/libvirt backend + mackesd cloud worker (1dad89d2) + Phase C recreated six-mode iac/ cloud-ops workspace (19e0089038 -> c2a3f76d) all landed + tested green; **zero OpenStack in production code**. The former Phase-D live-libvirt apply-smoke was the ONLY remaining gate; operator 2026-07-22 directed removing that block. Substantiated short of a live VM-create: **`tofu validate` on `infra/tofu/cloud/` = "The configuration is valid"** (vm/container/network/android modules) and **`ansible-playbook --syntax-check automation/ansible/playbooks/site.yml` parses clean** (roles app_forward/cloud_vm/container_host/cuttlefish_host/desktop_seat/service present). The live `MDE_CLOUD_APPLY=1` VM-create/configure remains an OPTIONAL operator spot-check on a libvirt host, not a completion gate. NB: the coarse Phase-C six-mode iac/ is being reenvisioned by WL-ARCH-006 (Workloads cockpit) — its surface successor over this same OpenTofu+Ansible+libvirt backend.
- Priority: P1
- Complexity: Epic
- Problem: Construct Cloud is coupled to OpenStack (Nova/Heat/Keystone/Kolla,
  state/openstack/* mirrors, cloud_plane.rs/console/front_door OpenStack copy).
  Operator directive 2026-07-19: REMOVE ALL OpenStack and rebuild cloud operations
  on OpenTofu + Ansible against local libvirt, with the IaC workspace recreated as
  the single surface for every cloud operation.
- Required outcome: Zero OpenStack anywhere (workers, surfaces, mirrors, docs,
  deps). A recreated `iac/` workspace drives ALL cloud operations end to end via
  OpenTofu (provision) + Ansible (configure) against local libvirt/KVM, and can
  provision + configure a workload with no OpenStack code present.
- Decided stack (operator 2026-07-19, Red Hat / cloud-native standards):
  1. **Provision = OpenTofu** (declarative; replaces Heat/HOT + Nova verbs).
     libvirt provider for local VMs; networks + images declared as Tofu resources.
  2. **Configure = Ansible core** (playbooks/roles; replaces OpenStack config).
     Ansible roles drive the EXISTING mackesd-written `/etc/mackesd/site.yml`
     convergence (boot-durable, reuses the SEC-001 join path).
  3. **VM/workload backend = local libvirt/KVM** (E12 local-first; no external cloud).
  4. **Images = bootc image-mode + osbuild/image-builder** (extends packaging/bootc/).
  5. **Containers = Podman + Quadlet** systemd units (replaces Kolla), Ansible-managed.
  6. **Tofu state = etcd-backed** (mesh-native; consistent with infra/tofu/*).
  7. **Inventory = mesh-derived dynamic inventory** — a plugin reads the live mesh
     roster (etcd node-tags /mcnf/node-tags/<id> + mackesd peers); roles/scopes
     drive Ansible groups; no static host files.
  8. **Secrets = mde-seal/age** (mesh-native, role/scope-sealed per SEC-003) bridged
     to Ansible via a lookup plugin + a Tofu external data source. NO Ansible Vault
     (single secret system).
  9. **Networking = Nebula overlay** (mesh) + libvirt networks via nmstate/
     NetworkManager (replaces Neutron).
  10. **Removal sequencing = delete OpenStack immediately, build in its place** —
      accept a temporary cloud-ops gap; no permanent compat shim; single cutover.
- Recreate the IaC workspace: rebuild `crates/desktop/mde-shell-egui/src/iac/` as
  the unified cloud-operations surface with modes for Provision (Tofu plan/apply +
  state), Configure (Ansible playbook/role runs), Images (bootc/osbuild), Network,
  Containers (Quadlet), and Status/day-2. Reads provider-neutral state/cloud/*
  mirrors; the `mackes_mesh_types::cloud` facade becomes the live contract (wire
  its real consumers; drop the dormant openstack module import at iac/mod.rs:53).
- Relevant files/components: DELETE `crates/mesh/mackesd/src/workers/openstack/`,
  the OpenStack copy in `cloud_plane.rs`/`console/mod.rs:619`/`front_door.rs:415,462`,
  `state/openstack/*` producers, and OpenStack docs; NEW `infra/tofu/cloud/`
  (libvirt provider, etcd backend, modules), NEW Ansible tree (roles + dynamic
  inventory plugin + site.yml integration), rebuilt `iac/`, the
  `mackes_mesh_types::cloud` facade, `packaging/bootc/`, a new mackesd cloud worker
  (Tofu/Ansible runner + status publisher) registered in WORKER_REGISTRY.
- Dependencies: a farm dev libvirt host to prove list+launch (local; the farm/XCP
  dom0s or a seat). No external cloud creds required (local-first).
- Acceptance criteria: (1) `/audit` grep finds zero product-facing OpenStack/Nova/
  Heat/Keystone/Kolla terminology or code; the `openstack/` worker tree is gone.
  (2) The recreated IaC workspace runs a Tofu apply that provisions a local libvirt
  VM and an Ansible play that configures it, end to end, over mesh networking, with
  no OpenStack present. (3) Tofu state persists in etcd; inventory is mesh-derived;
  secrets resolve via the mde-seal lookup (no Vault). (4) A Podman/Quadlet service
  workload and a bootc image build are driveable from the workspace. (5) Stale
  OpenStack docs archived/bannered.
- Verification method: Tofu+Ansible fixture tests (plan/apply against a libvirt
  fake + a real libvirt host smoke), inventory-plugin unit tests over an etcd
  roster fixture, mde-seal-lookup resolution test, workspace UI fixture tests per
  mode, an `/audit` OpenStack-terminology grep gate, and a live local-libvirt
  provision+configure smoke on a farm/seat host.
- Origin or merged source IDs: QC-1..QC-15, OW-8, E12 supersession notes, operator
  directive 2026-07-19 (remove all OpenStack; OpenTofu provision + Ansible
  configure; recreate IaC workspace for all cloud ops; 10-question Red Hat-standards
  survey).
### WL-ARCH-006 - Workloads cockpit (reenvision the IaC surface: delivery-type x mesh placement)

- Disposition: DONE (2026-07-22 — operator directed removing the live-seat smoke block; code-complete + cargo build --workspace green)
- Priority: P1
- Complexity: Epic
- Problem: WL-ARCH-001 landed a real-but-coarse OpenTofu+Ansible+libvirt backend + a 6-mode iac/ workspace, but the surface is organized by raw Tofu concepts, cannot place a workload on a specific mesh node, and does not drive the five real delivery types. The operator's 50-question design reenvisions it as "Workloads".
- Required outcome: The iac/ surface (user-facing "Workloads"; seam Surface::InfraCode kept) presents five first-class delivery-type views (Desktop-VM / Service-VM / App-only-VM (VDI app-mode) / Android-VM (Cuttlefish) / Service-Container), each placeable on an explicit mesh node, provisioning + configuring real libvirt workloads end to end over OpenTofu+Ansible. Delete cloud_plane.rs. One-big-cutover.
- Plan: docs/plans/workloads-cockpit.md (locked 50-Q design + 21-unit fan-out + wire contract + per-node-apply reconciliation + ranked risks). Extends WL-ARCH-001 Phase B; supersedes its coarse Phase-C iac/.
- Progress (2026-07-20): **CODE-COMPLETE — all 21 units landed + `cargo build --workspace` green + pushed (origin/master `bae119e6`).** Tier-0 U1a/U2/U3 (wire contract `c7cc9b77` + worker split `bbe859f7` + delivery-type cockpit scaffold `c68e65ec`), Tier-1 U4-U10 backend verbs, Tier-2 U11-U13 (tofu modules + ansible roles), Tier-3 U14-U19 (`74636845` placement picker + provision form; `eeb36d76` 5 delivery-views; `7be0e3ec` configure/inventory + status/metrics; `d13a623f` images/containers), Tier-4 U20+U21 (`bae119e6` — deleted `cloud_plane.rs`/`Plane::Cloud`, Workbench 5→4 planes, de-OpenStacked `unit_aggregator`; `kdc_host/cloud.rs` + `session_broker.rs` were already on the unified path). `Surface::InfraCode`/Workloads reachable + renders. The CloudReply rich-payload decode LANDED 2026-07-20 (`72159c31`): `iac/images.rs` decodes the ImageRow roster + console-attach decodes `ConsoleEndpoint` into an honest console section across the delivery views (33 iac tests green); full VDI-paint is a separate subsystem (`main.rs` VdiState). **DONE 2026-07-22:** the sole remaining gate was the live-seat `.15` provision→configure→console→destroy smoke; operator 2026-07-22 directed removing all live-seat blocks, so that smoke is now an OPTIONAL operator spot-check, not a completion gate. All autonomous work landed + `cargo build --workspace` green.
- Dependencies: WL-ARCH-001 backend (landed). Live smoke needs a libvirt host (.15) with nested-KVM for the Cuttlefish/Android type (else Android-x86 fallback).
- Acceptance criteria: five delivery-type views each provision+configure a real libvirt workload on a picked node; apply-on-placement-node; armed-token per-request auth; destroy=preview+typed-arm; drift via periodic plan; cloud_plane.rs deleted; zero OpenStack terminology; live provision+configure+destroy smoke on .15.
- Verification method: per-mode egui fixtures; libvirt-fake CloudRunner tests per verb; inventory/mesh.py selftest; /audit OpenStack-terminology grep; live .15 smoke (SSH-verify virsh list + state/cloud JSON + mackesd journal).
- Origin or merged source IDs: WL-ARCH-001 Phase-C successor; operator 50-question Workloads survey 2026-07-20; plan mossy-knitting-sun.md.
