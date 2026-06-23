#!/usr/bin/env bash
# etcd-lib.sh — minimal etcd v3 HTTP-gateway client (curl + base64), shared by the
# FARM-AUTO-3 queue pieces. No etcdctl needed on the build VMs — just curl/python3.
# Source it: . etcd-lib.sh ; then use etcd_put / etcd_range_keys / etcd_claim / …
# Endpoint: $MCNF_ETCD (default the control host).
MCNF_ETCD="${MCNF_ETCD:-http://172.20.145.192:2379}"

_b64()  { printf '%s' "$1" | base64 -w0; }
_b64d() { base64 -d 2>/dev/null; }
# prefix range_end = key with last byte incremented (etcd convention for "prefix").
_prefix_end() { python3 -c "import sys;b=sys.argv[1].encode();print(__import__('base64').b64encode(b[:-1]+bytes([b[-1]+1])).decode())" "$1"; }

etcd_put() { # key value [leaseID]
  local body; body=$(python3 -c "import json,sys;print(json.dumps({'key':sys.argv[1],'value':sys.argv[2],'lease':sys.argv[3]}))" "$(_b64 "$1")" "$(_b64 "$2")" "${3:-0}")
  curl -s -X POST "$MCNF_ETCD/v3/kv/put" -d "$body" >/dev/null
}
etcd_get() { # key -> value on stdout (empty if absent)
  curl -s -X POST "$MCNF_ETCD/v3/kv/range" -d "{\"key\":\"$(_b64 "$1")\"}" \
    | python3 -c "import json,sys,base64;d=json.load(sys.stdin);print(base64.b64decode(d['kvs'][0]['value']).decode()) if d.get('kvs') else ''" 2>/dev/null
}
etcd_range_keys() { # prefix -> keys (decoded), one per line
  curl -s -X POST "$MCNF_ETCD/v3/kv/range" -d "{\"key\":\"$(_b64 "$1")\",\"range_end\":\"$(_prefix_end "$1")\",\"keys_only\":true}" \
    | python3 -c "import json,sys,base64;d=json.load(sys.stdin);[print(base64.b64decode(k['key']).decode()) for k in d.get('kvs',[])]" 2>/dev/null
}
etcd_del() { curl -s -X POST "$MCNF_ETCD/v3/kv/deleterange" -d "{\"key\":\"$(_b64 "$1")\"}" >/dev/null; }
etcd_lease() { # ttl -> leaseID
  curl -s -X POST "$MCNF_ETCD/v3/lease/grant" -d "{\"TTL\":\"${1:-600}\",\"ID\":0}" \
    | python3 -c "import json,sys;print(json.load(sys.stdin).get('ID',''))" 2>/dev/null
}
# etcd_claim <lockkey> <owner> <leaseID> -> 0 if WE won the lock (create-if-absent txn).
etcd_claim() {
  local lk="$1" owner="$2" lease="$3" body resp
  body=$(python3 -c "
import json,sys,base64
lk=base64.b64encode(sys.argv[1].encode()).decode()
val=base64.b64encode(sys.argv[2].encode()).decode()
print(json.dumps({'compare':[{'key':lk,'result':'EQUAL','target':'CREATE','create_revision':'0'}],
 'success':[{'requestPut':{'key':lk,'value':val,'lease':sys.argv[3]}}],'failure':[]}))" "$lk" "$owner" "$lease")
  resp=$(curl -s -X POST "$MCNF_ETCD/v3/kv/txn" -d "$body")
  printf '%s' "$resp" | python3 -c "import json,sys;sys.exit(0 if json.load(sys.stdin).get('succeeded') else 1)" 2>/dev/null
}
