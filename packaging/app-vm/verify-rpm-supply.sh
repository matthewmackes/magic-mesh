#!/usr/bin/env bash
# Fail-closed admission for the governed RPM baked into the App VM image.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
BUILD_IDENTITY_VERIFY="$SCRIPT_DIR/verify-rpm-build-identity.py"
MAX_RPM_BYTES="${MCNF_APP_VM_MAX_RPM_BYTES:-1073741824}"
MAX_MANIFEST_BYTES="${MCNF_APP_VM_MAX_CANDIDATE_MANIFEST_BYTES:-1048576}"
KEY=""
SOURCE_COMMIT=""
CANDIDATE_MANIFEST=""

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
package = re.compile(r"magic-mesh [A-Za-z0-9][A-Za-z0-9._+~-]*-[A-Za-z0-9][A-Za-z0-9._+~-]*\.[A-Za-z0-9_]+\Z")
role_package = {
    "lighthouse": re.compile(r"magic-mesh-lighthouse [A-Za-z0-9][A-Za-z0-9._+~-]*-[A-Za-z0-9][A-Za-z0-9._+~-]*\.[A-Za-z0-9_]+\Z"),
    "workstation": package,
}
role_binaries = {
    "lighthouse": {"mackesd"},
    "workstation": {"mackesd", "mde-shell-egui"},
}

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
    if set(value) != {"kind", "revision", "roles", "schema_version"}:
        reject("candidate manifest top-level fields are not exact")
    if value["schema_version"] != 1 or value["kind"] != "mcnf-candidate-digest-manifest-v1":
        reject("candidate manifest identity is unsupported")
    if value["revision"] != expected_revision:
        reject("candidate manifest revision does not match the requested App VM source revision")
    roles = value["roles"]
    if not isinstance(roles, dict) or set(roles) != {"lighthouse", "workstation"}:
        reject("candidate manifest roles are not exact")
    for name, expected_binaries in role_binaries.items():
        role = roles[name]
        if not isinstance(role, dict) or set(role) != {"binaries", "package", "package_payload_sha256"}:
            reject(f"candidate manifest {name} fields are not exact")
        binaries = role["binaries"]
        if not isinstance(binaries, dict) or set(binaries) != expected_binaries:
            reject(f"candidate manifest {name} binaries are not exact")
        if any(not isinstance(item, str) or digest.fullmatch(item) is None or item == "0" * 64 for item in binaries.values()):
            reject(f"candidate manifest {name} binary digest is malformed")
        if not isinstance(role["package"], str) or role_package[name].fullmatch(role["package"]) is None:
            reject(f"candidate manifest {name} package identity is malformed")
        if (not isinstance(role["package_payload_sha256"], str)
                or digest.fullmatch(role["package_payload_sha256"]) is None
                or role["package_payload_sha256"] == "0" * 64):
            reject(f"candidate manifest {name} payload digest is malformed")
    print(roles["workstation"]["package"])
    print(roles["workstation"]["package_payload_sha256"])
except (OSError, UnicodeError, json.JSONDecodeError, TypeError, ValueError) as error:
    print(f"candidate manifest validation failed: {error}", file=sys.stderr)
    raise SystemExit(2)
PY
}

