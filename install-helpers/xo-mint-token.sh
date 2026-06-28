#!/usr/bin/env bash
# xo-mint-token.sh — mint a dedicated Xen Orchestra API token for the OpenTofu
# Farm Automation Manager and store it 0600 off-repo, never echoing it.
#
# The token is what the vatesfr/xenorchestra provider authenticates with
# (XOA_TOKEN). This codifies the one-time mint: authenticate to XO's REST API
# with the admin credential, POST a new named authentication token, and write
# its value to the token file. A dedicated, named, independently-revocable token
# is better practice than reusing the admin session token.
#
# Auth source: $XO_ADMIN_FILE (default /root/.mcnf-xo-admin), a shell file with
#   XOA_URL=ws://<host>:8080   (the http base is derived by swapping the scheme)
#   XOA_TOKEN=<an existing valid admin token>   (used as the REST cookie)
# Output (DAR-5):
#   default        → the MESH SECRET STORE (/mcnf/secret/xo-token, age-encrypted in
#                    etcd) via `mcnf-secret.sh put xo-token` — NO host-local plaintext.
#   --token-file   → legacy: a 0600 file at $XO_TOKEN_FILE (default /root/.mcnf-xo-token).
# Either way the token value is piped on STDIN, never echoed or passed on argv.
#
# Usage: xo-mint-token.sh [--desc opentofu-fam] [--http http://172.20.145.192:8080]
#                         [--to-store | --token-file]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
SECRET="${MCNF_SECRET_BIN:-$HERE/../automation/secrets/mcnf-secret.sh}"
ADMIN_FILE="${XO_ADMIN_FILE:-/root/.mcnf-xo-admin}"
TOKEN_FILE="${XO_TOKEN_FILE:-/root/.mcnf-xo-token}"
DESC="opentofu-fam"
HTTP_BASE=""
SINK="store"   # DAR-5 default: fold straight into the mesh secret store.
while [ $# -gt 0 ]; do case "$1" in
  --desc) DESC="$2"; shift 2;;
  --http) HTTP_BASE="$2"; shift 2;;
  --to-store)  SINK="store"; shift;;
  --token-file) SINK="file"; shift;;
  -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

[ -r "$ADMIN_FILE" ] || { echo "no admin creds at $ADMIN_FILE" >&2; exit 1; }
set -a
# shellcheck disable=SC1090
. "$ADMIN_FILE"
set +a
: "${XOA_TOKEN:?admin XOA_TOKEN missing from $ADMIN_FILE}"
# Derive the REST base from XOA_URL (ws://host:port -> http://host:port).
[ -n "$HTTP_BASE" ] || HTTP_BASE="$(printf '%s' "${XOA_URL:?XOA_URL missing}" | sed -E 's,^wss?://,http://,; s,/$,,')"

command -v python3 >/dev/null || { echo "python3 required" >&2; exit 1; }

tmp="$(mktemp)"; trap 'shred -u "$tmp" 2>/dev/null || rm -f "$tmp"' EXIT
code="$(curl -s -L -o "$tmp" -w '%{http_code}' -X POST \
  --cookie "authenticationToken=$XOA_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"description\":\"$DESC\"}" \
  "$HTTP_BASE/rest/v0/users/me/authentication_tokens")"
[ "$code" = "201" ] || { echo "token create failed (HTTP $code) — is the admin token still valid?" >&2; exit 1; }

# token.id is the token value. Never echo it — emit ONLY to the chosen sink.
exp="$(python3 -c "import json,datetime; print(datetime.datetime.utcfromtimestamp(json.load(open('$tmp'))['token']['expiration']/1000).strftime('%Y-%m-%d %H:%MZ'))")"
if [ "$SINK" = "store" ]; then
  # DAR-5: pipe the value straight into the mesh secret store on STDIN (never argv).
  python3 -c "import json; sys=open('$tmp'); print(json.load(sys)['token']['id'], end='')" \
    | bash "$SECRET" put xo-token >/dev/null
  echo "minted '$DESC' token → /mcnf/secret/xo-token (sealed in the store), expires $exp"
  echo "use it: source infra/tofu/env.sh  (XOA_TOKEN = mcnf-secret.sh get xo-token)"
else
  ( umask 077; python3 -c "import json; open('$TOKEN_FILE','w').write(json.load(open('$tmp'))['token']['id'])" )
  chmod 600 "$TOKEN_FILE"
  echo "minted '$DESC' token → $TOKEN_FILE (0600), $(wc -c <"$TOKEN_FILE") bytes, expires $exp"
  echo "use it (legacy): printf %s \"\$(cat $TOKEN_FILE)\" | mcnf-secret.sh put xo-token  # then source env.sh"
fi
