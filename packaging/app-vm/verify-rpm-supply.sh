#!/usr/bin/env bash
# Fail-closed admission for the governed RPM baked into the App VM image.
set -euo pipefail

# Fedora 44 exposes the package payload identity as PAYLOADSHA256* RPM tags.

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
BUILD_IDENTITY_VERIFY="$SCRIPT_DIR/verify-rpm-build-identity.py"
MAX_RPM_BYTES="${MCNF_APP_VM_MAX_RPM_BYTES:-1073741824}"
MAX_MANIFEST_BYTES="${MCNF_APP_VM_MAX_CANDIDATE_MANIFEST_BYTES:-1048576}"
KEY=""
SOURCE_COMMIT=""
CANDIDATE_MANIFEST=""
EXPECTED_SIGNING_FINGERPRINT=""

refuse() {
    echo "FATAL: App VM RPM supply refused: $*" >&2
    exit 2
}

regular_bounded_file() { # $1 = label, $2 = path, $3 = maximum bytes
    local label=$1 path=$2 maximum=$3 mode size
    [ -e "$path" ] || refuse "$label is missing: $path"
    [ ! -L "$path" ] || refuse "$label must not be a symlink: $path"
    [ -f "$path" ] || refuse "$label must be a regular file: $path"
    read -r mode size < <(stat -Lc '%a %s' -- "$path") \
        || refuse "$label metadata cannot be read: $path"
    [[ "$mode" =~ ^[0-7]{3,4}$ ]] || refuse "$label has an invalid mode: $path"
    (( (8#$mode & 0022) == 0 )) \
        || refuse "$label must not be group/other writable: $path"
    if [[ ! "$size" =~ ^[0-9]+$ ]] || [ "$size" -le 0 ] || [ "$size" -gt "$maximum" ]; then
        refuse "$label size must be between 1 and $maximum bytes: $path"
    fi
}

manifest_identity() {
    python3 - "$CANDIDATE_MANIFEST" "$SOURCE_COMMIT" <<'PY'
import json
import re
import sys

path, expected_revision = sys.argv[1:]
digest = re.compile(r"[0-9a-f]{64}\Z")
revision = re.compile(r"[0-9a-f]{40}\Z")
fingerprint = re.compile(r"[0-9A-F]{40,64}\Z")
nevra = re.compile(r"magic-mesh-(?:[0-9]+:)?[A-Za-z0-9][A-Za-z0-9._+~:-]*-[A-Za-z0-9][A-Za-z0-9._+~:-]*\.[A-Za-z0-9_]+\Z")

def reject(message):
    raise ValueError(message)

def exact_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            reject(f"candidate manifest contains duplicate field {key}")
        value[key] = item
    return value

try:
    with open(path, "r", encoding="utf-8") as stream:
        value = json.load(stream, object_pairs_hook=exact_object)
    if set(value) != {"app_vm_target_identity", "artifact", "build_identity", "kind", "schema_version", "signing_fingerprint"}:
        reject("candidate manifest top-level fields are not exact")
    if value["schema_version"] != 2 or value["kind"] != "mcnf-app-vm-rpm-candidate-manifest":
        reject("candidate manifest identity is unsupported")
    if value["app_vm_target_identity"] != "mcnf-app-vm/wayland-standard-v1":
        reject("candidate manifest does not target the immutable App VM profile")
    build = value["build_identity"]
    if not isinstance(build, dict) or set(build) != {"source_revision"}:
        reject("candidate manifest build identity fields are not exact")
    if not isinstance(build["source_revision"], str) or revision.fullmatch(build["source_revision"]) is None or build["source_revision"] != expected_revision:
        reject("candidate manifest revision does not match the requested App VM source revision")
    artifact = value["artifact"]
    if not isinstance(artifact, dict) or set(artifact) != {"nevra", "payload_sha256", "rpm_sha256"}:
        reject("candidate manifest artifact fields are not exact")
    if not isinstance(artifact["nevra"], str) or nevra.fullmatch(artifact["nevra"]) is None:
        reject("candidate manifest NEVRA is malformed")
    for name in ("payload_sha256", "rpm_sha256"):
        if not isinstance(artifact[name], str) or digest.fullmatch(artifact[name]) is None or artifact[name] == "0" * 64:
            reject(f"candidate manifest {name} is malformed")
    signer = value["signing_fingerprint"]
    if not isinstance(signer, str) or fingerprint.fullmatch(signer) is None:
        reject("candidate manifest signing fingerprint is malformed")
    print(artifact["nevra"])
    print(artifact["payload_sha256"])
    print(artifact["rpm_sha256"])
    print(signer)
except (OSError, UnicodeError, json.JSONDecodeError, TypeError, ValueError) as error:
    print(f"candidate manifest validation failed: {error}", file=sys.stderr)
    raise SystemExit(2)
PY
}

verify_supply() (
    local rpm=$1 rpmdb signature_output manifest_output rpm_output extra
    local manifest_nevra manifest_payload manifest_rpm manifest_signer package_name epoch version release architecture
    local payload_algorithm payload_digest actual_rpm_digest signature_key_id governed_fingerprints resolved_signer rpm_nevra epoch_prefix
    local -a manifest_fields rpm_fields signature_ids

    [[ "$MAX_RPM_BYTES" =~ ^[1-9][0-9]*$ ]] \
        || refuse 'MCNF_APP_VM_MAX_RPM_BYTES must be a positive integer'
    [[ "$MAX_MANIFEST_BYTES" =~ ^[1-9][0-9]*$ ]] \
        || refuse 'MCNF_APP_VM_MAX_CANDIDATE_MANIFEST_BYTES must be a positive integer'
    if [[ ! "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] \
        || [ "$SOURCE_COMMIT" = 0000000000000000000000000000000000000000 ]; then
        refuse '--source-commit must be a non-null 40-character lowercase Git revision'
    fi
    if [ -n "$EXPECTED_SIGNING_FINGERPRINT" ] \
        && [[ ! "$EXPECTED_SIGNING_FINGERPRINT" =~ ^[0-9A-F]{40,64}$ ]]; then
        refuse '--expected-signing-fingerprint must be one full uppercase fingerprint'
    fi
    regular_bounded_file 'governed RPM key' "$KEY" 1048576
    regular_bounded_file 'candidate manifest' "$CANDIDATE_MANIFEST" "$MAX_MANIFEST_BYTES"
    regular_bounded_file 'local RPM' "$rpm" "$MAX_RPM_BYTES"
    regular_bounded_file 'RPM build-identity verifier' "$BUILD_IDENTITY_VERIFY" 1048576
    [ -x "$BUILD_IDENTITY_VERIFY" ] || refuse 'RPM build-identity verifier is not executable'

    command -v python3 >/dev/null 2>&1 || refuse 'python3 is required'
    command -v rpm >/dev/null 2>&1 || refuse 'rpm is required'
    command -v rpmkeys >/dev/null 2>&1 || refuse 'rpmkeys is required'
    command -v rpm2cpio >/dev/null 2>&1 || refuse 'rpm2cpio is required'
    command -v cpio >/dev/null 2>&1 || refuse 'cpio is required'
    command -v gpg >/dev/null 2>&1 || refuse 'gpg is required'
    command -v sha256sum >/dev/null 2>&1 || refuse 'sha256sum is required'

    manifest_output=$(manifest_identity) \
        || refuse 'candidate manifest does not bind this RPM candidate to the requested source revision'
    mapfile -t manifest_fields <<<"$manifest_output"
    [ "${#manifest_fields[@]}" -eq 4 ] \
        || refuse 'candidate manifest returned an ambiguous App VM RPM identity'
    manifest_nevra=${manifest_fields[0]}
    manifest_payload=${manifest_fields[1]}
    manifest_rpm=${manifest_fields[2]}
    manifest_signer=${manifest_fields[3]}

    rpmdb=$(mktemp -d)
    trap 'rm -rf "$rpmdb"' EXIT
    rpm --dbpath "$rpmdb" --initdb >/dev/null \
        || refuse 'temporary RPM database initialization failed'
    rpm --dbpath "$rpmdb" --import "$KEY" >/dev/null \
        || refuse 'governed RPM key import failed'

    # Authenticate the complete package before asking rpm/cpio to inspect any
    # identity or payload bytes. The temporary database contains only the
    # governed project key.
    signature_output=$(rpmkeys --dbpath "$rpmdb" --checksig --verbose -- "$rpm" 2>&1) \
        || refuse "RPM signature is missing, invalid, or not made by the governed key: $rpm"
    grep -Eiq 'signature.*key (ID|fingerprint):?[[:space:]]+[0-9a-f]{8,64}[[:space:]]*:[[:space:]]*OK([[:space:]]|$)' <<<"$signature_output" \
        || refuse "RPM verification did not report a governed signature: $rpm"
    mapfile -t signature_ids < <(grep -Eio 'key (ID|fingerprint):?[[:space:]]+[0-9a-f]{8,64}[[:space:]]*:[[:space:]]*OK' <<<"$signature_output" \
        | sed -E 's/^key (ID|fingerprint):?[[:space:]]+([0-9a-fA-F]+).*/\U\2/')
    [ "${#signature_ids[@]}" -eq 1 ] || refuse 'RPM signature did not yield exactly one signing key ID'
    signature_key_id=${signature_ids[0]}
    governed_fingerprints=$(gpg --batch --with-colons --show-keys --fingerprint --fingerprint "$KEY" 2>/dev/null \
        | awk -F: '$1 == "fpr" { print toupper($10) }') \
        || refuse 'governed key fingerprints cannot be inspected'
    resolved_signer=$(awk -v suffix="$signature_key_id" 'substr($0, length($0)-length(suffix)+1) == suffix { print }' <<<"$governed_fingerprints")
    [ "$(sed '/^$/d' <<<"$resolved_signer" | wc -l)" -eq 1 ] \
        || refuse 'RPM signing key ID does not resolve to one governed full fingerprint'
    [ "$resolved_signer" = "$manifest_signer" ] \
        || refuse 'RPM governed signing fingerprint does not match the candidate manifest'
    if [ -n "$EXPECTED_SIGNING_FINGERPRINT" ]; then
        [ "$resolved_signer" = "$EXPECTED_SIGNING_FINGERPRINT" ] \
            || refuse 'RPM governed signing fingerprint does not match the explicitly expected release signer'
    fi
    actual_rpm_digest=$(sha256sum -- "$rpm" | awk '{print $1}') \
        || refuse 'RPM whole-file digest cannot be measured'
    [ "$actual_rpm_digest" = "$manifest_rpm" ] \
        || refuse 'RPM whole-file SHA-256 does not match the candidate manifest'

    local payload_query
    if rpm --querytags 2>/dev/null | grep -Fxq 'PAYLOADSHA256'; then
        payload_query='%{PAYLOADSHA256ALGO}\t%{PAYLOADSHA256}'
    else
        payload_query='%{PAYLOADDIGESTALGO}\t%{PAYLOADDIGEST}'
    fi
    rpm_output=$(rpm -qp --qf \
        "%{NAME}\\t%{EPOCHNUM}\\t%{VERSION}\\t%{RELEASE}\\t%{ARCH}\\n${payload_query}\\n" \
        -- "$rpm" 2>/dev/null) \
        || refuse "RPM package metadata is invalid: $rpm"
    mapfile -t rpm_fields <<<"$rpm_output"
    [ "${#rpm_fields[@]}" -eq 2 ] \
        || refuse "RPM returned ambiguous package metadata: $rpm"
    IFS=$'\t' read -r package_name epoch version release architecture extra <<<"${rpm_fields[0]}"
    if [ -n "${extra:-}" ] || [ -z "$package_name" ] || [ -z "$version" ] \
        || [ -z "$release" ] || [ -z "$architecture" ]; then
        refuse "RPM package identity metadata is incomplete: $rpm"
    fi
    [ "$package_name" = magic-mesh ] \
        || refuse "local RPM package name must be exactly magic-mesh (got: $package_name)"
    [[ "$epoch" =~ ^[0-9]+$ ]] || refuse 'RPM epoch metadata is malformed'
    epoch_prefix=''
    [ "$epoch" = 0 ] || epoch_prefix="$epoch:"
    rpm_nevra="$package_name-$epoch_prefix$version-$release.$architecture"
    [ "$rpm_nevra" = "$manifest_nevra" ] \
        || refuse 'RPM NEVRA does not match the source-revision candidate manifest'

    IFS=$'\t' read -r payload_algorithm payload_digest extra <<<"${rpm_fields[1]}"
    [ -z "${extra:-}" ] || refuse "RPM payload identity metadata is ambiguous: $rpm"
    case "$payload_algorithm" in 8|SHA256|sha256) ;; *) refuse 'RPM payload digest algorithm is not SHA-256' ;; esac
    [[ "$payload_digest" =~ ^[0-9a-fA-F]{64}$ ]] \
        || refuse "RPM payload digest metadata is malformed: $rpm"
    [ "${payload_digest,,}" = "$manifest_payload" ] \
        || refuse 'RPM payload digest does not match the source-revision candidate manifest'

    # The manifest binds the exact signed RPM bytes, governed signer, immutable
    # App VM target, NEVRA, payload, and source revision. Re-attest the revision
    # against compile-time BuildInfo in both signed binaries. Stream only those
    # members and parse their ELF bytes without extracting or executing them.
    for member in ./usr/bin/mackesd ./usr/bin/mde-shell-egui; do
        if ! rpm2cpio "$rpm" \
            | cpio -i --quiet --to-stdout -- "$member" \
            | "$BUILD_IDENTITY_VERIFY" \
                --source-commit "$SOURCE_COMMIT" \
                --package-version "$version" \
                --member "$member"; then
            refuse "signed RPM payload does not carry the requested source revision in $member"
        fi
    done
)

self_test() {
    local fixture script revision good unsigned wrong mutable old manifest stale_manifest hostile_manifest forged_manifest failure
    fixture=$(mktemp -d)
    SELF_TEST_FIXTURE=$fixture
    trap 'rm -rf "$SELF_TEST_FIXTURE"' EXIT
    script=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")
    revision=0123456789abcdef0123456789abcdef01234567
    mkdir -p "$fixture/bin"
    printf '%s\n' 'governed public key fixture' > "$fixture/key"
    chmod 0444 "$fixture/key"

    cat > "$fixture/bin/rpm" <<'EOF'
#!/usr/bin/env bash
set -eu
case " $* " in
    *' --initdb '*) exit 0 ;;
    *' --import '*) exit 0 ;;
esac
last=''
for item in "$@"; do last=$item; done
case "$(basename -- "$last")" in
    wrong.rpm) name=not-magic-mesh; payload=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ;;
    old.rpm) name=magic-mesh; payload=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ;;
    *) name=magic-mesh; payload=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ;;
