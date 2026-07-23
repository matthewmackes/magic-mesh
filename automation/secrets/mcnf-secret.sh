#!/usr/bin/env bash
# DATACENTER-3 / DS-8 — the mesh secret store: age-encrypted secrets in etcd.
# DAR-3 (DEVOPS-AUTOMATION-REBUILD) — on-VM secret-zero: multi-recipient seal.
# WL-SEC-003 — role/scope-targeted sealing: scoped decryption roots.
#
# Secrets are age-encrypted and stored in etcd, so the control plane carries no
# host-local plaintext: any node holding ONE of the registered age identities
# decrypts the same secret from the replicated store.
#
#   ciphertext → etcd /mcnf/secret/<name>
#   legacy single recipient → etcd /mcnf/age-recipient        (back-compat)
#   per-node recipient set  → etcd /mcnf/age-recipients/<id>  (DAR-3)
#   per-node role/scope tags → etcd /mcnf/node-tags/<id>      (WL-SEC-003)
#   per-secret recorded scope → etcd /mcnf/secret-scope/<name> (WL-SEC-003)
#
# WL-SEC-003 — SCOPED DECRYPTION ROOTS: by default a secret is sealed to EVERY
# registered recipient (whole-mesh — any node decrypts). `put --scope role:<r>`
# / `--scope scope:<s>` instead seals ONLY to the nodes whose published tags
# (/mcnf/node-tags/<id>, set at init-self from MCNF_NODE_ROLE / MCNF_NODE_SCOPES)
# match the selector — so a NON-matching node's key is never an `age` recipient
# and it cannot decrypt the scoped ciphertext even though it holds valid mesh
# creds. The recipient-set resolver is shared with the Rust `mde-seal::scope`
# module (`recipients_for`). Each scoped secret records its scope so a later
# `rotate` / bulk `reseal` preserves the narrowed blast radius (never widens it).
#
# The age IDENTITY (private) is the only host-local artifact — kept 0600 at
# /root/.mcnf-age-key and NEVER printed, logged, or transmitted. Only the public
# recipient (age1…) is ever written to the mesh.
#
# SECRET-ZERO (DAR-3): a fresh control VM does NOT receive any master key. At
# first boot it runs `init-self` to mint its OWN identity and publish only its
# public recipient. An operator (or the etcd leader) — a holder of the CURRENT
# mesh age key — then runs `reseal-to <recipient>` / `reseal-all`, which decrypts
# every /mcnf/secret/* with its own key and RE-ENCRYPTS multi-recipient so the new
# VM's key can `get` every cred. The control VM CANNOT self-reseal: it holds no
# master key, only its freshly-minted one (which the store is not yet sealed to).
#
# Usage:
#   mcnf-secret.sh init                 generate the mesh age key (if absent) + publish legacy recipient
#   mcnf-secret.sh init-self            DAR-3: mint THIS node's identity (0600) + register its recipient
#                                       WL-SEC-003: also publishes role/scope tags from MCNF_NODE_ROLE / MCNF_NODE_SCOPES
#   mcnf-secret.sh put <name> [--scope role:<r>|scope:<s>]
#                                       encrypt stdin → etcd /mcnf/secret/<name>. Default: FULL recipient set
#                                       (whole-mesh). --scope seals ONLY to matching nodes (scoped decryption root).
#   mcnf-secret.sh get <name>           decrypt etcd /mcnf/secret/<name> → stdout (exit 3 if absent)
#   mcnf-secret.sh rotate <name> [--scope role:<r>|scope:<s>] [--revoke-cmd '<cmd>']
#                                       atomic overwrite of <name> + optional provider-side revoke. Without --scope
#                                       the secret's EXISTING scope is preserved; --scope re-targets it.
#   mcnf-secret.sh reseal-to <recipient>  re-encrypt every secret to its scope's set ∪ {<recipient>} (whole-mesh secrets only)
#   mcnf-secret.sh reseal-all           re-encrypt every secret to its recorded scope's recipient set (scope-preserving)
#   mcnf-secret.sh recipients [--scope role:<r>|scope:<s>]
#                                       list the recipient SET (public keys only) — whole-mesh, or a scoped subset
#   mcnf-secret.sh list                 list stored secret names
#   mcnf-secret.sh selftest             run the offline self-test (mocked etcd; touches NO live store)
#   mcnf-secret.sh selftest-scope       WL-SEC-003 offline self-test of role/scope-targeted sealing (mocked etcd)
#
# Env:
#   MCNF_ETCD       etcd v3 HTTP endpoint (no default — resolved by the caller / DAR-1b).
#   MCNF_AGE_KEY    age identity path (default /root/.mcnf-age-key).
#   MCNF_NODE_ID    node-id for init-self's recipient key (default: hostname -s).
#   MCNF_NODE_ROLE  WL-SEC-003: this node's deployment role (lighthouse|workstation) published as a role:<r> tag by init-self.
#   MCNF_NODE_SCOPES  WL-SEC-003: comma/space-separated capability tags (e.g. media,voice) published as scope:<s> tags.
#   MCNF_SECRET_SELFTEST  set by `selftest` — routes etcd I/O to a local mock dir.
#
# v2 (DAR-1b lands the shared resolver): the dead 172.20.145.192:2379 default is
# GONE — MCNF_ETCD must be set by the caller (or sourced from
# /etc/mackesd/etcd-endpoints) or every etcd op fails loud.
set -euo pipefail

KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"

