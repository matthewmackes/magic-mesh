#!/usr/bin/env bash
# WL-CRIT-006 — restart-safe orchestration for the first full release boundary.
set -euo pipefail
umask 077

ROOT=${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
SOURCE_VERIFY=${MCNF_RELEASE_SOURCE_VERIFY:-$ROOT/install-helpers/source-revision-receipt.sh}
PREFLIGHT=${MCNF_RELEASE_PREFLIGHT:-$ROOT/install-helpers/release-input-preflight.sh}
FARM=${MCNF_RELEASE_FARM:-$ROOT/install-helpers/xcp-build.sh}
DERIVATIVES=${MCNF_RELEASE_DERIVATIVES:-$ROOT/install-helpers/build-release-derivative-images.sh}
PLAN=${MCNF_RELEASE_PLAN:-$ROOT/install-helpers/produce-release-output-plan.py}
COLLECTOR=${MCNF_RELEASE_COLLECTOR:-$ROOT/install-helpers/collect-release-outputs.py}
RPM_QUERY=${MCNF_RELEASE_RPM_QUERY:-rpm}

refuse() { printf 'first-full-release: REFUSED: %s\n' "$*" >&2; exit 2; }
usage() {
  cat <<'EOF'
Usage:
  run-first-full-release.sh prepare --source-revision REV --source-epoch EPOCH \
    --target-fedora 44 --preflight-arguments JSON --output DIR
  run-first-full-release.sh resume --source-revision REV --target-fedora 44 --handoff DIR \
    --derivative-arguments JSON --plan-input JSON --output DIR

Argument files contain one JSON array of argv strings. Prepare creates only an
immutable, unsigned, promotion-forbidden operator handoff. Resume accepts only
operator-signed candidates, verified derivatives, and all seven canonical
roles; it never signs, publishes, promotes, or runs live acceptance.
EOF
}

revision='' epoch='' target_fedora='' preflight_file='' handoff='' derivative_file=''
plan_input='' output=''
declare -a preflight_args=() derivative_args=()
mode=${1:-}
[[ "$mode" == prepare || "$mode" == resume ]] || { usage >&2; exit 2; }
shift
while (($#)); do
  case "$1" in
    --source-revision) revision=${2:-}; shift 2 ;;
    --source-epoch) epoch=${2:-}; shift 2 ;;
    --target-fedora) target_fedora=${2:-}; shift 2 ;;
    --preflight-arguments) preflight_file=${2:-}; shift 2 ;;
    --handoff) handoff=${2:-}; shift 2 ;;
    --derivative-arguments) derivative_file=${2:-}; shift 2 ;;
    --plan-input) plan_input=${2:-}; shift 2 ;;
    --output) output=${2:-}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) refuse "unknown or incomplete argument: $1" ;;
  esac
done

[[ "$revision" =~ ^[0-9a-f]{40}$ && "$revision" != 0000000000000000000000000000000000000000 ]] \
  || refuse 'source revision must be one non-null lowercase Git object ID'
