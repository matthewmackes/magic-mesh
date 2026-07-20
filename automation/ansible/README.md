# Cloud backend — Ansible configure (`automation/ansible/`)

**WL-ARCH-001 Phase B.** The Ansible half of the cloud backend that **replaces the
deleted OpenStack config plane**. OpenTofu (`infra/tofu/cloud/`) *provisions* the
libvirt/KVM VMs; this tree *configures* each one by driving the existing
mackesd-written `/etc/mackesd/site.yml` convergence (SETUP-7) over a
**mesh-derived dynamic inventory** — no static host files, no Ansible Vault.

The mackesd `cloud` worker runs this
(`ansible-playbook -i inventory/mesh.py playbooks/site.yml`) after a Tofu apply,
behind the `MDE_CLOUD_APPLY=1` operator gate.

## Layout
```
ansible.cfg                     wires the dynamic inventory + the mde_seal lookup
inventory/mesh.py               mesh-derived dynamic inventory (script inventory)
plugins/lookup/mde_seal.py      the mde-seal → Ansible secrets bridge (no Vault)
roles/cloud_vm/                 base: configure a VM via the SETUP-7 site.yml convergence
roles/desktop_seat/             Desktop-VM: install/enable the Quasar VDI seat
roles/service/                  Service-VM: headless systemd service(s) from a spec
roles/app_forward/              App-only-VM: VDI app-mode forwarding (session_broker)
roles/cuttlefish_host/          Android-VM: Cuttlefish + cvd (VNC), runs in the Debian VM
roles/container_host/           Service-Container: Podman + Quadlet host prep (rootless)
playbooks/site.yml              the configure entrypoint (base + per-delivery-type passes)
tests/roster.fixture.json       a fixture roster for the offline self-test
tests/selftest.sh               offline self-test (no live mesh / no live store)
```

## Delivery-type roles (WL-ARCH-006 / U13)
The Workloads cockpit places five delivery types, each with a real role selected by
the node's delivery group (see below):

| Delivery type       | Group                        | Role             | What it does |
|---------------------|------------------------------|------------------|--------------|
| Desktop-VM          | `delivery_desktop_vm`        | `desktop_seat`   | Pins the Workstation role + converges its unit set (`mackesd role-pin` / `onboard role-provision`) so the DRM egui seat (`mde-shell-egui.service`) runs. |
| Service-VM          | `delivery_service_vm`        | `service`        | Templates each `service_units` spec into a headless `/etc/systemd/system` unit, enables + starts it. |
| App-only-VM         | `delivery_app_vm`            | `app_forward`    | Marks the VM as an app-mode provider + declares/installs its forwarded app catalog; the existing `session_broker` (universal rank-0 worker) forwards them. |
| Android-VM          | `delivery_android_vm`        | `cuttlefish_host`| Installs android-cuttlefish + runs `cvd start --start_vnc_server` inside the Debian VM. |
| Service-Container   | `delivery_service_container` | `container_host` | Installs Podman, provisions the Quadlet drop-in dir, enables the Podman socket (rootless by default). |

`site.yml` runs a BASE `cloud_vm` pass on every `scope:cloud` mesh VM first, then
the per-delivery-type pass. Roles are idempotent (guarded `command`s, native
`systemd`/`package` modules, `creates:` guards) — no `ignore_errors`.

## Dynamic inventory (decided-stack #7)
`inventory/mesh.py` is a `script` inventory that reads the LIVE mesh roster and
groups hosts:
- node ids + role/scope tags from etcd `/mcnf/node-tags/<id>` (SEC-003),
- live membership from etcd `/mesh/peers/<hostname>` (the mackesd peer set).

Groups: `role_<role>`, `scope_<scope>`, `mesh` (all), `cloud_vm` (nodes tagged
`scope:cloud` — the VMs the Tofu root provisions), and `delivery_<type>` per
Workloads delivery type. A node's delivery type comes from a `delivery:<type>` tag
line OR a `scope:<type>` whose value is a known delivery token (mirroring
`DeliveryType::as_str` in `mackes-mesh-types/src/cloud.rs`), so the existing
`MCNF_NODE_SCOPES` publisher works unchanged. A Fedora mesh VM is tagged
`scope:cloud` + its delivery (base pass **and** specialization); an `android_vm` is
a Debian Cuttlefish VM that is NOT a mackesd mesh node, so it carries only its
delivery tag and is intentionally absent from `cloud_vm`. Each host's
`ansible_host` is its mesh hostname, reached over the Nebula overlay via mesh DNS.

Roster source: `MESH_INVENTORY_FIXTURE=<json>` (tests / offline) wins; otherwise
etcd at `MCNF_ETCD` or the first `/etc/mackesd/etcd-endpoints`. An unreachable
store with no fixture yields an empty-but-valid inventory (fail-soft, honest).

```sh
cd automation/ansible
MESH_INVENTORY_FIXTURE=tests/roster.fixture.json ansible-inventory --list
ansible-inventory --graph        # live: reads etcd
```

## Secrets (mde-seal, decided-stack #8 — NO Vault)
`plugins/lookup/mde_seal.py` resolves an age-sealed secret from the mesh store at
run time — the SAME store the Tofu external data source reads:
```yaml
join_token: "{{ lookup('mde_seal', 'nebula-join-token') }}"   # no_log the task
```

## Self-test (offline — no live mesh)
```sh
bash automation/ansible/tests/selftest.sh          # full harness
python3 automation/ansible/inventory/mesh.py --selftest   # inventory groups only
```
Proves `mesh.py --selftest` (a synthetic roster → role/scope/cloud_vm +
`delivery_<type>` group membership, with the android_vm excluded from `cloud_vm`),
the inventory groups the fixture roster, `playbooks/site.yml` passes
`ansible-playbook --syntax-check`, and the `mde_seal` lookup resolves a fixture
secret through a stubbed store. When ansible is absent on the builder it falls
back to `py_compile` + `bash -n` (still exercised above).
