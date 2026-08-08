#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="$REPO_ROOT/install-helpers/mcnf-overlay-identity-collision-guard.py"
TEST_ROOT="$(mktemp -d)"
trap 'sudo -n rm -rf -- "$TEST_ROOT"' EXIT
sudo -n chown root:root "$TEST_ROOT"
sudo -n chmod 0700 "$TEST_ROOT"
sudo -n install -d -o root -g root -m 0700 "$TEST_ROOT/runtime"

AUTH_KEY="$TEST_ROOT/auth.key"
MACHINE="$TEST_ROOT/machine-id"
BOOT_A="$TEST_ROOT/boot-a"
BOOT_B="$TEST_ROOT/boot-b"
CERTIFICATE="$TEST_ROOT/host.crt"
CERT_TEMPLATE="$TEST_ROOT/host-template.crt"
PARSER="$TEST_ROOT/nebula-cert"
PARSER_MODE="$TEST_ROOT/parser-mode"

sudo -n python3 - "$AUTH_KEY" "$MACHINE" "$BOOT_A" "$BOOT_B" "$CERT_TEMPLATE" <<'PY'
import json, pathlib, sys
key, machine, boot_a, boot_b, certificate = map(pathlib.Path, sys.argv[1:])
key.write_bytes(b"K" * 32)
machine.write_text("13579bdf2468ace013579bdf2468ace0\n")
boot_a.write_text("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa\n")
boot_b.write_text("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb\n")
certificate.write_text(json.dumps({
    "details": {
        "name": "peer:copied-seat",
        "issuer": "c" * 64,
        "networks": ["10.42.0.44/17"],
    },
    "fingerprint": "a" * 64,
}, separators=(",", ":")))
PY
sudo -n cp "$CERT_TEMPLATE" "$CERTIFICATE"
sudo -n sh -c "printf '%s\n' normal > '$PARSER_MODE'"
sudo -n chmod 0600 "$AUTH_KEY" "$MACHINE" "$BOOT_A" "$BOOT_B" \
    "$CERTIFICATE" "$CERT_TEMPLATE" "$PARSER_MODE"

sudo -n tee "$PARSER" >/dev/null <<EOF
#!/bin/sh
set -eu
[ "\$#" -eq 4 ] && [ "\$1" = print ] && [ "\$2" = -json ] && [ "\$3" = -path ]
[ "\$4" != '$CERTIFICATE' ]
case "\$(cat '$PARSER_MODE')" in
    normal)
        printf '%s\n' 'attacker-swapped-live-path' > '$CERTIFICATE'
        cat -- "\$4"
        ;;
    oversized) while :; do printf '%065536d' 0; done ;;
    hung) while :; do :; done ;;
    *) exit 64 ;;
esac
EOF
sudo -n chmod 0700 "$PARSER"

sudo -n python3 - "$TEST_ROOT" "$AUTH_KEY" <<'PY'
import hashlib, hmac, json, pathlib, sys, time

root=pathlib.Path(sys.argv[1]); key=pathlib.Path(sys.argv[2]).read_bytes()
now=time.time_ns()//1_000_000
fingerprint="a"*64
prefix="/mesh/overlay-identity-claims/v1/"
snapshot_schema="mcnf.overlay-identity-claim-snapshot.v2"
commit_schema="mcnf.overlay-identity-claim-snapshot-commitment.v1"
boot_raw=b"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
machine_a=b"13579bdf2468ace013579bdf2468ace0"
machine_b=b"02468ace13579bdf02468ace13579bdf"
boot_b=b"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()

def auth(domain, payload):
    return hmac.new(key, domain+b"\0"+canonical(payload), hashlib.sha256).hexdigest()

def envelope(schema, domain, payload, tag=None):
    return {"schema":schema,"payload":payload,"authentication":{
        "algorithm":"hmac-sha256","key_id":"local-overlay-claim-snapshot-hmac-v1",
        "tag":tag or auth(domain,payload)}}

def digest(domain, raw):
    return hashlib.sha256(domain+b"\0"+fingerprint.encode()+b"\0"+raw).hexdigest()

def entry(machine_raw, boot_raw, lease, revision=73):
    machine=digest(b"mcnf-overlay-machine-claimant-v1",machine_raw)
    boot=digest(b"mcnf-overlay-boot-claimant-v1",boot_raw)
    claim={"schema_version":1,"nebula_node_id":"peer:copied-seat",
           "nebula_name":"peer:copied-seat","nebula_address":"10.42.0.44",
           "certificate_fingerprint":fingerprint,"machine_claimant_digest":machine,
           "boot_claimant_digest":boot}
    return {"key":f"{prefix}{fingerprint}/{machine}/{boot}","lease_id":str(lease),
            "mod_revision":revision,"claim":claim}

boot_digest=hmac.new(key,b"mcnf-overlay-claim-snapshot-producer-boot-v1\0"+boot_raw,
                     hashlib.sha256).hexdigest()

def write_case(name, claims, *, generated=now, valid=None, mutate=None, commit_tag=None):
    if valid is None: valid=generated+30_000
    payload={"schema":snapshot_schema,"generated_at_unix_ms":generated,
             "valid_until_unix_ms":valid,"producer_boot_digest":boot_digest,
             "source":{"kind":"etcd-linearizable-lease-range","namespace":prefix,
                       "cluster_id":"9001","member_id":"42","etcd_revision":73,
                       "raft_term":7},"claims":sorted(claims,key=lambda item:item["key"])}
    if mutate is not None: mutate(payload)
    snapshot=envelope(snapshot_schema,b"mcnf-overlay-claim-snapshot-v2",payload)
    commitment_payload={"schema":commit_schema,
        "snapshot_tag":commit_tag or snapshot["authentication"]["tag"],
        "producer_boot_digest":boot_digest,"etcd_revision":73,
        "generated_at_unix_ms":generated}
    commitment=envelope(commit_schema,b"mcnf-overlay-claim-snapshot-commitment-v1",
                        commitment_payload)
    (root/f"{name}.snapshot").write_bytes(canonical(snapshot)+b"\n")
    (root/f"{name}.commit").write_bytes(canonical(commitment)+b"\n")

