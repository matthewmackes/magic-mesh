#!/usr/bin/env python3
"""DATACENTER-2 / DAR-6 — OpenTofu `http` state backend backed by etcd (SUBSTRATE-V2).

Implements the OpenTofu http-backend protocol (GET/POST/DELETE + LOCK/UNLOCK) over
the etcd v3 HTTP gateway, so Tofu state + its lock live in the mesh-replicated etcd
store rather than a single host's local file. Any leader-eligible node can then
plan/apply against the same state with proper locking — no-fixed-center IaC.

State  → etcd key  /tofu/state/<name>   (raw state JSON)   [FROZEN prefix]
Lock   → etcd key  /tofu/lock/<name>     (the lock-info JSON; created atomically)

The /tofu/state/* + /tofu/lock/* prefixes are FROZEN (lock 7): they are the live
prefixes already wired into dr-backup.sh. Do NOT rename to /tofu-state.

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

Env (DAR-6 — NO dead-LAN default):
  MCNF_ETCD            comma-separated etcd v3 gateway URLs (try-next failover).
                       Resolved by DAR-1b before launch (state-backend-up.sh sources
                       automation/lib/etcd-endpoints.sh). If unset, this falls back
                       to MCNF_ETCD_ENDPOINTS_FILE; if still empty it FAILS LOUD
                       (NO dead-LAN-node default — the old .192 control node is gone).
  MCNF_ETCD_ENDPOINTS_FILE  defaults /etc/mackesd/etcd-endpoints (setup-etcd.sh).
  STATE_BACKEND_BIND   address to bind (default the detected overlay IP, NOT
                       0.0.0.0 — the overlay-only bind is load-bearing, lock 7).
  STATE_BACKEND_PORT   default 8390.
  STATE_PREFIX         (DAR-1) etcd key prefix for state. Default the CANONICAL
                       live value /tofu/state/ — the SAME string dr-backup.sh
                       ranges, so DR captures what the backend writes. Overridable
                       ONLY for a test/throwaway namespace; in production this is
                       frozen (lock 7). STATE_LOCK_PREFIX defaults /tofu/lock/.

DAR-1: the canonical prefix lives in ONE place per language — here as
CANONICAL_STATE_PREFIX, in dr-backup.sh as the same default — and both honor the
SAME STATE_PREFIX override so they can never drift (a put under STATE_PREFIX is
read by a DR range over STATE_PREFIX). Run the prefix self-test with:
  STATE_BACKEND_SELFTEST=1 python3 tofu-state-etcd.py
"""
from __future__ import annotations

import base64
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def _parse_endpoints(raw: str) -> list[str]:
    """Mirror of substrate/etcd.rs::parse_endpoints: comma/whitespace/newline
    separators, trims, drops blanks + `#` comments."""
    out: list[str] = []
    for line in raw.splitlines():
        line = line.split("#", 1)[0]
        for tok in line.replace(",", " ").replace("\t", " ").split():
            tok = tok.strip()
            if tok:
                out.append(tok)
    return out


def _resolve_endpoints() -> list[str]:
    """Resolution order (DAR-1b / design §2.2): explicit MCNF_ETCD env →
    /etc/mackesd/etcd-endpoints → FAIL LOUD. NEVER the dead .192:2379."""
    env = os.environ.get("MCNF_ETCD", "").strip()
    if env:
        eps = _parse_endpoints(env)
        if eps:
            return eps
    epfile = os.environ.get("MCNF_ETCD_ENDPOINTS_FILE", "/etc/mackesd/etcd-endpoints")
    try:
        with open(epfile, encoding="utf-8") as fh:
            eps = _parse_endpoints(fh.read())
        if eps:
            return eps
    except OSError:
        pass
    sys.stderr.write(
        "tofu-state-etcd: no etcd endpoints resolved — MCNF_ETCD unset and "
        f"{epfile} missing/empty. This node is not on the mesh etcd quorum. "
        "Run setup-etcd.sh or export MCNF_ETCD=http://<lighthouse-overlay>:2379.\n"
    )
    sys.exit(2)