verify_supply() (
    local rpm=$1 rpmdb signature_output manifest_output rpm_output extra
    local manifest_package manifest_payload package_name version release architecture
    local payload_algorithm payload_digest
    local -a manifest_fields rpm_fields

    [[ "$MAX_RPM_BYTES" =~ ^[1-9][0-9]*$ ]] \
        || refuse 'MCNF_APP_VM_MAX_RPM_BYTES must be a positive integer'
    [[ "$MAX_MANIFEST_BYTES" =~ ^[1-9][0-9]*$ ]] \
        || refuse 'MCNF_APP_VM_MAX_CANDIDATE_MANIFEST_BYTES must be a positive integer'
    if [[ ! "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] \
        || [ "$SOURCE_COMMIT" = 0000000000000000000000000000000000000000 ]; then
        refuse '--source-commit must be a non-null 40-character lowercase Git revision'
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

    manifest_output=$(manifest_identity) \
        || refuse 'candidate manifest does not bind this RPM candidate to the requested source revision'
    mapfile -t manifest_fields <<<"$manifest_output"
    [ "${#manifest_fields[@]}" -eq 2 ] \
        || refuse 'candidate manifest returned an ambiguous workstation identity'
    manifest_package=${manifest_fields[0]}
    manifest_payload=${manifest_fields[1]}

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
    grep -Eiq 'signature.*key ID [0-9a-f]{8,16}.*: OK([[:space:]]|$)' <<<"$signature_output" \
        || refuse "RPM verification did not report a governed signature: $rpm"

    rpm_output=$(rpm -qp --qf \
        '%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\n%{PAYLOADDIGESTALGO}\t%{PAYLOADDIGEST}\n' \
        -- "$rpm" 2>/dev/null) \
        || refuse "RPM package metadata is invalid: $rpm"
    mapfile -t rpm_fields <<<"$rpm_output"
    [ "${#rpm_fields[@]}" -eq 2 ] \
        || refuse "RPM returned ambiguous package metadata: $rpm"
    IFS=$'\t' read -r package_name version release architecture extra <<<"${rpm_fields[0]}"
    if [ -n "${extra:-}" ] || [ -z "$package_name" ] || [ -z "$version" ] \
        || [ -z "$release" ] || [ -z "$architecture" ]; then
        refuse "RPM package identity metadata is incomplete: $rpm"
    fi
    [ "$package_name" = magic-mesh ] \
        || refuse "local RPM package name must be exactly magic-mesh (got: $package_name)"
    [ "$package_name $version-$release.$architecture" = "$manifest_package" ] \
        || refuse 'RPM NEVRA does not match the source-revision candidate manifest'

    IFS=$'\t' read -r payload_algorithm payload_digest extra <<<"${rpm_fields[1]}"
    [ -z "${extra:-}" ] || refuse "RPM payload identity metadata is ambiguous: $rpm"
    case "$payload_algorithm" in 8|SHA256|sha256) ;; *) refuse 'RPM payload digest algorithm is not SHA-256' ;; esac
    [[ "$payload_digest" =~ ^[0-9a-fA-F]{64}$ ]] \
        || refuse "RPM payload digest metadata is malformed: $rpm"
    [ "${payload_digest,,}" = "$manifest_payload" ] \
        || refuse 'RPM payload digest does not match the source-revision candidate manifest'

    # The caller-supplied candidate manifest is only a consistency record. The
    # authenticated revision authority is the exact compile-time BuildInfo in
    # both signed RPM binaries. Stream only those two members and parse their
    # ELF bytes without extracting paths or executing package code.
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
printf '%s\t%s\t%s\t%s\n%s\t%s\n' "$name" 12.1.6 33 x86_64 8 "$payload"
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
    chmod 0755 "$fixture/bin/rpm" "$fixture/bin/rpmkeys" "$fixture/bin/rpm2cpio" "$fixture/bin/cpio"

    manifest="$fixture/candidate-manifest.json"
    stale_manifest="$fixture/stale-candidate-manifest.json"
    hostile_manifest="$fixture/hostile-candidate-manifest.json"
    forged_manifest="$fixture/forged-current-candidate-manifest.json"
    python3 - "$manifest" "$revision" <<'PY'
import json, sys
path, revision = sys.argv[1:]
value = {
    "kind": "mcnf-candidate-digest-manifest-v1",
    "revision": revision,
    "roles": {
        "lighthouse": {
            "binaries": {"mackesd": "c" * 64},
            "package": "magic-mesh-lighthouse 12.1.6-33.x86_64",
            "package_payload_sha256": "d" * 64,
        },
        "workstation": {
            "binaries": {"mackesd": "e" * 64, "mde-shell-egui": "f" * 64},
            "package": "magic-mesh 12.1.6-33.x86_64",
            "package_payload_sha256": "a" * 64,
        },
    },
    "schema_version": 1,
}
with open(path, "w", encoding="utf-8") as stream:
    json.dump(value, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
with open(sys.argv[1].replace("candidate-manifest", "stale-candidate-manifest"), "w", encoding="utf-8") as stream:
    json.dump({**value, "revision": "1123456789abcdef0123456789abcdef01234567"}, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
with open(sys.argv[1].replace("candidate-manifest", "hostile-candidate-manifest"), "w", encoding="utf-8") as stream:
    json.dump({**value, "unsupported": True}, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
forged = json.loads(json.dumps(value))
forged["roles"]["workstation"]["package_payload_sha256"] = "b" * 64
with open(sys.argv[1].replace("candidate-manifest", "forged-current-candidate-manifest"), "w", encoding="utf-8") as stream:
    json.dump(forged, stream, sort_keys=True, separators=(",", ":"))
    stream.write("\n")
PY
    chmod 0444 "$manifest" "$stale_manifest" "$hostile_manifest" "$forged_manifest"

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

    supply=(--key "$fixture/key" --source-commit "$revision" --candidate-manifest "$manifest")
    PATH="$fixture/bin:/usr/bin:/bin" "$script" "${supply[@]}" "$good" >/dev/null
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
