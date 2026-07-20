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
roles/cloud_vm/                 configure a VM via the SETUP-7 site.yml convergence
playbooks/site.yml              the configure entrypoint (targets the cloud_vm group)
tests/roster.fixture.json       a fixture roster for the offline self-test
tests/selftest.sh               offline self-test (no live mesh / no live store)
```

## Dynamic inventory (decided-stack #7)
`inventory/mesh.py` is a `script` inventory that reads the LIVE mesh roster and
groups hosts:
- node ids + role/scope tags from etcd `/mcnf/node-tags/<id>` (SEC-003),
- live membership from etcd `/mesh/peers/<hostname>` (the mackesd peer set).

Groups: `role_<role>`, `scope_<scope>`, `mesh` (all), and `cloud_vm` (nodes tagged
`scope:cloud` — the VMs the Tofu root provisions). Each host's `ansible_host` is
its mesh hostname, reached over the Nebula overlay via mesh DNS.

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
bash automation/ansible/tests/selftest.sh
```
Proves the inventory groups the fixture roster, `playbooks/site.yml` passes
`ansible-playbook --syntax-check`, and the `mde_seal` lookup resolves a fixture
secret through a stubbed store. When ansible is absent on the builder it falls
back to `py_compile` + `bash -n` (still exercised above).
