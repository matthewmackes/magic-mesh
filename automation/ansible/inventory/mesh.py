#!/usr/bin/env python3
"""WL-ARCH-001 Phase B — mesh-derived dynamic inventory for the cloud backend.

The configure leg (Ansible) has NO static host files (decided-stack #7). This
script inventory reads the LIVE mesh roster and emits Ansible hosts + groups:

  * node ids + role/scope tags from etcd ``/mcnf/node-tags/<id>`` (the SEC-003
    published tags: ``role:<r>`` / ``scope:<s>``),
  * live membership from etcd ``/mesh/peers/<hostname>`` (the mackesd peer set;
    a present key = an alive keepalive lease),

and groups every host by role (``role_<r>``), scope (``scope_<s>``), a ``mesh``
group (all), and ``cloud_vm`` (nodes tagged ``scope:cloud`` — the VMs the
``infra/tofu/cloud`` root provisions, which the ``cloud_vm`` role configures).
Each host's ``ansible_host`` is its mesh hostname, reachable over the Nebula
overlay via mesh DNS.

Roster source resolution (first that yields):
  1. ``MESH_INVENTORY_FIXTURE=<path>`` — a JSON roster fixture (tests / offline
     ``ansible-inventory --list``; NEVER touches a live store),
  2. etcd at ``MCNF_ETCD`` or the first endpoint in
     ``/etc/mackesd/etcd-endpoints``.

Fail-soft: an unreachable store with no fixture yields an EMPTY-but-valid
inventory (honest — never a crash, never fabricated hosts).

Fixture shape:
  {"nodes": [
     {"id": "lh1",   "role": "lighthouse",  "scopes": [],        "alive": true},
     {"id": "eagle", "role": "workstation", "scopes": ["media"], "alive": true},
     {"id": "vm-a",  "role": "workstation", "scopes": ["cloud"], "ip": "10.44.0.10"}
  ]}
"""

import base64
import json
import os
import sys
import urllib.request


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
    """Split a node-tags value (one ``role:``/``scope:`` selector per line)."""
    role, scopes = None, []
    for line in raw.splitlines():
        line = line.strip()
        if line.startswith("role:"):
            role = line[len("role:"):]
        elif line.startswith("scope:"):
            scopes.append(line[len("scope:"):])
    return role, scopes


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
        role, scopes = _parse_tags(val)
        nodes.setdefault(nid, {"id": nid, "role": None, "scopes": [], "alive": False})
        nodes[nid]["role"] = role
        nodes[nid]["scopes"] = scopes
    try:
        peers = _etcd_range(endpoint, "/mesh/peers/")
    except Exception:  # noqa: BLE001
        peers = {}
    for key in peers:
        nid = key[len("/mesh/peers/"):]
        if not nid:
            continue
        nodes.setdefault(nid, {"id": nid, "role": None, "scopes": [], "alive": False})
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
        # targets — the site.yml convergence group.
        if "cloud" in scopes:
            group("cloud_vm")["hosts"].append(nid)

    for grp in inv.values():
        if isinstance(grp, dict) and "hosts" in grp:
            grp["hosts"] = sorted(set(grp["hosts"]))
    return inv


def main(argv):
    if "--host" in argv:
        # All hostvars ride _meta in --list, so per-host returns an empty map.
        print(json.dumps({}))
        return 0
    if "--list" not in argv:
        sys.stderr.write("usage: mesh.py --list | --host <name>\n")
        return 2
    print(json.dumps(build_inventory(_load_nodes()), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
