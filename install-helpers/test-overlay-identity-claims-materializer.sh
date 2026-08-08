#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MATERIALIZER="$REPO_ROOT/install-helpers/mcnf-overlay-identity-claims-materializer.py"
GUARD="$REPO_ROOT/install-helpers/mcnf-overlay-identity-collision-guard.py"
MATERIALIZER_UNIT="$REPO_ROOT/packaging/systemd/mcnf-overlay-identity-claims-materializer.service"
GUARD_DROPIN="$REPO_ROOT/packaging/systemd/nebula.service.d/05-overlay-identity-collision-guard.conf"
RPM_PAYLOAD_VERIFY="$REPO_ROOT/install-helpers/verify-rpm-payload.sh"

TEST_ROOT="$(mktemp -d)"
trap 'sudo -n rm -rf -- "$TEST_ROOT"' EXIT
sudo -n chown root:root "$TEST_ROOT"
sudo -n chmod 0700 "$TEST_ROOT"
for directory in state run runtime; do
    sudo -n install -d -o root -g root -m 0700 "$TEST_ROOT/$directory"
done

AUTH_KEY="$TEST_ROOT/auth.key"
BOOT_A="$TEST_ROOT/boot-a"
BOOT_B="$TEST_ROOT/boot-b"
MACHINE_A="$TEST_ROOT/machine-a"
ENDPOINTS="$TEST_ROOT/endpoints"
RESPONSE="$TEST_ROOT/response.json"
MODE="$TEST_ROOT/mode"
ETCDCTL="$TEST_ROOT/etcdctl"
CERT_PARSER="$TEST_ROOT/nebula-cert"
CERTIFICATE="$TEST_ROOT/host.crt"
SNAPSHOT="$TEST_ROOT/state/snapshot.json"
COMMITMENT="$TEST_ROOT/run/commitment.json"

sudo -n python3 - "$AUTH_KEY" <<'PY'
import pathlib, sys
pathlib.Path(sys.argv[1]).write_bytes(b"K" * 32)
PY
sudo -n sh -c "printf '%s\n' 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' > '$BOOT_A'"
sudo -n sh -c "printf '%s\n' 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb' > '$BOOT_B'"
sudo -n sh -c "printf '%s\n' '13579bdf2468ace013579bdf2468ace0' > '$MACHINE_A'"
sudo -n sh -c "printf '%s\n' 'http://10.42.0.1:2379' > '$ENDPOINTS'"
sudo -n sh -c "printf '%s\n' normal > '$MODE'"
sudo -n chmod 0600 "$AUTH_KEY" "$BOOT_A" "$BOOT_B" "$MACHINE_A" "$ENDPOINTS" "$MODE"

sudo -n tee "$ETCDCTL" >/dev/null <<EOF
#!/bin/sh
set -eu
[ "\$#" -eq 7 ]
[ "\$1" = '--endpoints=http://10.42.0.1:2379' ]
[ "\$2" = '--command-timeout=1s' ]
[ "\$3" = '--write-out=json' ]
[ "\$4" = get ]
[ "\$5" = '/mesh/overlay-identity-claims/v1/' ]
[ "\$6" = '--prefix' ]
[ "\$7" = '--consistency=l' ]
case "\$(cat '$MODE')" in
    normal) cat '$RESPONSE' ;;
    oversized) while :; do printf '%065536d' 0; done ;;
    hung) while :; do :; done ;;
    *) exit 64 ;;
esac
EOF
sudo -n chmod 0700 "$ETCDCTL"

sudo -n tee "$CERT_PARSER" >/dev/null <<'EOF'
#!/bin/sh
set -eu
[ "$#" -eq 4 ] && [ "$1" = print ] && [ "$2" = -json ] && [ "$3" = -path ]
cat -- "$4"
EOF
sudo -n chmod 0700 "$CERT_PARSER"

sudo -n python3 - "$CERTIFICATE" <<'PY'
import json, pathlib, sys
cert = {
    "details": {
        "name": "peer:copied-seat",
        "issuer": "c" * 64,
        "networks": ["10.42.0.44/17"],
    },
    "fingerprint": "a" * 64,
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(cert, separators=(",", ":")))
PY
sudo -n chmod 0600 "$CERTIFICATE"