# ── etcd endpoint resolution (fail-loud; NO dead .192:2379 default) ──
# Order: explicit MCNF_ETCD → /etc/mackesd/etcd-endpoints (comma-joined) → FAIL.
# Skipped entirely in selftest mode (the mock backs etcd with a local dir).
_resolve_etcd() {
  if [ -n "${MCNF_SECRET_SELFTEST:-}" ]; then ETCD="mock://selftest"; return 0; fi
  if [ -n "${MCNF_ETCD:-}" ]; then ETCD="$MCNF_ETCD"; return 0; fi
  local f="/etc/mackesd/etcd-endpoints"
  if [ -r "$f" ]; then
    # File holds one or more http://<ip>:2379 lines/commas; use the first.
    ETCD="$(tr ',\n' ' ' <"$f" | awk '{print $1}')"
    [ -n "$ETCD" ] && return 0
  fi
  echo "mcnf-secret: no etcd endpoint — set MCNF_ETCD or write /etc/mackesd/etcd-endpoints (run setup-etcd.sh)" >&2
  exit 1
}

b64()  { base64 -w0; }

# ── etcd v3 HTTP KV layer (mockable for selftest) ──
# In selftest mode every KV op is backed by files under $MOCK_DIR, so the crypto
# + reseal logic is exercised honestly WITHOUT touching any live etcd.
_mock_path() { printf '%s/%s' "$MOCK_DIR" "$(printf %s "$1" | b64)"; }

_put() { # <key> <value-b64-string>
  if [ "$ETCD" = "mock://selftest" ]; then
    mkdir -p "$MOCK_DIR"; printf %s "$2" >"$(_mock_path "$1")"; return 0
  fi
  local k; k=$(printf %s "$1" | b64)
  curl -s -X POST "$ETCD/v3/kv/put" -d "{\"key\":\"$k\",\"value\":\"$2\"}" >/dev/null
}

_get() { # <key> -> raw value bytes on stdout (exit 3 if absent)
  if [ "$ETCD" = "mock://selftest" ]; then
    local p; p="$(_mock_path "$1")"
    [ -f "$p" ] || return 3
    base64 -d <"$p"; return 0
  fi
  local k; k=$(printf %s "$1" | b64)
  curl -s -X POST "$ETCD/v3/kv/range" -d "{\"key\":\"$k\"}" | python3 -c '
import sys,json,base64
d=json.load(sys.stdin); kvs=d.get("kvs")
if not kvs: sys.exit(3)
sys.stdout.buffer.write(base64.b64decode(kvs[0]["value"]))'
}

_del() { # <key>
  if [ "$ETCD" = "mock://selftest" ]; then rm -f "$(_mock_path "$1")"; return 0; fi
  local k; k=$(printf %s "$1" | b64)
  curl -s -X POST "$ETCD/v3/kv/deleterange" -d "{\"key\":\"$k\"}" >/dev/null
}

