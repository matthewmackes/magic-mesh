#!/usr/bin/env python3
"""WL-ARCH-001 Phase B — mesh-derived dynamic inventory for the cloud backend.

The configure leg (Ansible) has NO static host files (decided-stack #7). This
script inventory reads the LIVE mesh roster and emits Ansible hosts + groups:

  * node ids + role/scope tags from etcd ``/mcnf/node-tags/<id>`` (the SEC-003
    published tags: ``role:<r>`` / ``scope:<s>``),
  * live membership from etcd ``/mesh/peers/<hostname>`` (the mackesd peer set;
    a present key = an alive keepalive lease),

and groups every host by role (``role_<r>``), scope (``scope_<s>``), a ``mesh``
group (all), ``cloud_vm`` (nodes tagged ``scope:cloud`` — the VMs the
``infra/tofu/cloud`` root provisions, which the ``cloud_vm`` role configures), and
— WL-ARCH-006 (U13) — a ``delivery_<type>`` group per Workloads delivery type
(``desktop_vm`` / ``service_vm`` / ``app_vm`` / ``android_vm`` /
``service_container``). The delivery group selects the per-type role in
``playbooks/site.yml`` (``delivery_desktop_vm`` → ``desktop_seat``, etc). A node's
delivery type comes from a ``delivery:<type>`` tag line OR, equivalently, a
``scope:<type>`` tag whose value is a known delivery token (so the existing
``MCNF_NODE_SCOPES`` publisher works with no changes). The delivery tokens mirror
``mackes_mesh_types::cloud::DeliveryType::as_str`` (the ONE wire contract). A
Fedora mesh VM is typically tagged ``scope:cloud`` + its delivery (so it takes the
base ``cloud_vm`` pass **and** its specialization); an ``android_vm`` is a Debian
Cuttlefish VM that is NOT a mackesd mesh node, so it carries only its delivery tag
and is intentionally absent from ``cloud_vm``.
Each host's ``ansible_host`` is its mesh hostname, reachable over the Nebula
overlay via mesh DNS.

Roster source resolution (first that yields):
  1. ``MESH_INVENTORY_FIXTURE=<path>`` — a JSON roster fixture (tests / offline
     ``ansible-inventory --list``; NEVER touches a live store),
  2. etcd at ``MCNF_ETCD`` or the first endpoint in
     ``/etc/mackesd/etcd-endpoints``.

Fail-soft: an unreachable store with no fixture yields an EMPTY-but-valid
inventory (honest — never a crash, never fabricated hosts).

Fixture shape (``delivery`` is an optional per-node list, in addition to any
delivery tokens carried as ``scopes``):
  {"nodes": [
     {"id": "lh1",   "role": "lighthouse",  "scopes": [],        "alive": true},
     {"id": "eagle", "role": "workstation", "scopes": ["media"], "alive": true},
     {"id": "seat",  "role": "workstation", "scopes": ["cloud", "desktop_vm"]},
     {"id": "droid", "role": "workstation", "delivery": ["android_vm"], "alive": true}
  ]}

Self-test: ``mesh.py --selftest`` builds the inventory from a synthetic node set
and asserts the role / scope / cloud_vm / delivery_<type> group membership,
exiting non-zero on any mismatch (no live store, no fixture).
"""

import base64
import json
import os
import sys
import urllib.request

# The Workloads delivery-type tokens — the ONE wire contract, mirroring
# mackes_mesh_types::cloud::DeliveryType::as_str (crates/mesh/mackes-mesh-types/
# src/cloud.rs). A tag ``delivery:<token>`` — or a ``scope:<token>`` whose value is
# one of these — puts the node in the ``delivery_<token>`` group that selects its
# per-type role in playbooks/site.yml.
DELIVERY_TYPES = frozenset(
    {"desktop_vm", "service_vm", "app_vm", "android_vm", "service_container"}
)


def _b64(s):
    return base64.b64encode(s.encode()).decode()


def _range_end(prefix):
    """The etcd range_end that selects every key under ``prefix``."""
    b = prefix.encode()
    return base64.b64encode(b[:-1] + bytes([b[-1] + 1])).decode()