# DAR-1: the canonical live prefixes (lock 7 — FROZEN). STATE_PREFIX is overridable
# so a test/throwaway run can isolate its keys, but it defaults to the same string
# dr-backup.sh ranges. Always kept with a single trailing slash.
CANONICAL_STATE_PREFIX = "/tofu/state/"
CANONICAL_LOCK_PREFIX = "/tofu/lock/"


def _norm_prefix(raw: str, default: str) -> str:
    raw = (raw or "").strip()
    if not raw:
        return default
    return raw if raw.endswith("/") else raw + "/"


# Resolved at import so both the server and the self-test share one source.
ENDPOINTS = []
PORT = int(os.environ.get("STATE_BACKEND_PORT", "8390"))
STATE_PREFIX = _norm_prefix(os.environ.get("STATE_PREFIX", ""), CANONICAL_STATE_PREFIX)
LOCK_PREFIX = _norm_prefix(os.environ.get("STATE_LOCK_PREFIX", ""), CANONICAL_LOCK_PREFIX)


def _detect_overlay() -> str:
    """The Nebula overlay IPv4 of this node (the only address the backend should
    bind — lock 7). Falls back to 127.0.0.1 (NEVER 0.0.0.0) if no overlay iface,
    so an un-meshed box can't accidentally expose state on every interface."""
    try:
        out = subprocess.run(
            ["ip", "-o", "-4", "addr", "show"],
            capture_output=True, text=True, timeout=5, check=False,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        out = ""
    for line in out.splitlines():
        f = line.split()
        # f[1] = iface, f[3] = <ip>/<prefix>
        if len(f) >= 4 and ("nebula" in f[1] or "mde-neb" in f[1]):
            return f[3].split("/")[0]
    return "127.0.0.1"


BIND = os.environ.get("STATE_BACKEND_BIND", "").strip() or _detect_overlay()


def _b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def _etcd(path: str, body: dict) -> dict:
    """POST to the etcd v3 gateway, trying each endpoint in turn (naive try-next
    failover — NOT linearizable, acceptable because Tofu re-locks before write)."""
    last: Exception | None = None
    for ep in ENDPOINTS:
        req = urllib.request.Request(
            f"{ep}{path}",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=10) as r:
                return json.loads(r.read() or b"{}")
        except (urllib.error.URLError, OSError, ValueError) as exc:
            last = exc
            continue
    raise RuntimeError(f"all etcd endpoints failed ({ENDPOINTS}): {last}")


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
    # /state/<name>  ->  <name>  (keyed under the FROZEN /tofu/state/ prefix)
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
        data = etcd_get(f"{STATE_PREFIX}{name}")
        if data is None:
            return self._send(404)  # no state yet → tofu treats as empty
        self._send(200, data)

    def do_POST(self):
        name = self._name()
        if not name:
            return self._send(400)
        body = self.rfile.read(int(self.headers.get("Content-Length", 0) or 0))
        etcd_put(f"{STATE_PREFIX}{name}", body)
        self._send(200)

    def do_DELETE(self):
        name = self._name()
        if not name:
            return self._send(400)
        etcd_del(f"{STATE_PREFIX}{name}")
        self._send(200)

    def do_LOCK(self):
        name = self._name()
        if not name:
            return self._send(400)
        info = self.rfile.read(int(self.headers.get("Content-Length", 0) or 0)) or b"{}"
        lk = f"{LOCK_PREFIX}{name}"
        if etcd_lock(lk, info):
            return self._send(200)
        held = etcd_get(lk) or b"{}"
        self._send(423, held)  # 423 Locked + the current lock info

    def do_UNLOCK(self):
        name = self._name()
        if not name:
            return self._send(400)
        etcd_del(f"{LOCK_PREFIX}{name}")
        self._send(200)


def _selftest() -> int:
    """DAR-1: offline self-test — mock etcd with an in-process dict and drive a
    PUT/GET/LOCK/UNLOCK through the REAL handler logic, asserting every key lands
    under STATE_PREFIX / LOCK_PREFIX (so a custom prefix actually relocates the
    keys and the default matches dr-backup.sh's range). Touches NO live etcd."""
    import io

    store: dict[str, bytes] = {}

    # Monkeypatch the etcd KV layer with the in-memory mock.
    global etcd_get, etcd_put, etcd_del, etcd_lock  # noqa: PLW0603
    etcd_get = lambda k: store.get(k)  # noqa: E731
    etcd_put = lambda k, v: store.__setitem__(k, v)  # noqa: E731
    etcd_del = lambda k: store.pop(k, None)  # noqa: E731

    def _mock_lock(k, info):
        if k in store:
            return False
        store[k] = info
        return True

    etcd_lock = _mock_lock  # noqa: F811

    fails: list[str] = []

    def check(cond: bool, msg: str) -> None:
        print(f"  {'PASS' if cond else 'FAIL'} {msg}")
        if not cond:
            fails.append(msg)

    # A minimal fake request driving the handler methods without a socket.
    class _Fake(Handler):
        def __init__(self, path, body=b""):  # noqa: D401  (skip BaseHTTPRequestHandler.__init__)
            self.path = path
            self.rfile = io.BytesIO(body)
            self.wfile = io.BytesIO()
            self.headers = {"Content-Length": str(len(body))}
            self._code = None
            self._body = b""

        def send_response(self, code):
            self._code = code

        def send_header(self, *a, **k):
            pass

        def end_headers(self):
            pass

    expect_state = f"{STATE_PREFIX}xen-xapi"
    expect_lock = f"{LOCK_PREFIX}xen-xapi"
    print(
        f"tofu-state-etcd selftest (mock etcd; STATE_PREFIX={STATE_PREFIX} "
        f"LOCK_PREFIX={LOCK_PREFIX})"
    )

    # POST a state body → it must land under STATE_PREFIX.
    r = _Fake("/state/xen-xapi", b'{"version":4}')
    r.do_POST()
    check(store.get(expect_state) == b'{"version":4}', f"POST writes under {expect_state}")
    check(
        not any(k.startswith("/tofu/state/") and k != expect_state for k in store)
        or STATE_PREFIX == CANONICAL_STATE_PREFIX,
        "no stray default-prefix key when STATE_PREFIX is overridden",
    )

    # GET reads it back from the SAME prefix key.
    r = _Fake("/state/xen-xapi")
    r.do_GET()
    check(r._code == 200, "GET of an existing key returns 200")
    check(r.wfile.getvalue() == b'{"version":4}', "GET returns the stored body")

    # GET of an absent name is 404.
    r = _Fake("/state/never")
    r.do_GET()
    check(r._code == 404, "GET of an absent key returns 404")

    # LOCK creates under LOCK_PREFIX; a 2nd LOCK is 423.
    r = _Fake("/state/xen-xapi", b'{"ID":"a"}')
    r.do_LOCK()
    check(r._code == 200, "first LOCK returns 200")
    check(store.get(expect_lock) == b'{"ID":"a"}', f"LOCK writes under {expect_lock}")
    r = _Fake("/state/xen-xapi", b'{"ID":"b"}')
    r.do_LOCK()
    check(r._code == 423, "second LOCK returns 423 (held)")
    check(r.wfile.getvalue() == b'{"ID":"a"}', "423 carries the held lock-info")

    # UNLOCK clears it; lock can be re-taken.
    r = _Fake("/state/xen-xapi")
    r.do_UNLOCK()
    check(expect_lock not in store, "UNLOCK clears the lock key")

    # DELETE removes the state.
    r = _Fake("/state/xen-xapi")
    r.do_DELETE()
    check(expect_state not in store, "DELETE removes the state key")

    if fails:
        print(f"selftest: {len(fails)} FAILURE(S)", file=sys.stderr)
        return 1
    print("selftest: ALL PASS")
    return 0


if __name__ == "__main__":
    if os.environ.get("STATE_BACKEND_SELFTEST"):
        sys.exit(_selftest())
    # Resolve endpoints only on the real server path (fail-loud if un-meshed).
    ENDPOINTS = _resolve_endpoints()
    print(
        f"tofu-state-etcd: {BIND}:{PORT} -> etcd {','.join(ENDPOINTS)} "
        f"(state={STATE_PREFIX} lock={LOCK_PREFIX})",
        flush=True,
    )
    ThreadingHTTPServer((BIND, PORT), Handler).serve_forever()
