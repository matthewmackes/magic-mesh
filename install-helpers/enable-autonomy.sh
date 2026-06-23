#!/usr/bin/env bash
# enable-autonomy.sh — grant the Claude Code permission rules that let the
# autonomous `/ship` farm-drain run without per-action classifier prompts.
#
#   RUN THIS YOURSELF (operator). It is a deliberate, reviewable grant of BROAD
#   autonomous capability to the agent on THIS host:
#     * self-modify its own skills (.claude/skills/**)
#     * drive the XEN build farm + deploy binaries over SSH (build + prod deploy)
#     * merge PRs, push, cut/sign/publish RPMs
#     * manage cloud + hypervisor infra via OpenTofu + doctl
#   Review the RULES list below before running. Remove any line you do NOT want
#   to grant. Re-running is idempotent (rules are de-duplicated).
#
# Scope: writes the project-local, git-ignored `.claude/settings.local.json`
# (precedence over the checked-in settings; never committed). Pass `--user` to
# write `~/.claude/settings.json` instead (session-wide, all projects).
#
# Usage:
#   ./install-helpers/enable-autonomy.sh           # project-local (recommended)
#   ./install-helpers/enable-autonomy.sh --user    # user-global
#   ./install-helpers/enable-autonomy.sh --print    # show the rules, write nothing
set -euo pipefail

# --- the grant -------------------------------------------------------------
# Each entry is a Claude Code permission rule (tool + pattern). These pre-approve
# the actions the autonomous /ship loop performs so the auto-mode classifier stops
# gating them. Trim to taste.
read -r -d '' RULES <<'JSON' || true
[
  "Edit(.claude/skills/**)",
  "Write(.claude/skills/**)",
  "Edit(docs/WORKLIST.md)",
  "Edit(docs/**)",
  "Edit(AI_GOVERNANCE.md)",

  "Bash(./install-helpers/xcp-build.sh:*)",
  "Bash(rsync:*)",
  "Bash(ssh:*)",
  "Bash(scp:*)",
  "Bash(sshpass:*)",
  "Bash(nohup:*)",

  "Bash(gh pr create:*)",
  "Bash(gh pr merge:*)",
  "Bash(gh pr view:*)",
  "Bash(gh pr list:*)",
  "Bash(gh release create:*)",
  "Bash(gh release view:*)",
  "Bash(git push:*)",
  "Bash(git fetch:*)",

  "Bash(createrepo_c:*)",
  "Bash(rpmsign:*)",
  "Bash(gpg:*)",

  "Bash(tofu:*)",
  "Bash(terraform:*)",
  "Bash(doctl:*)",
  "Bash(xe:*)"
]
JSON

MODE="project"; PRINT=0
for a in "$@"; do case "$a" in
  --user) MODE="user";;
  --print) PRINT=1;;
  *) echo "unknown arg: $a" >&2; exit 2;;
esac; done

if [ "$PRINT" -eq 1 ]; then
  echo "Permission rules this grant adds:"; printf '%s\n' "$RULES" | python3 -c 'import json,sys;[print("  -",r) for r in json.load(sys.stdin)]'
  exit 0
fi

# Resolve the target settings file.
if [ "$MODE" = "user" ]; then
  SETTINGS="$HOME/.claude/settings.json"
else
  ROOT="$(cd "$(dirname "$0")/.." && pwd)"   # repo root (script lives in install-helpers/)
  SETTINGS="$ROOT/.claude/settings.local.json"
fi
mkdir -p "$(dirname "$SETTINGS")"
[ -s "$SETTINGS" ] || echo '{}' > "$SETTINGS"

# Merge the rules into permissions.allow (preserve existing keys + rules; dedupe).
python3 - "$SETTINGS" <<PY
import json, sys
path = sys.argv[1]
rules = json.loads('''$RULES''')
try:
    with open(path) as f:
        data = json.load(f) or {}
except Exception:
    data = {}
perms = data.setdefault("permissions", {})
allow = perms.setdefault("allow", [])
added = []
for r in rules:
    if r not in allow:
        allow.append(r); added.append(r)
with open(path, "w") as f:
    json.dump(data, f, indent=2); f.write("\n")
print(f"settings: {path}")
print(f"added {len(added)} new rule(s); {len(allow)} total in permissions.allow")
for r in added: print("  +", r)
PY

echo
echo "Done. The autonomous /ship loop's actions are now pre-approved on this host."
echo "Restart/continue the session for the new rules to take effect."
echo "Revoke any time by editing $SETTINGS (remove lines from permissions.allow)."