esac
printf '%s\t%s\t%s\t%s\t%s\n%s\t%s\n' "$name" 0 12.1.6 33 x86_64 8 "$payload"
EOF
    cat > "$fixture/bin/gpg" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' 'pub:-:4096:1:00000000D0921C73::::::sc:::::::'
printf '%s\n' 'fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD0921C73:'
EOF
    cat > "$fixture/bin/rpmkeys" <<'EOF'
#!/usr/bin/env bash
set -eu
last=''
for item in "$@"; do last=$item; done
case "$(basename -- "$last")" in
    unsigned.rpm) printf '%s\n' 'digests OK'; exit 1 ;;
    *) printf '%s\n' 'Header V4 RSA/SHA256 Signature, key ID d0921c73: OK' ;;
esac
EOF
    cat > "$fixture/bin/rpm2cpio" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$(basename -- "${1:?RPM path required}")"
EOF
    cat > "$fixture/bin/cpio" <<'EOF'
#!/usr/bin/env bash
set -eu
read -r rpm_name
member=''
for item in "$@"; do member=$item; done
case "$member" in ./usr/bin/mackesd|./usr/bin/mde-shell-egui) ;; *) exit 2 ;; esac
case "$rpm_name" in old.rpm) revision=1123456789abcdef0123456789abcdef01234567 ;; *) revision=0123456789abcdef0123456789abcdef01234567 ;; esac
printf '\177ELF12.1.6Construct%s2026-08-11dev\n' "$revision"
EOF
    chmod 0755 "$fixture/bin/rpm" "$fixture/bin/rpmkeys" "$fixture/bin/rpm2cpio" "$fixture/bin/cpio" "$fixture/bin/gpg"

    good="$fixture/candidate.rpm"
    unsigned="$fixture/unsigned.rpm"
    wrong="$fixture/wrong.rpm"
    mutable="$fixture/mutable.rpm"
    old="$fixture/old.rpm"
    printf '%s\n' payload > "$good"
    cp "$good" "$unsigned"
    cp "$good" "$wrong"
    cp "$good" "$mutable"
    cp "$good" "$old"
    chmod 0444 "$good" "$unsigned" "$wrong" "$old"
    chmod 0666 "$mutable"

    manifest="$fixture/candidate-manifest.json"
    stale_manifest="$fixture/stale-candidate-manifest.json"
    hostile_manifest="$fixture/hostile-candidate-manifest.json"
    forged_manifest="$fixture/forged-current-candidate-manifest.json"
    python3 - "$manifest" "$revision" "$good" <<'PY'