[[ "$target_fedora" == 44 ]] || refuse 'the first full release target must be Fedora 44'
[[ -n "$output" && ! -e "$output" && ! -L "$output" ]] || refuse 'output must be one absent path'
output_parent=$(dirname -- "$output")
[[ -d "$output_parent" && ! -L "$output_parent" ]] || refuse 'output parent must be an existing real directory'
parent_mode=$(stat -Lc '%a' -- "$output_parent")
(( (8#$parent_mode & 0022) == 0 )) || refuse 'output parent must be private'

regular() {
  local label=$1 path=$2 maximum=$3 mode_bits size links
  [[ -f "$path" && ! -L "$path" ]] || refuse "$label must be a regular non-symlink file"
  read -r mode_bits size links < <(stat -Lc '%a %s %h' -- "$path") || refuse "$label metadata is unavailable"
  (( links == 1 && size > 0 && size <= maximum && (8#$mode_bits & 0022) == 0 )) \
    || refuse "$label must be single-link, bounded, and not group/other writable"
}

rpm_identity() { # role rpm
  local role=$1 rpm_path=$2 value name nevra algorithm payload extra expected
  value=$("$RPM_QUERY" -qp --qf '%{NAME}\t%{NEVRA}\t%{PAYLOADDIGESTALGO}\t%{PAYLOADDIGEST}\n' -- "$rpm_path") \
    || refuse "$role RPM identity query failed"
  IFS=$'\t' read -r name nevra algorithm payload extra <<<"$value"
  [[ -z ${extra:-} && "$algorithm" == 8 && "$payload" =~ ^[0-9a-fA-F]{64}$ ]] \
    || refuse "$role RPM does not expose one SHA-256 payload identity"
  case "$role" in
    workstation-rpm) expected=magic-mesh ;;
    server-rpm) expected=magic-mesh-server ;;
    lighthouse-rpm) expected=magic-mesh-lighthouse ;;
    *) refuse 'unknown RPM role' ;;
  esac
  [[ "$name" == "$expected" && "$nevra" == "$expected-"* ]] \
    || refuse "$role RPM NEVRA does not match its exact role"
  printf '%s\t%s\t%s\t%s\n' "$role" "$nevra" "$algorithm" "${payload,,}"
}

argument_value() { # array-name option
  local -n values=$1
  local option=$2 index found=''
  for ((index=0; index<${#values[@]}; index++)); do
    if [[ ${values[index]} == "$option" ]]; then
      [[ -z "$found" && $((index + 1)) -lt ${#values[@]} ]] \
        || refuse "derivative arguments contain duplicate or incomplete $option"
      found=${values[index + 1]}
    fi
  done
  [[ -n "$found" ]] || refuse "derivative arguments omit $option"
  printf '%s\n' "$found"
}

load_arguments() { # file array-name reserved-options...
  local file=$1 name=$2 encoded reserved item
  shift 2
  local -n loaded=$name
  regular 'argument file' "$file" 1048576
  encoded=$work/argv.$RANDOM
  python3 - "$file" >"$encoded" <<'PY' || refuse 'argument JSON is malformed'
import json, sys
try:
    value = json.load(open(sys.argv[1], encoding="utf-8"))
except (OSError, UnicodeError, json.JSONDecodeError) as exc:
    raise SystemExit(f"invalid argument JSON: {exc}")
if not isinstance(value, list) or len(value) > 128:
    raise SystemExit("argument JSON must be a bounded array")
for item in value:
    if not isinstance(item, str) or not item or len(item) > 4096 or "\0" in item:
        raise SystemExit("argument JSON contains an invalid argv item")
    sys.stdout.buffer.write(item.encode() + b"\0")
PY
  mapfile -d '' -t "$name" <"$encoded"
  rm -f -- "$encoded"
  for item in "${loaded[@]}"; do
    for reserved in "$@"; do
      [[ "$item" != "$reserved" ]] || refuse "argument JSON may not override $reserved"
    done
  done
}

work=$(mktemp -d --tmpdir="$output_parent" .first-full-release.XXXXXX)
cleanup() { chmod -R u+rwX -- "$work" 2>/dev/null || true; rm -rf -- "$work"; }
trap cleanup EXIT
chmod 0700 "$work"

publish_dir() {
  local staged=$1 destination=$2
  [[ ! -e "$destination" && ! -L "$destination" ]] || refuse 'output appeared during phase execution'
  # Keep directories owner-writable through rename: overlay-backed farm homes
  # reject renaming a read-only source directory. Files are immutable before
  # publication; directory write bits are removed immediately afterward.
  find "$staged" -type f -exec chmod a-w -- {} +
  mv -T --no-clobber -- "$staged" "$destination" || refuse 'atomic no-replace phase publication failed'
  [[ ! -e "$staged" ]] || refuse 'atomic no-replace phase publication lost a race'
  find "$destination" -depth -type d -exec chmod a-w -- {} +
}

assert_source_identity() {
  local receipt
  receipt=$("$SOURCE_VERIFY" --repo "$ROOT") \
    || refuse 'cannot resolve an exact clean source receipt'
  [[ "$receipt" == "$revision"$'\t'"$epoch" ]] \
    || refuse 'clean checkout identity differs from the requested release revision or epoch'
}

if [[ "$mode" == prepare ]]; then
  [[ "$epoch" =~ ^[1-9][0-9]{0,11}$ ]] || refuse 'source epoch must be one bounded positive decimal'
  [[ -n "$preflight_file" && -z "$handoff$derivative_file$plan_input" ]] \
    || refuse 'prepare requires only --preflight-arguments'
  load_arguments "$preflight_file" preflight_args --source-revision --source-epoch

  # Resolve rather than syntax-check the receipt. Repeat immediately before
  # each cut so a dirty or moving checkout cannot cross a build boundary.
  assert_source_identity
  "$PREFLIGHT" --source-revision "$revision" --source-epoch "$epoch" "${preflight_args[@]}" >/dev/null \
    || refuse 'release-input preflight rejected the pinned inputs'

  full_pull=$work/full-pull
  server_pull=$work/server-pull
  mkdir -m 0700 "$full_pull" "$server_pull"
  assert_source_identity
  MCNF_BUILD_SLOT=${MCNF_RELEASE_FULL_SLOT:-first-release-full} "$FARM" container-rpm --full "$target_fedora"
  MCNF_BUILD_SLOT=${MCNF_RELEASE_FULL_SLOT:-first-release-full} MCNF_BUILD_ARTIFACTS=$full_pull \
    "$FARM" pull 'target-f43/generate-rpm/*.rpm'
  assert_source_identity
  MCNF_BUILD_SLOT=${MCNF_RELEASE_SERVER_SLOT:-first-release-server} "$FARM" container-rpm --server "$target_fedora"
  MCNF_BUILD_SLOT=${MCNF_RELEASE_SERVER_SLOT:-first-release-server} MCNF_BUILD_ARTIFACTS=$server_pull \
    "$FARM" pull 'target-f43/generate-rpm/*.rpm'

  shopt -s nullglob
  workstation=("$full_pull"/magic-mesh-[0-9]*.rpm)
  lighthouse=("$full_pull"/magic-mesh-lighthouse-*.rpm)
  server=("$server_pull"/magic-mesh-server-*.rpm)
  (( ${#workstation[@]} == 1 && ${#lighthouse[@]} == 1 && ${#server[@]} == 1 )) \
    || refuse 'farm cuts did not yield exactly one Workstation, Lighthouse, and Server RPM'

  staged=$work/handoff
  mkdir -m 0700 "$staged"
  install -m 0400 -- "${workstation[0]}" "$staged/workstation-unsigned.rpm"
  install -m 0400 -- "${server[0]}" "$staged/server-unsigned.rpm"
  install -m 0400 -- "${lighthouse[0]}" "$staged/lighthouse-unsigned.rpm"
  identities=$work/rpm-identities.tsv
  rpm_identity workstation-rpm "$staged/workstation-unsigned.rpm" >"$identities"
  rpm_identity server-rpm "$staged/server-unsigned.rpm" >>"$identities"
  rpm_identity lighthouse-rpm "$staged/lighthouse-unsigned.rpm" >>"$identities"
  python3 - "$staged" "$revision" "$epoch" "$target_fedora" "$identities" <<'PY'
import hashlib, json, os, pathlib, sys
root, revision, epoch, target = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4])
identities = {}
for line in pathlib.Path(sys.argv[5]).read_text(encoding="ascii").splitlines():
    role, nevra, algorithm, digest = line.split("\t")
    identities[role] = {"nevra": nevra, "payload_digest_algorithm": int(algorithm), "payload_digest": digest}
outputs = []
for role in ("workstation", "server", "lighthouse"):
    path = root / f"{role}-unsigned.rpm"
    release_role = f"{role}-rpm"
    outputs.append({"role": release_role, "file": path.name,
                    "sha256": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
                    "size": path.stat().st_size, **identities[release_role]})
document = {"schema_version": 1, "kind": "mcnf-first-release-unsigned-rpm-handoff",
            "source_revision": revision, "commit_epoch": epoch,
            "target_fedora": target,
            "operator_action": "sign-and-produce-candidates", "promotion": "forbidden",
            "outputs": outputs}
fd = os.open(root / "handoff.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
with os.fdopen(fd, "w", encoding="ascii") as stream:
    json.dump(document, stream, sort_keys=True, separators=(",", ":")); stream.write("\n")
PY
  publish_dir "$staged" "$output"
  printf 'first-full-release: PASS: unsigned operator handoff %s (promotion forbidden)\n' "$output"
  exit 0
fi

[[ -z "$epoch$preflight_file" && -n "$handoff$derivative_file$plan_input" ]] \
  || refuse 'resume requires handoff, derivative arguments, and plan input only'
[[ -d "$handoff" && ! -L "$handoff" ]] || refuse 'handoff must be a real directory'
regular 'handoff manifest' "$handoff/handoff.json" 1048576
python3 - "$handoff/handoff.json" "$handoff" "$revision" "$target_fedora" <<'PY' || exit 2
import hashlib, json, pathlib, sys
import re
manifest, root, revision, target = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3], int(sys.argv[4])
try: value = json.loads(manifest.read_text(encoding="ascii"))
except Exception as exc: raise SystemExit(f"first-full-release: REFUSED: invalid handoff: {exc}")
expected_top={"schema_version","kind","source_revision","commit_epoch","target_fedora","operator_action","promotion","outputs"}
if set(value) != expected_top or value.get("schema_version") != 1 or value.get("kind") != "mcnf-first-release-unsigned-rpm-handoff" or value.get("source_revision") != revision or value.get("target_fedora") != target or value.get("operator_action") != "sign-and-produce-candidates" or value.get("promotion") != "forbidden":
    raise SystemExit("first-full-release: REFUSED: cross-revision, cross-target, or promotable handoff")
if not isinstance(value.get("outputs"), list) or len(value["outputs"]) != 3:
    raise SystemExit("first-full-release: REFUSED: incomplete handoff")
expected_files={"workstation-rpm":"workstation-unsigned.rpm","server-rpm":"server-unsigned.rpm","lighthouse-rpm":"lighthouse-unsigned.rpm"}
seen=set()
for row in value["outputs"]:
    if not isinstance(row, dict) or set(row) != {"role","file","sha256","size","nevra","payload_digest_algorithm","payload_digest"}:
        raise SystemExit("first-full-release: REFUSED: malformed handoff output")
    role=row["role"]
    if role in seen or row["file"] != expected_files.get(role) or row["payload_digest_algorithm"] != 8 or not isinstance(row["size"], int) or not 0 < row["size"] <= 1073741824 or not re.fullmatch(r"[0-9a-f]{64}", row["payload_digest"]):
        raise SystemExit("first-full-release: REFUSED: malformed handoff RPM identity")
    seen.add(role)
    path = root / row["file"]
    if not path.is_file() or path.is_symlink() or path.stat().st_mode & 0o222:
        raise SystemExit("first-full-release: REFUSED: mutable handoff artifact")
    if path.stat().st_size != row["size"] or "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
        raise SystemExit("first-full-release: REFUSED: mutated handoff artifact")
if seen != set(expected_files): raise SystemExit("first-full-release: REFUSED: incomplete handoff roles")
PY
load_arguments "$derivative_file" derivative_args --source-revision --output
regular 'release-output plan input' "$plan_input" 1048576
signed_paths=$work/signed-rpm-paths.tsv
python3 - "$plan_input" "$revision" >"$signed_paths" <<'PY' || refuse 'plan input does not name the exact signed RPM release roles'
import json, pathlib, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
outputs = value.get("outputs")
roles={"workstation-rpm","server-rpm","lighthouse-rpm","browser-vm","app-vm","cuttlefish-image","bootc-image"}
if value.get("schema_version") != 1 or value.get("kind") != "mcnf-release-output-plan-input" or value.get("source_revision") != sys.argv[2] or not isinstance(outputs, dict) or set(outputs) != roles:
    raise SystemExit("plan identity or roles are not exact")
for role in ("workstation-rpm", "server-rpm", "lighthouse-rpm"):
    row = outputs.get(role)
    if not isinstance(row, dict) or set(row) != {"artifact","candidate_manifest"} or not isinstance(row.get("artifact"), str): raise SystemExit(f"malformed {role}")
    print(role, str(pathlib.Path(row["artifact"]).resolve(strict=True)), sep="\t")
PY
handoff_identities=$work/handoff-identities.tsv
python3 - "$handoff/handoff.json" >"$handoff_identities" <<'PY'
import json, sys
value=json.load(open(sys.argv[1], encoding="ascii"))
for row in value["outputs"]:
    print(row["role"], row["nevra"], row["payload_digest_algorithm"], row["payload_digest"], sep="\t")
PY
signed_identities=$work/signed-identities.tsv
: >"$signed_identities"
while IFS=$'\t' read -r role path; do
  regular "$role signed candidate" "$path" 1073741824
  rpm_identity "$role" "$path" >>"$signed_identities"
done <"$signed_paths"
cmp -s -- "$handoff_identities" "$signed_identities" \
  || refuse 'signed RPM payload identity differs from the prepared unsigned handoff'
workstation_signed=$(awk -F '\t' '$1 == "workstation-rpm" {print $2}' "$signed_paths")
lighthouse_signed=$(awk -F '\t' '$1 == "lighthouse-rpm" {print $2}' "$signed_paths")
[[ $(realpath -e -- "$(argument_value derivative_args --signed-workstation-rpm)") == "$workstation_signed" ]] \
  || refuse 'derivative Workstation RPM differs from the admitted signed plan candidate'
[[ $(realpath -e -- "$(argument_value derivative_args --signed-lighthouse-rpm)") == "$lighthouse_signed" ]] \
  || refuse 'derivative Lighthouse RPM differs from the admitted signed plan candidate'
staged=$work/resumed
mkdir -m 0700 "$staged"
# Admit every signed/package/image input through its canonical owning verifier
# before derivative construction can create an image or other expensive side
# effect.  Collection is private until the entire phase publishes, so a later
# derivative failure still leaves no caller-visible partial release.
python3 "$PLAN" --inputs "$plan_input" --output "$staged/collection-plan.json"
python3 "$COLLECTOR" --plan "$staged/collection-plan.json" --output "$staged/release-outputs.json"
python3 - "$staged/release-outputs.json" "$revision" <<'PY' || refuse 'collector emitted a promotable or cross-revision result'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["source_revision"] == sys.argv[2] and value["promotion"] == "forbidden"
PY
"$DERIVATIVES" --source-revision "$revision" "${derivative_args[@]}" --output "$staged/derivatives"
publish_dir "$staged" "$output"
printf 'first-full-release: PASS: verified seven-role output %s (promotion forbidden)\n' "$output"
