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
# Output: $XO_TOKEN_FILE (default /root/.mcnf-xo-token), 0600.
#
# Usage: xo-mint-token.sh [--desc opentofu-fam] [--http http://172.20.145.192:8080]
set -euo pipefail

ADMIN_FILE="${XO_ADMIN_FILE:-/root/.mcnf-xo-admin}"
TOKEN_FILE="${XO_TOKEN_FILE:-/root/.mcnf-xo-token}"
DESC="opentofu-fam"
HTTP_BASE=""
while [ $# -gt 0 ]; do case "$1" in
  --desc) DESC="$2"; shift 2;;
  --http) HTTP_BASE="$2"; shift 2;;
  -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

[ -r "$ADMIN_FILE" ] || { echo "no admin creds at $ADMIN_FILE" >&2; exit 1; }
# shellcheck disable=SC1090
set -a; . "$ADMIN_FILE"; set +a
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

# token.id is the token value; write it without printing it.
( umask 077; python3 -c "import json,sys; open('$TOKEN_FILE','w').write(json.load(open('$tmp'))['token']['id'])" )
chmod 600 "$TOKEN_FILE"
exp="$(python3 -c "import json,datetime; print(datetime.datetime.utcfromtimestamp(json.load(open('$tmp'))['token']['expiration']/1000).strftime('%Y-%m-%d %H:%MZ'))")"
echo "minted '$DESC' token → $TOKEN_FILE (0600), $(wc -c <"$TOKEN_FILE") bytes, expires $exp"
echo "use it: export XOA_TOKEN=\"\$(cat $TOKEN_FILE)\"  (see infra/tofu/env.sh.example)"