local=entry(machine_a,boot_raw,101)
other=entry(machine_b,boot_b,202)
write_case("safe",[local])
write_case("copied",[local,other])
write_case("stale",[local],generated=now-60_000,valid=now-30_000)
write_case("replay",[local],commit_tag="d"*64)
write_case("unknown",[local],mutate=lambda payload:payload.update({"unexpected":True}))
write_case("wrong-source",[local],mutate=lambda payload:payload["source"].update(
    {"namespace":"/mesh/peers/"}))

mismatch=entry(machine_a,boot_raw,101)
mismatch["key"]=mismatch["key"][:-1]+("0" if mismatch["key"][-1]!="0" else "1")
write_case("key-mismatch",[mismatch])

bad_auth=json.loads((root/"safe.snapshot").read_text())
bad_auth["authentication"]["tag"]="e"*64
(root/"bad-auth.snapshot").write_bytes(canonical(bad_auth)+b"\n")
(root/"bad-auth.commit").write_bytes((root/"safe.commit").read_bytes())

(root/"duplicate-json.snapshot").write_text('{"schema":"x","schema":"y"}')
(root/"duplicate-json.commit").write_bytes((root/"safe.commit").read_bytes())

for path in root.glob("*.snapshot"):
    path.chmod(0o600)
for path in root.glob("*.commit"):
    path.chmod(0o600)
PY

passes=0
run_case() {
    local name="$1" expected="$2" fixture="$3" boot="${4:-$BOOT_A}" mode="${5:-normal}"
    local output rc
    sudo -n cp "$CERT_TEMPLATE" "$CERTIFICATE"
    sudo -n chmod 0600 "$CERTIFICATE"
    sudo -n sh -c "printf '%s\n' '$mode' > '$PARSER_MODE'"
    set +e
    output="$(sudo -n "$GUARD" \
        --certificate "$CERTIFICATE" \
        --fallback-certificate "$CERTIFICATE" \
        --snapshot "$TEST_ROOT/$fixture.snapshot" \
        --commitment "$TEST_ROOT/$fixture.commit" \
        --auth-key "$AUTH_KEY" \
        --machine-id "$MACHINE" \
        --boot-id "$boot" \
        --nebula-cert-bin "$PARSER" \
        --runtime-dir "$TEST_ROOT/runtime" \
        --max-snapshot-age-seconds 30 \
        --certificate-parser-timeout-seconds 1 2>&1)"
    rc=$?
    set -e
    if [ "$rc" -ne "$expected" ]; then
        printf 'FAIL %-24s expected=%s actual=%s output=%s\n' "$name" "$expected" "$rc" "$output" >&2
        exit 1
    fi
    if grep -Eq 'peer:copied-seat|13579bdf|aaaaaaaa-aaaa|KKKKKK|PRIVATE KEY' <<<"$output"; then
        printf 'FAIL %-24s diagnostic disclosed identity or credential material: %s\n' "$name" "$output" >&2
        exit 1
    fi
    if sudo -n find "$TEST_ROOT/runtime" -mindepth 1 -print -quit | grep -q .; then
        printf 'FAIL %-24s left parser staging material\n' "$name" >&2
        exit 1
    fi
    passes=$((passes+1))
}

run_case safe-snapshot          0  safe
run_case copied-collision      20  copied
run_case stale-snapshot        22  stale
run_case previous-boot         22  safe "$BOOT_B"
run_case replayed-snapshot     25  replay
run_case bad-authentication    23  bad-auth
run_case unknown-signed-field  21  unknown
run_case wrong-source          23  wrong-source
run_case key-value-mismatch    21  key-mismatch
run_case duplicate-json-key    21  duplicate-json
run_case hostile-parser-size   21  safe "$BOOT_A" oversized
run_case hostile-parser-hang   24  safe "$BOOT_A" hung

# Final-link and mode attacks fail before authentication is consumed.
sudo -n ln -s "$TEST_ROOT/safe.snapshot" "$TEST_ROOT/symlink.snapshot"
sudo -n cp "$TEST_ROOT/safe.commit" "$TEST_ROOT/symlink.commit"
sudo -n chmod 0600 "$TEST_ROOT/symlink.commit"
run_case symlink-snapshot      23  symlink
sudo -n cp "$TEST_ROOT/safe.snapshot" "$TEST_ROOT/writable.snapshot"
sudo -n cp "$TEST_ROOT/safe.commit" "$TEST_ROOT/writable.commit"
sudo -n chmod 0622 "$TEST_ROOT/writable.snapshot"
sudo -n chmod 0600 "$TEST_ROOT/writable.commit"
run_case writable-snapshot     23  writable

if sudo -n sh -c \
    'grep -Fq "13579bdf2468ace013579bdf2468ace0" "$1"/*.snapshot || grep -Fq "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa" "$1"/*.snapshot' \
    sh "$TEST_ROOT"; then
    printf 'FAIL raw machine/boot id escaped into an authenticated snapshot\n' >&2
    exit 1
fi

python3 -m py_compile "$GUARD"
printf 'PASS authenticated overlay collision guard hostile fixtures: %d cases; admitted-byte parser=proven; raw ids=absent\n' "$passes"