def _etcd_range(endpoint, prefix):
    """Return {key: value} for every key under ``prefix`` (v3 HTTP KV range)."""
    body = json.dumps({"key": _b64(prefix), "range_end": _range_end(prefix)}).encode()
    req = urllib.request.Request(
        endpoint.rstrip("/") + "/v3/kv/range",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        data = json.load(resp)
    out = {}
    for kv in data.get("kvs", []) or []:
        key = base64.b64decode(kv["key"]).decode("utf-8", "replace")
        val = base64.b64decode(kv.get("value", "")).decode("utf-8", "replace")
        out[key] = val
    return out


def _resolve_endpoint():
    ep = os.environ.get("MCNF_ETCD")
    if ep:
        return ep
    try:
        with open("/etc/mackesd/etcd-endpoints") as fh:
            first = fh.read().replace(",", " ").split()
            if first:
                return first[0]
    except OSError:
        pass
    return None


def _parse_tags(raw):
    """Split a node-tags value into (role, scopes, deliveries).

    One selector per line: ``role:<r>`` / ``scope:<s>`` (SEC-003), plus the
    optional WL-ARCH-006 ``delivery:<type>`` first-class delivery selector.
    """
    role, scopes, deliveries = None, [], []
    for line in raw.splitlines():
        line = line.strip()
        if line.startswith("role:"):
            role = line[len("role:"):]
        elif line.startswith("scope:"):
            scopes.append(line[len("scope:"):])
        elif line.startswith("delivery:"):
            deliveries.append(line[len("delivery:"):])
    return role, scopes, deliveries


def _nodes_from_fixture(path):
    with open(path) as fh:
        return (json.load(fh) or {}).get("nodes", [])


def _nodes_from_etcd(endpoint):
    """Union node ids from ``/mcnf/node-tags/`` (tags) + ``/mesh/peers/`` (live)."""
    nodes = {}
    try:
        tags = _etcd_range(endpoint, "/mcnf/node-tags/")
    except Exception:  # noqa: BLE001 — unreachable store degrades to no tags
        tags = {}
    for key, val in tags.items():
        nid = key[len("/mcnf/node-tags/"):]
        role, scopes, deliveries = _parse_tags(val)
        nodes.setdefault(
            nid, {"id": nid, "role": None, "scopes": [], "delivery": [], "alive": False}
        )
        nodes[nid]["role"] = role
        nodes[nid]["scopes"] = scopes
        nodes[nid]["delivery"] = deliveries
    try:
        peers = _etcd_range(endpoint, "/mesh/peers/")
    except Exception:  # noqa: BLE001
        peers = {}
    for key in peers:
        nid = key[len("/mesh/peers/"):]
        if not nid:
            continue
        nodes.setdefault(
            nid, {"id": nid, "role": None, "scopes": [], "delivery": [], "alive": False}
        )
        nodes[nid]["alive"] = True
    return list(nodes.values())


def _load_nodes():
    fixture = os.environ.get("MESH_INVENTORY_FIXTURE")
    if fixture:
        return _nodes_from_fixture(fixture)
    endpoint = _resolve_endpoint()
    if not endpoint:
        return []
    return _nodes_from_etcd(endpoint)


def build_inventory(nodes):
    """Turn a node list into the Ansible ``--list`` structure."""
    inv = {"_meta": {"hostvars": {}}, "mesh": {"hosts": []}}

    def group(name):
        return inv.setdefault(name, {"hosts": []})

    for node in nodes:
        nid = node.get("id")
        if not nid:
            continue
        role = node.get("role")
        scopes = node.get("scopes") or []
        inv["mesh"]["hosts"].append(nid)
        inv["_meta"]["hostvars"][nid] = {
            "ansible_host": node.get("ip", nid),
            "mesh_id": nid,
            "mesh_role": role,
            "mesh_scopes": scopes,
            "mesh_alive": bool(node.get("alive", False)),
        }
        if role:
            group("role_" + role)["hosts"].append(nid)
        for scope in scopes:
            group("scope_" + scope)["hosts"].append(nid)
        # The provisioned cloud VMs (tagged scope:cloud) are the cloud_vm role's
        # targets — the site.yml base-convergence group.
        if "cloud" in scopes:
            group("cloud_vm")["hosts"].append(nid)
        # WL-ARCH-006 (U13) — the Workloads delivery group selects the per-type
        # role in site.yml. A node's delivery type may arrive as a first-class
        # `delivery:<type>` tag OR as a `scope:<type>` whose value is a known
        # delivery token (so the existing scope publisher needs no change).
        deliveries = set(node.get("delivery") or [])
        deliveries |= {s for s in scopes if s in DELIVERY_TYPES}
        for delivery in deliveries:
            group("delivery_" + delivery)["hosts"].append(nid)

    for grp in inv.values():
        if isinstance(grp, dict) and "hosts" in grp:
            grp["hosts"] = sorted(set(grp["hosts"]))
    return inv


def _selftest():
    """Build the inventory from a synthetic roster and assert group membership.

    Self-contained (no live store, no fixture): proves role_/scope_/mesh/cloud_vm
    grouping AND the WL-ARCH-006 delivery_<type> groups — including that the two
    delivery tag forms (``scope:<type>`` and ``delivery:<type>``) land in the same
    group, and that an android_vm carrying only its delivery tag is NOT in
    cloud_vm. Returns 0 on all-pass, 1 on any mismatch.
    """
    nodes = [
        {"id": "lh1", "role": "lighthouse", "scopes": [], "alive": True},
        {"id": "eagle", "role": "workstation", "scopes": ["media"], "alive": True},
        # Fedora mesh VMs: scope:cloud (base) + a delivery token as a scope.
        {"id": "seat-1", "role": "workstation", "scopes": ["cloud", "desktop_vm"]},
        {"id": "svc-1", "role": "workstation", "scopes": ["cloud", "service_vm"]},
        {"id": "app-1", "role": "workstation", "scopes": ["cloud", "app_vm"]},
        {"id": "ctr-1", "role": "workstation", "scopes": ["cloud", "service_container"]},
        # Debian Cuttlefish VM: first-class delivery tag, NOT a cloud mesh node.
        {"id": "droid-1", "role": "workstation", "delivery": ["android_vm"], "alive": True},
    ]
    inv = build_inventory(nodes)

    def hosts(group):
        return inv.get(group, {}).get("hosts", [])

    checks = [
        ("role_lighthouse", ["lh1"]),
        ("scope_media", ["eagle"]),
        ("cloud_vm", ["app-1", "ctr-1", "seat-1", "svc-1"]),
        ("delivery_desktop_vm", ["seat-1"]),
        ("delivery_service_vm", ["svc-1"]),
        ("delivery_app_vm", ["app-1"]),
        ("delivery_service_container", ["ctr-1"]),
        # android via the first-class `delivery:` tag form — same group as scopes.
        ("delivery_android_vm", ["droid-1"]),
        (
            "mesh",
            ["app-1", "ctr-1", "droid-1", "eagle", "lh1", "seat-1", "svc-1"],
        ),
    ]
    failed = 0
    for group, want in checks:
        got = hosts(group)
        if got == want:
            sys.stderr.write(f"  PASS {group} = {want}\n")
        else:
            sys.stderr.write(f"  FAIL {group}: got {got} want {want}\n")
            failed += 1
    # The android_vm must NOT ride the base cloud_vm convergence.
    if "droid-1" in hosts("cloud_vm"):
        sys.stderr.write("  FAIL cloud_vm: android_vm droid-1 leaked into cloud_vm\n")
        failed += 1
    else:
        sys.stderr.write("  PASS android_vm droid-1 excluded from cloud_vm\n")
    if failed:
        sys.stderr.write(f"selftest: {failed} FAILURE(S)\n")
        return 1
    sys.stderr.write("selftest: ALL PASS\n")
    return 0


def main(argv):
    if "--selftest" in argv:
        return _selftest()
    if "--host" in argv:
        # All hostvars ride _meta in --list, so per-host returns an empty map.
        print(json.dumps({}))
        return 0
    if "--list" not in argv:
        sys.stderr.write("usage: mesh.py --list | --host <name> | --selftest\n")
        return 2
    print(json.dumps(build_inventory(_load_nodes()), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
