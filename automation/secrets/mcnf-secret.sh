#!/usr/bin/env bash
# DATACENTER-3 / DS-8 — the mesh secret store: age-encrypted secrets in etcd.
#
# Secrets are age-encrypted to the mesh recipient and stored in etcd, so the
# control plane carries no host-local plaintext: any leader-eligible node holding
# the mesh age identity can decrypt the same secret from the replicated store.
#
#   ciphertext → etcd /mcnf/secret/<name>     recipient → etcd /mcnf/age-recipient
#
# The mesh age IDENTITY (private) is the only host-local artifact — distributed to
# eligible nodes like the mesh SSH key (~/.ssh/mackes_mesh_ed25519), kept 0600.
#
# Usage:
#   mcnf-secret.sh init                 generate the mesh age key (if absent) + publish recipient
#   mcnf-secret.sh put <name>           encrypt stdin → etcd /mcnf/secret/<name>
#   mcnf-secret.sh get <name>           decrypt etcd /mcnf/secret/<name> → stdout
#   mcnf-secret.sh list                 list stored secret names
# Env: MCNF_ETCD (http://172.20.145.192:2379), MCNF_AGE_KEY (/root/.mcnf-age-key).
set -euo pipefail
ETCD="${MCNF_ETCD:-http://172.20.145.192:2379}"
KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"

b64()  { base64 -w0; }
_put() { # <key> <value-bytes-on-stdin-as-b64-string>
  local k; k=$(printf %s "$1" | b64)
  curl -s -X POST "$ETCD/v3/kv/put" -d "{\"key\":\"$k\",\"value\":\"$2\"}" >/dev/null
}
_get() { # <key> -> raw value bytes on stdout (exit 3 if absent)
  local k; k=$(printf %s "$1" | b64)
  curl -s -X POST "$ETCD/v3/kv/range" -d "{\"key\":\"$k\"}" | python3 -c '
import sys,json,base64
d=json.load(sys.stdin); kvs=d.get("kvs")
if not kvs: sys.exit(3)
sys.stdout.buffer.write(base64.b64decode(kvs[0]["value"]))'
}
recipient() { age-keygen -y "$KEY" 2>/dev/null; }

cmd="${1:-}"
case "$cmd" in
  init)
    if [ ! -f "$KEY" ]; then (umask 077; age-keygen -o "$KEY" 2>/dev/null); echo "generated $KEY"; else echo "age key present: $KEY"; fi
    R="$(recipient)"; _put "/mcnf/age-recipient" "$(printf %s "$R" | b64)"
    echo "recipient published: $R"
    ;;
  put)
    [ -n "${2:-}" ] || { echo "usage: put <name>" >&2; exit 2; }
    R="$(recipient)"; ct="$(age -r "$R" | b64)"
    _put "/mcnf/secret/$2" "$ct"; echo "stored /mcnf/secret/$2"
    ;;
  get)
    [ -n "${2:-}" ] || { echo "usage: get <name>" >&2; exit 2; }
    _get "/mcnf/secret/$2" | age -d -i "$KEY"
    ;;
  list)
    s=$(printf %s "/mcnf/secret/" | b64); e=$(printf %s "/mcnf/secret0" | b64)
    curl -s -X POST "$ETCD/v3/kv/range" -d "{\"key\":\"$s\",\"range_end\":\"$e\",\"keys_only\":true}" | python3 -c '
import sys,json,base64
for kv in (json.load(sys.stdin).get("kvs") or []):
    print(base64.b64decode(kv["key"]).decode().split("/mcnf/secret/")[1])'
    ;;
  *) echo "usage: $0 {init|put <name>|get <name>|list}" >&2; exit 2 ;;
esac