# List keys under a prefix (decoded), one per line.
_range_keys() { # <prefix>
  if [ "$ETCD" = "mock://selftest" ]; then
    [ -d "$MOCK_DIR" ] || return 0
    local f name
    for f in "$MOCK_DIR"/*; do
      [ -e "$f" ] || continue
      name="$(basename "$f" | base64 -d 2>/dev/null || true)"
      case "$name" in "$1"*) printf '%s\n' "$name" ;; esac
    done
    return 0
  fi
  local s e; s=$(printf %s "$1" | b64)
  e=$(python3 -c "import sys,base64;b=sys.argv[1].encode();print(base64.b64encode(b[:-1]+bytes([b[-1]+1])).decode())" "$1")
  curl -s -X POST "$ETCD/v3/kv/range" -d "{\"key\":\"$s\",\"range_end\":\"$e\",\"keys_only\":true}" | python3 -c '
import sys,json,base64
for kv in (json.load(sys.stdin).get("kvs") or []):
    print(base64.b64decode(kv["key"]).decode())'
}

# ── reseal lock (atomicity) ──
# create-if-absent CAS over /mcnf/reseal/lock, lease-backed so a crashed holder
# auto-releases after the TTL. Returns 0 iff WE won the lock.
RESEAL_LOCK="/mcnf/reseal/lock"
RESEAL_MARKER="/mcnf/reseal/marker"
_lease() { # <ttl> -> leaseID (selftest: a fixed sentinel)
  if [ "$ETCD" = "mock://selftest" ]; then echo "0"; return 0; fi
  curl -s -X POST "$ETCD/v3/lease/grant" -d "{\"TTL\":\"${1:-300}\",\"ID\":0}" \
    | python3 -c "import json,sys;print(json.load(sys.stdin).get('ID',''))" 2>/dev/null
}
_claim_lock() { # <owner> <leaseID> -> 0 if WE won
  if [ "$ETCD" = "mock://selftest" ]; then
    # File-based create-if-absent for the mock.
    local p; p="$(_mock_path "$RESEAL_LOCK")"; mkdir -p "$MOCK_DIR"
    if ( set -o noclobber; printf %s "$1" >"$p" ) 2>/dev/null; then return 0; else return 1; fi
  fi
  local lk val body resp
  lk=$(printf %s "$RESEAL_LOCK" | b64); val=$(printf %s "$1" | b64)
  body=$(python3 -c "
import json,sys
print(json.dumps({'compare':[{'key':sys.argv[1],'result':'EQUAL','target':'CREATE','create_revision':'0'}],
 'success':[{'requestPut':{'key':sys.argv[1],'value':sys.argv[2],'lease':sys.argv[3]}}],'failure':[]}))" "$lk" "$val" "$2")
  resp=$(curl -s -X POST "$ETCD/v3/kv/txn" -d "$body")
  printf '%s' "$resp" | python3 -c "import json,sys;sys.exit(0 if json.load(sys.stdin).get('succeeded') else 1)" 2>/dev/null
}
_release_lock() { _del "$RESEAL_LOCK"; }

# ── recipients ──
# The mesh age identity recipient (public) derived from the local private key.
recipient() { age-keygen -y "$KEY" 2>/dev/null; }

# The FULL recipient SET used for sealing: every /mcnf/age-recipients/<id> PLUS
# the legacy single /mcnf/age-recipient, de-duplicated. Public keys only — safe
# to print. Echoes one age1… recipient per line.
recipient_set() {
  {
    # Legacy single recipient (back-compat). Stored without a trailing newline,
    # so re-terminate it explicitly or it runs into the next recipient.
    local legacy; legacy="$(_get "/mcnf/age-recipient" 2>/dev/null || true)"
    [ -n "$legacy" ] && printf '%s\n' "$legacy"
    # Per-node recipient set (DAR-3). Each value IS a recipient line.
    local k v
    while IFS= read -r k; do
      [ -n "$k" ] || continue
      v="$(_get "$k" 2>/dev/null || true)"
      [ -n "$v" ] && printf '%s\n' "$v"
    done < <(_range_keys "/mcnf/age-recipients/")
  } | grep -E '^age1' | sort -u
}

# Encrypt stdin to a set of recipients (one age1… per arg) → b64 ciphertext.
# Repeated `-r` is age's native multi-recipient seal: ANY listed identity can
# decrypt. NEVER echoes the plaintext.
_seal_to_set() { # <recipient>...
  local args=()
  local r
  for r in "$@"; do args+=("-r" "$r"); done
  age "${args[@]}" | b64
}

# ── WL-SEC-003: role/scope-targeted sealing (scoped decryption roots) ──
# The recipient-set resolver shared with the Rust `mde-seal::scope` module.

# Validate a --scope selector: role:<r> or scope:<s>, non-empty value.
_validate_scope() { # <selector>
  case "$1" in
    role:?*|scope:?*) return 0 ;;
    *) echo "mcnf-secret: bad --scope '$1' — expected role:<role> or scope:<scope> (e.g. role:lighthouse, scope:media)" >&2; return 2 ;;
  esac
}

# Build THIS node's PUBLIC tag list from MCNF_NODE_ROLE + MCNF_NODE_SCOPES
# (comma/space separated), one selector per line, lowercased to match the
# resolver's canonicalization:
#   role:<role>
#   scope:<s1>
#   scope:<s2>
# Empty when neither env is set — an untagged node only takes part in whole-mesh
# seals. Public data only (role + capability names), never a secret.
_node_tags() {
  local out="" s role scope
  role="$(printf %s "${MCNF_NODE_ROLE:-}" | tr '[:upper:]' '[:lower:]')"
  if [ "$role" = lighthouse ] && [ -n "${MCNF_NODE_SCOPES:-}" ]; then
    for s in $(printf '%s' "$MCNF_NODE_SCOPES" | tr ',' ' '); do
      [ -n "$s" ] || continue
      scope="$(printf %s "$s" | tr '[:upper:]' '[:lower:]')"
      case "$scope" in
        media|fileshare|file_share|file-sharing|filesharing)
          echo "mcnf-secret: thin lighthouse cannot advertise media/fileshare scope '$s'" >&2
          return 2
          ;;
      esac
    done
  fi
  if [ -n "$role" ]; then
    out="role:$role"
  fi
  if [ -n "${MCNF_NODE_SCOPES:-}" ]; then
    for s in $(printf '%s' "$MCNF_NODE_SCOPES" | tr ',' ' '); do
      [ -n "$s" ] || continue
      s="scope:$(printf %s "$s" | tr '[:upper:]' '[:lower:]')"
      out="${out:+$out
}$s"
    done
  fi
  printf '%s' "$out"
}

# The SCOPED recipient set: only registered nodes whose /mcnf/node-tags/<id>
# contains the exact selector line (role:<r> or scope:<s>). Mirrors
# mde-seal::recipients_for over etcd. Public keys only, one age1… per line,
# deduped+sorted. The legacy single recipient (no node-id ⇒ no tags) is
# intentionally absent from scoped seals — an untagged recipient only ever
# receives whole-mesh secrets, so a scoped secret never leaks to it.
recipient_set_scoped() { # <selector>
  local selector="$1"
  {
    local k id tags v
    while IFS= read -r k; do
      [ -n "$k" ] || continue
      id="${k#/mcnf/age-recipients/}"
      tags="$(_get "/mcnf/node-tags/$id" 2>/dev/null || true)"
      [ -n "$tags" ] || continue
      if printf '%s\n' "$tags" | grep -Fxq -- "$selector"; then
        v="$(_get "$k" 2>/dev/null || true)"
        [ -n "$v" ] && printf '%s\n' "$v"
      fi
    done < <(_range_keys "/mcnf/age-recipients/")
  } | grep -E '^age1' | sort -u
}

# The recipient set a given secret must be (re)sealed to, honoring its RECORDED
# scope (/mcnf/secret-scope/<name>): the scoped subset when scoped, else the
# whole-mesh set. So a bulk reseal preserves each secret's blast radius rather
# than silently widening a scoped secret back to the whole mesh.
_recipients_for_secret() { # <name>
  local sc; sc="$(_get "/mcnf/secret-scope/$1" 2>/dev/null || true)"
  if [ -n "$sc" ]; then recipient_set_scoped "$sc"; else recipient_set; fi
}

cmd="${1:-}"
case "$cmd" in
  init)
    _resolve_etcd
    if [ ! -f "$KEY" ]; then (umask 077; age-keygen -o "$KEY" 2>/dev/null); echo "generated $KEY"; else echo "age key present: $KEY"; fi
    chmod 600 "$KEY" 2>/dev/null || true
    R="$(recipient)"; _put "/mcnf/age-recipient" "$(printf %s "$R" | b64)"
    echo "recipient published: $R"
    ;;

  init-self)
    # DAR-3: mint THIS node's OWN identity (private key never leaves the box) and
    # register only its PUBLIC recipient under /mcnf/age-recipients/<node-id>.
    # Idempotent: a re-run keeps the existing key + re-publishes its recipient.
    _resolve_etcd
    NODE_ID="${MCNF_NODE_ID:-$(hostname -s 2>/dev/null || hostname)}"
    [ -n "$NODE_ID" ] || { echo "init-self: could not determine node-id (set MCNF_NODE_ID)" >&2; exit 2; }
    # Validate the thin-lighthouse role/scope policy before minting or
    # publishing any identity material, so a rejected configuration leaves no
    # partially registered node behind.
    TAGS="$(_node_tags)"
    if [ ! -f "$KEY" ]; then
      (umask 077; age-keygen -o "$KEY" 2>/dev/null)
      echo "init-self: minted a fresh age identity at $KEY (0600)"
    else
      echo "init-self: identity already present at $KEY (idempotent re-run)"
    fi
    chmod 600 "$KEY" 2>/dev/null || true
    R="$(recipient)"
    [ -n "$R" ] || { echo "init-self: failed to derive recipient from $KEY" >&2; exit 1; }
    _put "/mcnf/age-recipients/$NODE_ID" "$(printf %s "$R" | b64)"
    # Public key only — safe to print. The PRIVATE key is never echoed.
    echo "init-self: registered recipient for '$NODE_ID': $R"
    # WL-SEC-003 — publish THIS node's role/scope tags so scoped seals
    # (`put --scope role:<r>|scope:<s>`) can target it. Tags are PUBLIC (role +
    # capability names), never secret; an untagged node only receives whole-mesh
    # seals. Re-publishes on every idempotent re-run (drops the key if the envs
    # are now unset).
    if [ -n "$TAGS" ]; then
      _put "/mcnf/node-tags/$NODE_ID" "$(printf %s "$TAGS" | b64)"
      echo "init-self: published tags for '$NODE_ID': $(printf %s "$TAGS" | tr '\n' ',' | sed 's/,$//')"
    else
      _del "/mcnf/node-tags/$NODE_ID"
    fi
    echo "init-self: an operator/leader must now run: mcnf-secret.sh reseal-to $R"
    ;;

  put)
    _resolve_etcd
    [ -n "${2:-}" ] || { echo "usage: put <name> [--scope role:<r>|scope:<s>]" >&2; exit 2; }
    name="$2"; shift 2
    scope=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --scope) scope="${2:-}"; shift 2 ;;
        *) echo "put: unknown arg '$1'" >&2; exit 2 ;;
      esac
    done
    if [ -n "$scope" ]; then
      # WL-SEC-003: seal ONLY to the nodes matching role:<r>/scope:<s> — a scoped
      # decryption root. Refuse an empty set rather than sealing to nobody (which
      # would write an undecryptable secret).
      _validate_scope "$scope" || exit 2
      mapfile -t RSET < <(recipient_set_scoped "$scope")
      [ "${#RSET[@]}" -gt 0 ] || { echo "put: --scope '$scope' matched no registered node — refusing to seal to an empty recipient set" >&2; exit 1; }
    else
      # DAR-3 default: seal to the FULL recipient set (every registered identity).
      mapfile -t RSET < <(recipient_set)
      if [ "${#RSET[@]}" -eq 0 ]; then
        # No recipient registered yet — fall back to THIS node's own recipient so a
        # bootstrap `init` + `put` on a single node still works (legacy behavior).
        R="$(recipient)"; [ -n "$R" ] || { echo "put: no recipients registered and no local key" >&2; exit 1; }
        RSET=("$R")
      fi
    fi
    ct="$(_seal_to_set "${RSET[@]}")"
    _put "/mcnf/secret/$name" "$ct"
    # Record (or clear) the secret's scope so a later rotate/reseal preserves the
    # narrowed blast radius (an unscoped put deliberately clears it → whole-mesh).
    if [ -n "$scope" ]; then _put "/mcnf/secret-scope/$name" "$(printf %s "$scope" | b64)"; else _del "/mcnf/secret-scope/$name"; fi
    echo "stored /mcnf/secret/$name (recipients: ${#RSET[@]}${scope:+, scope: $scope})"
    ;;

  get)
    _resolve_etcd
    [ -n "${2:-}" ] || { echo "usage: get <name>" >&2; exit 2; }
    # Fetch the ciphertext to a temp FILE first — age ciphertext is BINARY, and a
    # `$(...)` capture strips NUL bytes and corrupts it ("ignored null byte in
    # input" → decrypt failure). The file route stays binary-safe AND keeps the
    # two outcomes distinguishable by exit code: a genuinely ABSENT secret makes
    # `_get` exit 3 (taking the else), while a real fault (etcd unreachable,
    # decrypt failure) exits non-zero-and-not-3. `set -e` is suspended in the `if`
    # condition, so the exit-3 path is taken cleanly.
    ct="$(mktemp)"; trap 'rm -f "$ct"' EXIT
    if _get "/mcnf/secret/$2" >"$ct"; then
      age -d -i "$KEY" <"$ct"
    else
      exit $?
    fi
    ;;

  rotate)
    _resolve_etcd
    [ -n "${2:-}" ] || { echo "usage: rotate <name> [--scope role:<r>|scope:<s>] [--revoke-cmd '<cmd>']" >&2; exit 2; }
    name="$2"; shift 2
    revoke_cmd=""; scope=""; scope_set=0
    while [ $# -gt 0 ]; do
      case "$1" in
        --revoke-cmd) revoke_cmd="${2:-}"; shift 2 ;;
        --scope) scope="${2:-}"; scope_set=1; shift 2 ;;
        *) echo "rotate: unknown arg '$1'" >&2; exit 2 ;;
      esac
    done
    # Rotation requires the named secret to already exist (you rotate a value, not
    # create one) — a non-existent name exits 3 (matches `get`'s absent code).
    probe="$(mktemp)"; trap 'rm -f "$probe"' EXIT
    if ! _get "/mcnf/secret/$name" >"$probe"; then
      echo "rotate: '$name' is not in the store (use put to create)" >&2; exit 3
    fi
    # WL-SEC-003: without --scope, PRESERVE the secret's existing scope (never
    # silently widen a scoped secret when rotating its value). An explicit --scope
    # re-targets it; --scope '' explicitly widens back to whole-mesh.
    if [ "$scope_set" -eq 1 ]; then
      if [ -n "$scope" ]; then
        _validate_scope "$scope" || exit 2
        mapfile -t RSET < <(recipient_set_scoped "$scope")
        [ "${#RSET[@]}" -gt 0 ] || { echo "rotate: --scope '$scope' matched no registered node — refusing to seal to an empty recipient set" >&2; exit 1; }
      else
        mapfile -t RSET < <(recipient_set)
      fi
    else
      scope="$(_get "/mcnf/secret-scope/$name" 2>/dev/null || true)"
      mapfile -t RSET < <(_recipients_for_secret "$name")
    fi
    # Read the NEW value from stdin, seal to the resolved set, atomic single-key put.
    [ "${#RSET[@]}" -gt 0 ] || { R="$(recipient)"; RSET=("$R"); }
    ct="$(_seal_to_set "${RSET[@]}")"
    _put "/mcnf/secret/$name" "$ct"
    # Record the re-targeted scope only when --scope was explicit.
    if [ "$scope_set" -eq 1 ]; then
      if [ -n "$scope" ]; then _put "/mcnf/secret-scope/$name" "$(printf %s "$scope" | b64)"; else _del "/mcnf/secret-scope/$name"; fi
    fi
    echo "rotated /mcnf/secret/$name (recipients: ${#RSET[@]}${scope:+, scope: $scope})"
    if [ -n "$revoke_cmd" ]; then
      echo "rotate: running provider-side revoke…"
      bash -c "$revoke_cmd"
    fi
    ;;

  reseal-to|reseal-all)
    _resolve_etcd
    # DAR-3: re-encrypt EVERY /mcnf/secret/* to the union of registered recipients
    # (reseal-to additionally adds the explicit <recipient> arg). Run by the
    # OPERATOR/LEADER — a holder of the CURRENT mesh key, since the values must be
    # decryptable with the local $KEY to be re-encrypted. A control VM CANNOT do
    # this (it holds no master key); it only `init-self`s and waits for this step.
    #
    # ATOMICITY: the whole walk is wrapped in an etcd lease-backed lock so two
    # operators can't reseal concurrently and interleave writes. A completion
    # MARKER (/mcnf/reseal/marker) records {started,by,total} before the walk and
    # {completed,resealed} after — so a crash mid-walk leaves an INCOMPLETE marker
    # a later run (or backoffice-up Phase 0) can detect. Each secret is rewritten
    # in a SINGLE etcd put (etcd per-key writes are atomic), and we only advance
    # the count after the put returns, so no key is left half-written.
    extra=""
    if [ "$cmd" = "reseal-to" ]; then
      extra="${2:-}"; [ -n "$extra" ] || { echo "usage: reseal-to <recipient>" >&2; exit 2; }
      case "$extra" in age1*) ;; *) echo "reseal-to: '<recipient>' must be an age1… public recipient" >&2; exit 2 ;; esac
    fi
    [ -f "$KEY" ] || { echo "reseal: local age identity $KEY absent — the resealer must hold the current mesh key" >&2; exit 1; }

    owner="reseal:$(hostname -s 2>/dev/null || echo node):$$"
    lease="$(_lease 300)"
    if ! _claim_lock "$owner" "${lease:-0}"; then
      echo "reseal: another reseal holds $RESEAL_LOCK — refusing to interleave (try again shortly)" >&2
      exit 1
    fi
    # From here on, ALWAYS release the lock on exit.
    trap '_release_lock' EXIT

    # Build the whole-mesh base set: registered union ∪ {extra}. This is the
    # target for UNSCOPED secrets and the presence-check that there is anyone to
    # seal to at all. Scoped secrets are (re)sealed to THEIR own recorded scope's
    # set below (WL-SEC-003 — a bulk reseal must not widen a scoped secret).
    mapfile -t RSET < <(recipient_set)
    [ -n "$extra" ] && RSET+=("$extra")
    # De-dup once more (extra may already be registered).
    mapfile -t RSET < <(printf '%s\n' "${RSET[@]}" | grep -E '^age1' | sort -u)
    if [ "${#RSET[@]}" -eq 0 ]; then
      echo "reseal: no recipients to seal to (register one with init-self, or pass reseal-to <recipient>)" >&2
      exit 1
    fi

    mapfile -t NAMES < <(_range_keys "/mcnf/secret/")
    total="${#NAMES[@]}"
    ts="$(date -u +%FT%TZ 2>/dev/null || echo unknown)"
    _put "$RESEAL_MARKER" "$(printf '{"status":"started","by":"%s","total":%d,"ts":"%s"}' "$owner" "$total" "$ts" | b64)"

    resealed=0
    for full in "${NAMES[@]}"; do
      [ -n "$full" ] || continue
      # Skip non-secret keys that may share the range edge.
      case "$full" in /mcnf/secret/*) ;; *) continue ;; esac
      sname="${full#/mcnf/secret/}"
      # WL-SEC-003 — the target set for THIS secret: honor its recorded scope. A
      # scoped secret keeps its narrowed recipient set (the new reseal-to <extra>
      # is NOT force-added — a node is granted a scoped secret only via `put
      # --scope` once it publishes a matching tag). An unscoped secret gets the
      # whole-mesh base set ∪ {extra}.
      ssc="$(_get "/mcnf/secret-scope/$sname" 2>/dev/null || true)"
      if [ -n "$ssc" ]; then
        mapfile -t TSET < <(recipient_set_scoped "$ssc")
        if [ "${#TSET[@]}" -eq 0 ]; then
          echo "reseal: WARN $full is scoped '$ssc' but no registered node matches — leaving it unchanged" >&2
          continue
        fi
      else
        TSET=("${RSET[@]}")
      fi
      tmp="$(mktemp)"
      if ! _get "$full" >"$tmp"; then rm -f "$tmp"; echo "reseal: WARN $full vanished mid-walk, skipping" >&2; continue; fi
      # Decrypt with the local (current mesh) key, re-seal to the target set, atomic put.
      # NEVER echo the plaintext: the pipe goes age-d → age-seal → b64 in-process.
      if ! ct="$(age -d -i "$KEY" <"$tmp" | _seal_to_set "${TSET[@]}")"; then
        rm -f "$tmp"
        echo "reseal: FAILED to decrypt $full with $KEY — the resealer must hold the CURRENT mesh key" >&2
        # Mark the walk failed so a watcher sees the incomplete state.
        _put "$RESEAL_MARKER" "$(printf '{"status":"failed","by":"%s","total":%d,"resealed":%d,"failed_key":"%s","ts":"%s"}' "$owner" "$total" "$resealed" "$full" "$ts" | b64)"
        exit 1
      fi
      rm -f "$tmp"
      _put "$full" "$ct"
      resealed=$((resealed + 1))
    done

    _put "$RESEAL_MARKER" "$(printf '{"status":"completed","by":"%s","total":%d,"resealed":%d,"recipients":%d,"ts":"%s"}' "$owner" "$total" "$resealed" "${#RSET[@]}" "$ts" | b64)"
    echo "reseal: re-encrypted $resealed/$total secret(s) to ${#RSET[@]} recipient(s)"
    ;;

  recipients)
    _resolve_etcd
    # Public keys only — safe to print. (Length/presence + public recipients are
    # the ONLY secret-store data this tool ever emits.) WL-SEC-003: --scope
    # previews the scoped subset a `put --scope <sel>` would seal to.
    if [ "${2:-}" = "--scope" ]; then
      [ -n "${3:-}" ] || { echo "usage: recipients --scope role:<r>|scope:<s>" >&2; exit 2; }
      _validate_scope "$3" || exit 2
      recipient_set_scoped "$3"
    else
      recipient_set
    fi
    ;;

  list)
    _resolve_etcd
    _range_keys "/mcnf/secret/" | sed 's#^/mcnf/secret/##'
    ;;

  selftest)
    # Offline self-test: mock etcd with a local dir; touches NO live store.
    # Drives the REAL init-self/put/reseal-to arms (re-invoking $0 with the mock
    # active) so the production code paths are what's exercised, not a re-impl.
    # Asserts: (1) two registered recipients both decrypt the same secret after a
    # reseal, (2) the VM key file is 0600, (3) the VM key CANNOT read before
    # reseal, (4) the completion marker reaches 'completed', (5) NO secret value
    # and NO private key are ever logged.
    MOCK_DIR="$(mktemp -d)"; export MOCK_DIR
    ETCD="mock://selftest"
    work="$(mktemp -d)"
    fail=0
    pass() { printf '  PASS %s\n' "$1"; }
    bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
    # Re-invoke this script with the mock etcd active, capturing ALL output so we
    # can assert nothing sensitive leaked. SC: the secret is passed via stdin only.
    run() { env MCNF_SECRET_SELFTEST=1 MOCK_DIR="$MOCK_DIR" "$@" >>"$work/run.log" 2>&1; }
    : >"$work/run.log"

    echo "mcnf-secret selftest (mocked etcd at $MOCK_DIR — NO live store touched)"
    SECRET_VALUE="do-token-SUPER-SECRET-$RANDOM-$$"

    # --- Operator/leader identity (the holder of the CURRENT mesh key) ---
    op_key="$work/op-key"
    run env MCNF_AGE_KEY="$op_key" bash "$0" init

    # --- Seal a secret as the operator (only the op recipient registered) ---
    printf %s "$SECRET_VALUE" | run env MCNF_AGE_KEY="$op_key" bash "$0" put do-token

    got="$(age -d -i "$op_key" < <(_get "/mcnf/secret/do-token") 2>/dev/null || true)"
    [ "$got" = "$SECRET_VALUE" ] && pass "operator decrypts its own sealed secret" || bad "operator could not decrypt"

    # --- Control VM mints its OWN identity + registers recipient (init-self) ---
    vm_key="$work/vm-key"
    run env MCNF_AGE_KEY="$vm_key" MCNF_NODE_ID="control-vm-1" bash "$0" init-self
    vm_recip="$(age-keygen -y "$vm_key" 2>/dev/null)"

    mode="$(stat -c '%a' "$vm_key" 2>/dev/null || stat -f '%Lp' "$vm_key" 2>/dev/null)"
    [ "$mode" = "600" ] && pass "VM identity file is 0600" || bad "VM identity file mode is $mode, expected 600"
    [ -n "$(_get "/mcnf/age-recipients/control-vm-1" 2>/dev/null)" ] && pass "init-self registered the VM recipient" || bad "VM recipient not registered"

    # BEFORE reseal: the VM key CANNOT read the secret (sealed only to operator).
    if age -d -i "$vm_key" < <(_get "/mcnf/secret/do-token") >/dev/null 2>&1; then
      bad "VM key could decrypt BEFORE reseal (should not)"
    else
      pass "VM key cannot decrypt before reseal (expected)"
    fi

    # --- Operator runs the REAL reseal-to <vm-recipient> arm ---
    run env MCNF_AGE_KEY="$op_key" bash "$0" reseal-to "$vm_recip"

    # AFTER reseal: BOTH the operator key AND the VM key decrypt the SAME secret.
    g_op="$(age -d -i "$op_key" < <(_get "/mcnf/secret/do-token") 2>/dev/null || true)"
    g_vm="$(age -d -i "$vm_key" < <(_get "/mcnf/secret/do-token") 2>/dev/null || true)"
    [ "$g_op" = "$SECRET_VALUE" ] && pass "operator still decrypts after reseal" || bad "operator lost access after reseal"
    [ "$g_vm" = "$SECRET_VALUE" ] && pass "VM key decrypts SAME secret after reseal (multi-recipient)" || bad "VM key still cannot decrypt after reseal"

    marker="$(_get "$RESEAL_MARKER" 2>/dev/null || true)"
    case "$marker" in *'"status":"completed"'*) pass "reseal completion marker is 'completed'" ;; *) bad "reseal marker not completed: $marker" ;; esac

    # --- rotate <name> on the resealed store, with a new value ---
    NEW_VALUE="do-token-ROTATED-$RANDOM"
    printf %s "$NEW_VALUE" | run env MCNF_AGE_KEY="$op_key" bash "$0" rotate do-token
    r_vm="$(age -d -i "$vm_key" < <(_get "/mcnf/secret/do-token") 2>/dev/null || true)"
    [ "$r_vm" = "$NEW_VALUE" ] && pass "rotate replaced the value, still multi-recipient" || bad "rotate did not update for the VM key"
    rc=0; run env MCNF_AGE_KEY="$op_key" bash "$0" rotate no-such-secret || rc=$?
    [ "$rc" -eq 3 ] && pass "rotate of an absent secret exits 3" || bad "rotate of absent secret exited $rc (want 3)"

    # --- NO secret VALUE and NO private key ever logged (acceptance) ---
    if grep -q -- "$SECRET_VALUE" "$work/run.log" || grep -q -- "$NEW_VALUE" "$work/run.log"; then
      bad "a secret VALUE leaked into the run log"
    else
      pass "no secret value appears in any logged output"
    fi
    if grep -q -- "AGE-SECRET-KEY" "$work/run.log"; then
      bad "an age PRIVATE key leaked into the run log"
    else
      pass "no age private key appears in any logged output"
    fi

    rm -rf "$MOCK_DIR" "$work"
    if [ "$fail" -eq 0 ]; then echo "selftest: ALL PASS"; else echo "selftest: FAILURES" >&2; fi
    exit "$fail"
    ;;

  selftest-scope)
    # WL-SEC-003 offline self-test: role/scope-targeted sealing with REAL age
    # keypairs + a mocked etcd roster. Touches NO live store. Drives the REAL
    # init-self/put arms (re-invoking $0 with the mock active). Proves:
    #   (1) role-match-decrypts  — a role:lighthouse secret decrypts with a
    #                              lighthouse node's key,
    #   (2) role-mismatch-fails  — ...and FAILS with a workstation-only key,
    #   (3) whole-mesh-default   — an unscoped secret decrypts with BOTH,
    #   (4) scope:<s> resolves to exactly the capability-tagged node (excluding a
    #       same-role node without the tag),
    #   (5) rotate preserves a secret's scope; a bulk reseal does NOT widen it,
    #   (6) --scope matching no node is refused (never seal to nobody),
    #   (7) NO secret value / private key is ever logged.
    MOCK_DIR="$(mktemp -d)"; export MOCK_DIR
    ETCD="mock://selftest"
    work="$(mktemp -d)"
    fail=0
    pass() { printf '  PASS %s\n' "$1"; }
    bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
    run() { env MCNF_SECRET_SELFTEST=1 MOCK_DIR="$MOCK_DIR" "$@" >>"$work/run.log" 2>&1; }
    : >"$work/run.log"
    dec() { age -d -i "$1" < <(_get "/mcnf/secret/$2") 2>/dev/null || true; }  # <key> <name> -> plaintext (empty on fail)

    echo "mcnf-secret selftest-scope (mocked etcd at $MOCK_DIR — NO live store touched)"
    SECRET_VALUE="do-token-SCOPED-$RANDOM-$$"

    # Operator (holder of the current mesh key) + a fresh store.
    op_key="$work/op-key"
    run env MCNF_AGE_KEY="$op_key" bash "$0" init

    # Four nodes, each minting its OWN identity + publishing role/scope tags:
    #   lh1/lh2  role:lighthouse         med  role:workstation + scope:media
    #   ws1      role:workstation
    lh_key="$work/lh-key"
    run env MCNF_AGE_KEY="$lh_key" MCNF_NODE_ID=lh1 MCNF_NODE_ROLE=lighthouse bash "$0" init-self
    lh2_key="$work/lh2-key"
    run env MCNF_AGE_KEY="$lh2_key" MCNF_NODE_ID=lh2 MCNF_NODE_ROLE=lighthouse bash "$0" init-self
    med_key="$work/med-key"
    run env MCNF_AGE_KEY="$med_key" MCNF_NODE_ID=med MCNF_NODE_ROLE=workstation MCNF_NODE_SCOPES=media bash "$0" init-self
    ws_key="$work/ws-key"
    run env MCNF_AGE_KEY="$ws_key" MCNF_NODE_ID=ws1 MCNF_NODE_ROLE=workstation bash "$0" init-self

    # Thin-lighthouse policy — a lighthouse may not publish media/fileshare
    # capability tags, and rejection happens before any identity is registered.
    bad_lh_key="$work/bad-lh-key"
    for forbidden_scope in media fileshare file_share file-sharing filesharing; do
      rc=0
      run env MCNF_AGE_KEY="$bad_lh_key" MCNF_NODE_ID="bad-lh-$forbidden_scope" \
        MCNF_NODE_ROLE=lighthouse MCNF_NODE_SCOPES="$forbidden_scope" bash "$0" init-self || rc=$?
      [ "$rc" -ne 0 ] \
        && pass "lighthouse + $forbidden_scope scope is refused before registration" \
        || bad "lighthouse + $forbidden_scope scope was accepted"
    done

    # (1)+(2) role:lighthouse — seal, then LH keys decrypt, the WS key FAILS.
    printf %s "$SECRET_VALUE" | run env MCNF_AGE_KEY="$op_key" bash "$0" put do-token-lh --scope role:lighthouse
    [ "$(dec "$lh_key" do-token-lh)" = "$SECRET_VALUE" ] && pass "role:lighthouse secret decrypts with a lighthouse key (role-match-decrypts)" || bad "lighthouse key could not decrypt a role:lighthouse secret"
    [ "$(dec "$lh2_key" do-token-lh)" = "$SECRET_VALUE" ] && pass "role:lighthouse secret also decrypts with a second thin-lighthouse key" || bad "second lighthouse key could not decrypt a role:lighthouse secret"
    if [ -n "$(dec "$ws_key" do-token-lh)" ]; then bad "workstation-only key COULD decrypt a role:lighthouse secret (role-mismatch-fails)"; else pass "workstation-only key CANNOT decrypt a role:lighthouse secret (role-mismatch-fails)"; fi

    # (3) whole-mesh default — unscoped put reaches BOTH a lighthouse and the ws.
    printf %s "$SECRET_VALUE" | run env MCNF_AGE_KEY="$op_key" bash "$0" put do-token-all
    if [ "$(dec "$lh_key" do-token-all)" = "$SECRET_VALUE" ] && [ "$(dec "$ws_key" do-token-all)" = "$SECRET_VALUE" ]; then
      pass "whole-mesh default seals to ALL nodes (lighthouse + workstation both decrypt)"
    else
      bad "whole-mesh default did not reach all nodes"
    fi

    # (4) scope-preserving rotate + reseal — the do-token-lh secret stays
    #     lighthouse-only (never silently widened to the workstation). Run the
    #     reseal before creating the separate scope:media secret: no one node
    #     should need both role roots and a media capability root.
    NEW_VALUE="do-token-lh-ROTATED-$RANDOM"
    printf %s "$NEW_VALUE" | run env MCNF_AGE_KEY="$lh_key" bash "$0" rotate do-token-lh
    [ "$(dec "$lh_key" do-token-lh)" = "$NEW_VALUE" ] && pass "rotate (no --scope) preserved the value + lighthouse scope" || bad "rotate did not preserve the scoped secret for the lighthouse key"
    if [ -n "$(dec "$ws_key" do-token-lh)" ]; then bad "rotate silently WIDENED a scoped secret to the workstation"; else pass "rotate kept the scoped secret out of the workstation's reach"; fi
    run env MCNF_AGE_KEY="$lh2_key" bash "$0" reseal-all
    [ "$(dec "$lh_key" do-token-lh)" = "$NEW_VALUE" ] && pass "bulk reseal-all kept the scoped secret readable by its lighthouse recipients" || bad "bulk reseal-all lost the scoped secret for the lighthouse key"
    if [ -n "$(dec "$ws_key" do-token-lh)" ]; then bad "bulk reseal-all WIDENED a scoped secret to the workstation"; else pass "bulk reseal-all preserved the scoped secret's blast radius"; fi

    # (5) scope:media — only the explicitly tagged non-lighthouse media host
    # decrypts; thin lighthouses cannot carry this capability.
    printf %s "$SECRET_VALUE" | run env MCNF_AGE_KEY="$op_key" bash "$0" put do-token-media --scope scope:media
    [ "$(dec "$med_key" do-token-media)" = "$SECRET_VALUE" ] && pass "scope:media secret decrypts with the media-tagged host" || bad "media host could not decrypt a scope:media secret"
    if [ -n "$(dec "$lh_key" do-token-media)" ]; then bad "plain lighthouse COULD decrypt a scope:media secret (should not)"; else pass "plain-lighthouse key CANNOT decrypt a scope:media secret (scope excludes it)"; fi

    # (6) --scope matching no node is refused, and writes nothing.
    rc=0; printf %s "$SECRET_VALUE" | run env MCNF_AGE_KEY="$op_key" bash "$0" put do-token-none --scope role:relay || rc=$?
    [ "$rc" -ne 0 ] && pass "put --scope matching no node is refused (never seal to nobody)" || bad "put --scope with no matching node did not fail"
    if _get "/mcnf/secret/do-token-none" >/dev/null 2>&1; then bad "a refused scoped put still wrote a secret"; else pass "a refused scoped put wrote nothing"; fi

    # (7) NO secret value / private key ever logged.
    if grep -q -- "$SECRET_VALUE" "$work/run.log" || grep -q -- "$NEW_VALUE" "$work/run.log"; then bad "a secret VALUE leaked into the run log"; else pass "no secret value appears in any logged output"; fi
    if grep -q -- "AGE-SECRET-KEY" "$work/run.log"; then bad "an age PRIVATE key leaked into the run log"; else pass "no age private key appears in any logged output"; fi

    rm -rf "$MOCK_DIR" "$work"
    if [ "$fail" -eq 0 ]; then echo "selftest-scope: ALL PASS"; else echo "selftest-scope: FAILURES" >&2; fi
    exit "$fail"
    ;;

  *)
    echo "usage: $0 {init|init-self|put <name> [--scope role:<r>|scope:<s>]|get <name>|rotate <name> [--scope ...]|reseal-to <recipient>|reseal-all|recipients [--scope ...]|list|selftest|selftest-scope}" >&2
    exit 2
    ;;
esac