write_fixture() {
    local revision="$1" shape="$2"
    sudo -n python3 - "$RESPONSE" "$revision" "$shape" <<'PY'
import base64, hashlib, json, pathlib, sys

target = pathlib.Path(sys.argv[1]); revision = int(sys.argv[2]); shape = sys.argv[3]
prefix = "/mesh/overlay-identity-claims/v1/"
fingerprint = "a" * 64
machine_a_raw = b"13579bdf2468ace013579bdf2468ace0"
machine_b_raw = b"02468ace13579bdf02468ace13579bdf"
boot_a_raw = b"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
boot_b_raw = b"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"

def digest(domain, raw):
    return hashlib.sha256(domain + b"\0" + fingerprint.encode() + b"\0" + raw).hexdigest()

def claim(machine_raw, boot_raw, lease):
    machine = digest(b"mcnf-overlay-machine-claimant-v1", machine_raw)
    boot = digest(b"mcnf-overlay-boot-claimant-v1", boot_raw)
    value = {
        "schema_version": 1,
        "nebula_node_id": "peer:copied-seat",
        "nebula_name": "peer:copied-seat",
        "nebula_address": "10.42.0.44",
        "certificate_fingerprint": fingerprint,
        "machine_claimant_digest": machine,
        "boot_claimant_digest": boot,
    }
    key = f"{prefix}{fingerprint}/{machine}/{boot}"
    return key, value, lease

rows = [claim(machine_a_raw, boot_a_raw, 101)]
if shape == "copied":
    rows.append(claim(machine_b_raw, boot_b_raw, 202))
kvs = []
for key, value, lease in rows:
    kvs.append({
        "key": base64.b64encode(key.encode()).decode(),
        "create_revision": str(revision - 1),
        "mod_revision": str(revision),
        "version": "1",
        "value": base64.b64encode(json.dumps(value, separators=(",", ":")).encode()).decode(),
        "lease": str(lease),
    })
response = {
    "header": {
        "cluster_id": "9001",
        "member_id": "42",
        "revision": str(revision),
        "raft_term": "7",
    },
    "kvs": kvs,
    "count": str(len(kvs)),
}
target.write_text(json.dumps(response, separators=(",", ":")))
PY
    sudo -n chmod 0600 "$RESPONSE"
}

materialize() {
    local validity="${1:-15}"
    sudo -n "$MATERIALIZER" \
        --endpoints-file "$ENDPOINTS" \
        --output "$SNAPSHOT" \
        --commitment "$COMMITMENT" \
        --auth-key "$AUTH_KEY" \
        --boot-id "$BOOT_A" \
        --etcdctl-bin "$ETCDCTL" \
        --validity-seconds "$validity" \
        --command-timeout-seconds 1
}

guard() {
    local boot="${1:-$BOOT_A}"
    sudo -n "$GUARD" \
        --certificate "$CERTIFICATE" \
        --fallback-certificate "$CERTIFICATE" \
        --snapshot "$SNAPSHOT" \
        --commitment "$COMMITMENT" \
        --auth-key "$AUTH_KEY" \
        --machine-id "$MACHINE_A" \
        --boot-id "$boot" \
        --nebula-cert-bin "$CERT_PARSER" \
        --runtime-dir "$TEST_ROOT/runtime" \
        --certificate-parser-timeout-seconds 1
}

expect_rc() {
    local expected="$1"; shift
    local output rc
    set +e
    output="$("$@" 2>&1)"
    rc=$?
    set -e
    if [ "$rc" -ne "$expected" ]; then
        printf 'FAIL expected=%s actual=%s output=%s\n' "$expected" "$rc" "$output" >&2
        exit 1
    fi
    if grep -Eq '13579bdf|aaaaaaaa-aaaa|KKKKKK|PRIVATE KEY|peer:copied-seat' <<<"$output"; then
        printf 'FAIL diagnostic disclosed claimant or credential material: %s\n' "$output" >&2
        exit 1
    fi
}

# Two distinct machine/boot claimants sharing one copied public identity remain
# separate lease-backed entries, and the local first claimant detects the other.
write_fixture 73 copied
materialize >/dev/null
sudo -n python3 - "$SNAPSHOT" <<'PY'
import json, sys
value=json.load(open(sys.argv[1])); claims=value["payload"]["claims"]
assert len(claims) == 2
assert len({entry["key"] for entry in claims}) == 2
assert len({entry["claim"]["machine_claimant_digest"] for entry in claims}) == 2
assert len({entry["claim"]["boot_claimant_digest"] for entry in claims}) == 2
PY
expect_rc 20 guard
if sudo -n grep -Fq '13579bdf2468ace013579bdf2468ace0' "$SNAPSHOT" || \
   sudo -n grep -Fq 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' "$SNAPSHOT"; then
    printf 'FAIL snapshot exposed a raw machine/boot id\n' >&2
    exit 1