import hashlib, json, sys
path, revision, rpm = sys.argv[1:]
value = {
    "app_vm_target_identity": "mcnf-app-vm/wayland-standard-v1",
    "artifact": {
        "nevra": "magic-mesh-12.1.6-33.x86_64",
        "payload_sha256": "a" * 64,
        "rpm_sha256": hashlib.sha256(open(rpm, "rb").read()).hexdigest(),
    },
    "build_identity": {"source_revision": revision},
    "kind": "mcnf-app-vm-rpm-candidate-manifest",
    "schema_version": 2,
    "signing_fingerprint": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD0921C73",
}
with open(path, "w", encoding="utf-8") as stream:
    json.dump(value, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
stale = json.loads(json.dumps(value))
stale["build_identity"]["source_revision"] = "1123456789abcdef0123456789abcdef01234567"
with open(sys.argv[1].replace("candidate-manifest", "stale-candidate-manifest"), "w", encoding="utf-8") as stream:
    json.dump(stale, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
with open(sys.argv[1].replace("candidate-manifest", "hostile-candidate-manifest"), "w", encoding="utf-8") as stream:
    json.dump({**value, "unsupported": True}, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
forged = json.loads(json.dumps(value))
forged["artifact"]["payload_sha256"] = "b" * 64
with open(sys.argv[1].replace("candidate-manifest", "forged-current-candidate-manifest"), "w", encoding="utf-8") as stream:
    json.dump(forged, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
PY
    chmod 0444 "$manifest" "$stale_manifest" "$hostile_manifest" "$forged_manifest"

    supply=(--key "$fixture/key" --source-commit "$revision" --candidate-manifest "$manifest")
    PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$good" >/dev/null
    PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" \
        --expected-signing-fingerprint AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD0921C73 "$good" >/dev/null
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" \
        --expected-signing-fingerprint BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB "$good" >/dev/null 2>&1; then
        refuse 'self-test admitted an explicitly mismatched release signing fingerprint'
    fi
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$old" >/dev/null 2>&1; then
        refuse 'self-test admitted an older correctly signed RPM with a substituted payload digest'
    fi
    if failure=$(PATH="$fixture/bin:/usr/bin:/bin" "$script" --key "$fixture/key" \
        --source-commit "$revision" --candidate-manifest "$forged_manifest" "$old" 2>&1); then
        refuse 'self-test admitted an older signed RPM through a forged current-revision manifest'
    fi
    [[ "$failure" == *'signed RPM payload does not carry the requested source revision'* ]] \
        || refuse 'self-test forged-manifest case did not reach authenticated payload revision admission'
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" --key "$fixture/key" --source-commit "$revision" \
        --candidate-manifest "$stale_manifest" "$good" >/dev/null 2>&1; then
        refuse 'self-test admitted a candidate manifest from an older source revision'
    fi
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" --key "$fixture/key" --source-commit "$revision" \
        --candidate-manifest "$hostile_manifest" "$good" >/dev/null 2>&1; then
        refuse 'self-test admitted a candidate manifest with unsupported fields'
    fi
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$unsigned" >/dev/null 2>&1; then
        refuse 'self-test admitted an unsigned RPM'
    fi
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$wrong" >/dev/null 2>&1; then
        refuse 'self-test admitted the wrong package name from authoritative RPM metadata'
    fi
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$mutable" >/dev/null 2>&1; then
        refuse 'self-test admitted a group/other-writable RPM'
    fi
    if MCNF_APP_VM_MAX_RPM_BYTES=4 PATH="$fixture/bin:/usr/bin:/bin" \
        "$script" "${supply[@]}" "$good" >/dev/null 2>&1; then
        refuse 'self-test admitted an oversized RPM'
    fi
    ln -s "$good" "$fixture/symlink.rpm"
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$fixture/symlink.rpm" >/dev/null 2>&1; then
        refuse 'self-test admitted a symlinked RPM'
    fi
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$good" "$good" >/dev/null 2>&1; then
        refuse 'self-test admitted more than one local RPM'
    fi

    echo 'App VM RPM supply self-test passed (signature + manifest consistency + signed-payload source revision)'
}

if [ "${1:-}" = --self-test ]; then
    [ "$#" -eq 1 ] || refuse '--self-test takes no other arguments'
    self_test
    exit 0
fi

while [ "$#" -gt 0 ]; do
    case "$1" in
        --key) KEY="${2:?--key needs a path}"; shift 2 ;;
        --source-commit) SOURCE_COMMIT="${2:?--source-commit needs a revision}"; shift 2 ;;
        --candidate-manifest) CANDIDATE_MANIFEST="${2:?--candidate-manifest needs a path}"; shift 2 ;;
        --expected-signing-fingerprint) EXPECTED_SIGNING_FINGERPRINT="${2:?--expected-signing-fingerprint needs a fingerprint}"; shift 2 ;;
        --) shift; break ;;
        -*) refuse "unknown option: $1" ;;
        *) break ;;
    esac
done

[ -n "$KEY" ] || refuse '--key is required'
[ -n "$SOURCE_COMMIT" ] || refuse '--source-commit is required'
[ -n "$CANDIDATE_MANIFEST" ] || refuse '--candidate-manifest is required'
[ "$#" -eq 1 ] || refuse 'exactly one local magic-mesh RPM is required'
verify_supply "$1"
