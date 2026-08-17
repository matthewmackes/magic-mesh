#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DRIVER=$ROOT/install-helpers/run-first-full-release.sh
WORK=$(mktemp -d --tmpdir="$PWD" .first-release-driver-test.XXXXXX)
trap 'chmod -R u+rwX -- "$WORK" 2>/dev/null || true; rm -rf -- "$WORK"' EXIT
chmod 0700 "$WORK"
REV=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
EPOCH=1776038400
LOG=$WORK/log
MOCK=$WORK/mock
cat >"$MOCK" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\t%s\n' "$(basename "$0")" "$*" >>"$TEST_LOG"
case "$(basename "$0")" in
  source)
    [[ "$*" == "--repo $REPO_ROOT" ]]
    if [[ -n ${TEST_SOURCE_SEQUENCE:-} ]]; then
      count=0; [[ ! -e $TEST_SOURCE_COUNT ]] || read -r count <"$TEST_SOURCE_COUNT"
      count=$((count + 1)); printf '%s\n' "$count" >"$TEST_SOURCE_COUNT"
      sed -n "${count}p" "$TEST_SOURCE_SEQUENCE"
    else
      printf '%s\t%s\n' "$TEST_REV" "$TEST_EPOCH"
    fi ;;
  preflight) [[ "$1 $2 $3 $4" == "--source-revision $TEST_REV --source-epoch $TEST_EPOCH" ]] ;;
  farm)
    if [[ $1 == pull ]]; then
      mkdir -p "$MCNF_BUILD_ARTIFACTS"
      case "$MCNF_BUILD_SLOT" in
        *full) printf '\355\253\356\333full' >"$MCNF_BUILD_ARTIFACTS/magic-mesh-1-1.x86_64.rpm"; printf '\355\253\356\333light' >"$MCNF_BUILD_ARTIFACTS/magic-mesh-lighthouse-1-1.x86_64.rpm" ;;
        *server) printf '\355\253\356\333server' >"$MCNF_BUILD_ARTIFACTS/magic-mesh-server-1-1.x86_64.rpm" ;;
      esac
    fi ;;
  derivatives)
    while (($#)); do
      if [[ $1 == --output ]]; then mkdir -m 0700 "$2"; printf derivative >"$2/marker"; break; fi
      shift
    done ;;
  rpm-query)
    path=${!#}; base=$(basename "$path")
    case "$base" in
      workstation-*|signed-workstation.rpm) name=magic-mesh; digest=$(printf '1%.0s' {1..64}) ;;
      server-*|signed-server.rpm) name=magic-mesh-server; digest=$(printf '2%.0s' {1..64}) ;;
      lighthouse-*|signed-lighthouse.rpm) name=magic-mesh-lighthouse; digest=$(printf '3%.0s' {1..64}) ;;
      substituted.rpm) name=magic-mesh; digest=$(printf '9%.0s' {1..64}) ;;
      *) exit 2 ;;
    esac
    printf '%s\t%s-1.0-1.x86_64\t8\t%s\n' "$name" "$name" "$digest" ;;
  plan) while (($#)); do [[ $1 == --output ]] && { printf plan >"$2"; break; }; shift; done ;;
  collector) while (($#)); do [[ $1 == --output ]] && { printf '{"source_revision":"%s","promotion":"forbidden"}\n' "$TEST_REV" >"$2"; break; }; shift; done ;;
esac
SH
chmod 0755 "$MOCK"
for name in source preflight farm derivatives rpm-query; do ln -s mock "$WORK/$name"; done
cat >"$WORK/plan" <<'PY'
import pathlib, sys
pathlib.Path(sys.argv[sys.argv.index("--output") + 1]).write_text("plan")
PY
cat >"$WORK/collector" <<'PY'
import json, os, pathlib, sys
if os.environ.get("TEST_COLLECTOR_REFUSE") == "1":
    raise SystemExit("hostile signed candidate refused")
pathlib.Path(sys.argv[sys.argv.index("--output") + 1]).write_text(json.dumps({"source_revision": os.environ["TEST_REV"], "promotion": "forbidden"}))
PY
chmod 0755 "$WORK/plan" "$WORK/collector"
printf '["--fixture","admitted"]\n' >"$WORK/preflight.json"
chmod 0400 "$WORK/preflight.json"
cat >"$WORK/argv-loader" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == "$TEST_PRIVATE_OBJECT" && "$2" == --emit-driver-arguments ]]
printf '["--fixture","admitted"]\n' >"$3"
chmod 0400 "$3"
SH
chmod 0755 "$WORK/argv-loader"
printf '{}\n' >"$WORK/private-object.json"
chmod 0400 "$WORK/private-object.json"
export TEST_LOG=$LOG TEST_REV=$REV TEST_EPOCH=$EPOCH
export REPO_ROOT=$ROOT
export MCNF_RELEASE_SOURCE_VERIFY=$WORK/source MCNF_RELEASE_PREFLIGHT=$WORK/preflight
export MCNF_RELEASE_INPUT_ARGV_LOADER=$WORK/argv-loader TEST_PRIVATE_OBJECT=$WORK/private-object.json
export MCNF_RELEASE_FARM=$WORK/farm MCNF_RELEASE_DERIVATIVES=$WORK/derivatives
export MCNF_RELEASE_PLAN=$WORK/plan MCNF_RELEASE_COLLECTOR=$WORK/collector
export MCNF_RELEASE_RPM_QUERY=$WORK/rpm-query

# A wrong or dirty/unresolvable initial checkout must stop before preflight and
# before any farm action. The mock's mismatched receipt represents either case;
# canonical source-revision-receipt owns the dirty-check implementation.
printf '%s\t%s\n' bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb "$EPOCH" >"$WORK/wrong-sequence"
export TEST_SOURCE_SEQUENCE=$WORK/wrong-sequence TEST_SOURCE_COUNT=$WORK/source-count
if "$DRIVER" prepare --source-revision "$REV" --source-epoch "$EPOCH" --target-fedora 44 --preflight-arguments "$WORK/preflight.json" --output "$WORK/wrong" >/dev/null 2>&1; then
  echo 'wrong source identity was accepted' >&2; exit 1
fi
[[ $(grep -Ec '^(preflight|farm)' "$LOG" || true) -eq 0 ]] \
  || { echo 'wrong source identity reached preflight or farm' >&2; exit 1; }
unset TEST_SOURCE_SEQUENCE TEST_SOURCE_COUNT

"$DRIVER" prepare --source-revision "$REV" --source-epoch "$EPOCH" \
  --target-fedora 44 --preflight-arguments "$WORK/preflight.json" --output "$WORK/handoff"
grep -Fq $'farm\tcontainer-rpm --full 44' "$LOG"
grep -Fq $'farm\tcontainer-rpm --server 44' "$LOG"
python3 - "$WORK/handoff/handoff.json" <<'PY'
import json, sys
v=json.load(open(sys.argv[1])); assert v["promotion"] == "forbidden" and v["target_fedora"] == 44 and len(v["outputs"]) == 3
PY
"$DRIVER" prepare --source-revision "$REV" --source-epoch "$EPOCH" \
  --target-fedora 44 --preflight-object "$WORK/private-object.json" --output "$WORK/object-handoff"
[[ -f "$WORK/object-handoff/handoff.json" ]] || { echo 'private preflight object did not produce a handoff' >&2; exit 1; }
if "$DRIVER" prepare --source-revision "$REV" --source-epoch "$EPOCH" --target-fedora 44 --preflight-arguments "$WORK/preflight.json" --output "$WORK/handoff" >/dev/null 2>&1; then
  echo 'duplicate prepare output was accepted' >&2; exit 1
fi

# Identity movement after the full cut must prevent the server cut. This proves
# re-resolution occurs at both build mutation boundaries.
: >"$LOG"
printf '%s\t%s\n%s\t%s\n%s\t%s\n' "$REV" "$EPOCH" "$REV" "$EPOCH" bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb "$EPOCH" >"$WORK/moving-sequence"
rm -f -- "$WORK/source-count"
export TEST_SOURCE_SEQUENCE=$WORK/moving-sequence TEST_SOURCE_COUNT=$WORK/source-count
if "$DRIVER" prepare --source-revision "$REV" --source-epoch "$EPOCH" --target-fedora 44 --preflight-arguments "$WORK/preflight.json" --output "$WORK/moving" >/dev/null 2>&1; then
  echo 'moving source identity was accepted' >&2; exit 1
fi
[[ $(grep -Fc $'farm\tcontainer-rpm --full 44' "$LOG") -eq 1 ]]
[[ $(grep -Fc $'farm\tcontainer-rpm --server 44' "$LOG") -eq 0 ]]
unset TEST_SOURCE_SEQUENCE TEST_SOURCE_COUNT
for role in workstation server lighthouse; do
  printf '\355\253\356\333signed' >"$WORK/signed-$role.rpm"
  chmod 0400 "$WORK/signed-$role.rpm"
done
printf '["--signed-workstation-rpm","%s","--signed-lighthouse-rpm","%s"]\n' \
  "$WORK/signed-workstation.rpm" "$WORK/signed-lighthouse.rpm" >"$WORK/derivative.json"
python3 - "$WORK/plan-input.json" "$WORK" <<'PY'
import json, pathlib, sys
root=pathlib.Path(sys.argv[2])
outputs={f"{role}-rpm":{"artifact":str(root/f"signed-{role}.rpm"),"candidate_manifest":str(root/f"{role}.json")} for role in ("workstation","server","lighthouse")}
outputs.update({"browser-vm":{},"app-vm":{},"bootc-image":{}})
value={"schema_version":1,"kind":"mcnf-release-output-plan-input","source_revision":"a"*40,
       "commit_epoch":"1776038400","signing_identity":"A"*40,"release_key":str(root/"key"),"outputs":outputs}
pathlib.Path(sys.argv[1]).write_text(json.dumps(value))
PY
chmod 0400 "$WORK/derivative.json" "$WORK/plan-input.json"

# Canonical signed-artifact admission must precede derivative construction.
# A verifier refusal may not create derivative images or caller-visible output.
: >"$LOG"
export TEST_COLLECTOR_REFUSE=1
if "$DRIVER" resume --source-revision "$REV" --target-fedora 44 --handoff "$WORK/handoff" \
  --derivative-arguments "$WORK/derivative.json" --plan-input "$WORK/plan-input.json" --output "$WORK/refused" >/dev/null 2>&1; then
  echo 'hostile signed candidate was accepted' >&2; exit 1
fi
unset TEST_COLLECTOR_REFUSE
[[ $(grep -Ec '^derivatives' "$LOG" || true) -eq 0 ]] \
  || { echo 'derivative construction ran before signed-artifact admission' >&2; exit 1; }
[[ ! -e "$WORK/refused" && ! -L "$WORK/refused" ]] \
  || { echo 'refused signed candidate published partial release output' >&2; exit 1; }

"$DRIVER" resume --source-revision "$REV" --target-fedora 44 --handoff "$WORK/handoff" \
  --derivative-arguments "$WORK/derivative.json" --plan-input "$WORK/plan-input.json" --output "$WORK/resumed"
python3 - "$WORK/resumed/release-outputs.json" <<'PY'
import json, sys
assert json.load(open(sys.argv[1]))["promotion"] == "forbidden"
PY
if "$DRIVER" resume --source-revision bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb --target-fedora 44 --handoff "$WORK/handoff" --derivative-arguments "$WORK/derivative.json" --plan-input "$WORK/plan-input.json" --output "$WORK/cross" >/dev/null 2>&1; then
  echo 'cross-revision handoff was accepted' >&2; exit 1
fi
if "$DRIVER" resume --source-revision "$REV" --target-fedora 43 --handoff "$WORK/handoff" --derivative-arguments "$WORK/derivative.json" --plan-input "$WORK/plan-input.json" --output "$WORK/cross-target" >/dev/null 2>&1; then
  echo 'cross-target handoff was accepted' >&2; exit 1
fi
printf '\355\253\356\333substitute' >"$WORK/substituted.rpm"; chmod 0400 "$WORK/substituted.rpm"
python3 - "$WORK/substituted-plan.json" "$WORK/plan-input.json" "$WORK/substituted.rpm" <<'PY'
import json, pathlib, sys
value=json.load(open(sys.argv[2])); value["outputs"]["workstation-rpm"]["artifact"]=sys.argv[3]
pathlib.Path(sys.argv[1]).write_text(json.dumps(value))
PY
chmod 0400 "$WORK/substituted-plan.json"
: >"$LOG"
if "$DRIVER" resume --source-revision "$REV" --target-fedora 44 --handoff "$WORK/handoff" --derivative-arguments "$WORK/derivative.json" --plan-input "$WORK/substituted-plan.json" --output "$WORK/substituted" >/dev/null 2>&1; then
  echo 'cross-build signed RPM was accepted' >&2; exit 1
fi
[[ $(grep -Ec '^derivatives' "$LOG" || true) -eq 0 ]]
echo 'test-run-first-full-release: hostile phase-boundary suite passed'