fi

# Same claimant is safe, while replaying its old snapshot against a newer
# current-boot commitment is rejected explicitly.
write_fixture 74 safe
materialize >/dev/null
expect_rc 0 guard
sudo -n cp "$SNAPSHOT" "$TEST_ROOT/old-snapshot.json"
write_fixture 75 safe
materialize >/dev/null
sudo -n cp "$SNAPSHOT" "$TEST_ROOT/new-snapshot.json"
sudo -n cp "$TEST_ROOT/old-snapshot.json" "$SNAPSHOT"
sudo -n chmod 0600 "$SNAPSHOT"
expect_rc 25 guard
sudo -n cp "$TEST_ROOT/new-snapshot.json" "$SNAPSHOT"
sudo -n chmod 0600 "$SNAPSHOT"
expect_rc 0 guard

# The /run commitment and producer boot binding are honest across reboot.
expect_rc 22 guard "$BOOT_B"
sudo -n rm -f "$COMMITMENT"
expect_rc 23 guard
write_fixture 76 safe
materialize 1 >/dev/null
sleep 2
expect_rc 22 guard

# Authentication catches byte mutation. Invalid or hostile etcd responses do
# not replace the last authenticated pair.
write_fixture 77 safe
materialize >/dev/null
sudo -n python3 - "$SNAPSHOT" <<'PY'
import json, sys
value=json.load(open(sys.argv[1])); value["payload"]["source"]["etcd_revision"] += 1
json.dump(value, open(sys.argv[1], "w"), sort_keys=True, separators=(",", ":"))
PY
expect_rc 23 guard
materialize >/dev/null
before_snapshot="$(sudo -n sha256sum "$SNAPSHOT")"
before_commitment="$(sudo -n sha256sum "$COMMITMENT")"
sudo -n python3 - "$RESPONSE" <<'PY'
import json, sys
value=json.load(open(sys.argv[1])); value["unexpected"]=True
json.dump(value, open(sys.argv[1], "w"), separators=(",", ":"))
PY
expect_rc 21 materialize
[ "$before_snapshot" = "$(sudo -n sha256sum "$SNAPSHOT")" ]
[ "$before_commitment" = "$(sudo -n sha256sum "$COMMITMENT")" ]

sudo -n sh -c "printf '%s\n' oversized > '$MODE'"
expect_rc 24 materialize
sudo -n sh -c "printf '%s\n' hung > '$MODE'"
expect_rc 24 materialize
sudo -n sh -c "printf '%s\n' normal > '$MODE'"

# The post-overlay producer remains non-activating, and the Nebula drop-in is
# inert until a separate current-boot pre-overlay transport exists.
grep -Fqx 'After=network-online.target nebula.service etcd.service' "$MATERIALIZER_UNIT"
grep -Fq 'ACTIVATION_BLOCKER=pre-nebula-current-authority-transport-unavailable' \
    "$MATERIALIZER_UNIT"
grep -Fq 'ACTIVATION_BLOCKER=pre-nebula-current-authority-transport-unavailable' \
    "$GUARD_DROPIN"
if grep -Eq '^\[Install\]$|^(WantedBy|RequiredBy)=|^Before=.*nebula\.service' "$MATERIALIZER_UNIT"; then
    printf 'FAIL producer acquired an activation/pre-Nebula edge\n' >&2
    exit 1
fi
if grep -Eq '^\[(Unit|Service)\]|^ExecStartPre=' "$GUARD_DROPIN"; then
    printf 'FAIL guard drop-in became active before authority transport exists\n' >&2
    exit 1
fi

# Package shape is part of this prerequisite: base, Server, and Lighthouse all
# ship the same disabled producer + inert guard payload. The focused RPM gate
# also pins the dedicated 0700 state leaf used by both Python defaults.
"$RPM_PAYLOAD_VERIFY" overlay-claims-package >/dev/null

python3 -m py_compile "$MATERIALIZER" "$GUARD"
printf 'PASS authenticated overlay claim materializer: copied=distinct collision=blocked replay=blocked reboot=blocked stale=blocked hostile-etcd=bounded activation=absent\n'
