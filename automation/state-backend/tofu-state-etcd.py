#!/usr/bin/env python3
"""DATACENTER-2 — OpenTofu `http` state backend backed by etcd (SUBSTRATE-V2).

Implements the OpenTofu http-backend protocol (GET/POST/DELETE + LOCK/UNLOCK) over
the etcd v3 HTTP gateway, so Tofu state + its lock live in the mesh-replicated etcd
store rather than a single host's local file. Any leader-eligible node can then
plan/apply against the same state with proper locking — no-fixed-center IaC.

State  → etcd key  /tofu/state/<name>   (raw state JSON)
Lock   → etcd key  /tofu/lock/<name>    (the lock-info JSON; created atomically)

Stateless itself: all durable state is in etcd, so the service can run on any node
(or several behind a VIP). Configure a workspace's backend with:

    terraform {
      backend "http" {
        address        = "http://<host>:8390/state/<name>"
        lock_address   = "http://<host>:8390/state/<name>"
        unlock_address = "http://<host>:8390/state/<name>"
        lock_method    = "LOCK"
        unlock_method  = "UNLOCK"
      }
    }

Env: MCNF_ETCD (default http://172.20.145.192:2379), STATE_BACKEND_PORT (8390).
"""
from __future__ import annotations

import base64
import json
import os
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ETCD = os.environ.get("MCNF_ETCD", "http://172.20.145.192:2379")
PORT = int(os.environ.get("STATE_BACKEND_PORT", "8390"))


def _b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def _etcd(path: str, body: dict) -> dict:
    req = urllib.request.Request(
        f"{ETCD}{path}", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read() or b"{}")


def etcd_get(key: str) -> bytes | None:
    r = _etcd("/v3/kv/range", {"key": _b64(key.encode())})
    kvs = r.get("kvs")
    if not kvs:
        return None
    return base64.b64decode(kvs[0]["value"])


def etcd_put(key: str, val: bytes) -> None:
    _etcd("/v3/kv/put", {"key": _b64(key.encode()), "value": _b64(val)})


def etcd_del(key: str) -> None:
    _etcd("/v3/kv/deleterange", {"key": _b64(key.encode())})


def etcd_lock(key: str, info: bytes) -> bool:
    """Atomic create-if-absent via an etcd txn. True if the lock was acquired."""
    k = _b64(key.encode())
    txn = {
        "compare": [{"key": k, "target": "CREATE", "create_revision": "0"}],
        "success": [{"requestPut": {"key": k, "value": _b64(info)}}],
    }
    return bool(_etcd("/v3/kv/txn", txn).get("succeeded"))


def _state_key(path: str) -> str | None:
    # /state/<name>  ->  /tofu/state/<name>
    parts = path.lstrip("/").split("/", 1)
    if len(parts) != 2 or parts[0] != "state" or not parts[1]:
        return None
    return parts[1].split("?")[0]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):  # quiet
        pass

    def _name(self):
        return _state_key(self.path)

    def _send(self, code: int, body: bytes = b""):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_GET(self):
        name = self._name()
        if not name:
            return self._send(400)
        data = etcd_get(f"/tofu/state/{name}")
        if data is None:
            return self._send(404)  # no state yet → tofu treats as empty
        self._send(200, data)

    def do_POST(self):
        name = self._name()
        if not name:
            return self._send(400)
        body = self.rfile.read(int(self.headers.get("Content-Length", 0) or 0))
        etcd_put(f"/tofu/state/{name}", body)
        self._send(200)

    def do_DELETE(self):
        name = self._name()
        if not name:
            return self._send(400)
        etcd_del(f"/tofu/state/{name}")
        self._send(200)

    def do_LOCK(self):
        name = self._name()
        if not name:
            return self._send(400)
        info = self.rfile.read(int(self.headers.get("Content-Length", 0) or 0)) or b"{}"
        lk = f"/tofu/lock/{name}"
        if etcd_lock(lk, info):
            return self._send(200)
        held = etcd_get(lk) or b"{}"
        self._send(423, held)  # 423 Locked + the current lock info

    def do_UNLOCK(self):
        name = self._name()
        if not name:
            return self._send(400)
        etcd_del(f"/tofu/lock/{name}")
        self._send(200)


if __name__ == "__main__":
    print(f"tofu-state-etcd: :{PORT} -> etcd {ETCD}", flush=True)
    ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
