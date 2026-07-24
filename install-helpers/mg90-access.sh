#!/usr/bin/env bash
# mg90-access.sh — one operator/agent entry point for the MG90 communication planes.
#
# The adapter deliberately exposes the four useful, bounded channels instead of
# making callers rediscover transport details:
#   ssh-probe             read-only root/OS identity probe over pinned SSH
#   ssh-exec -- COMMAND   an explicitly requested root command over pinned SSH
#   lci-get PATH          authenticated MG-LCI HTTP GET (port 80)
#   app-get PATH          authenticated MG90 application HTTP GET (port 11532)
#   status-listen PORT     receive documented JSON status broadcasts over UDP
#   gps-udp-listen PORT    receive forwarded NMEA/TAIP UDP (default 5067)
#   gps-tcp-connect PORT   consume the MG90 TCP GPS stream (default 9345)
#   inventory              read-only reachability/auth inventory, no mutations
#
# Passwords are read from root-owned files only. They never appear in argv. The
# SSH host key is pinned through mg90-known-hosts (or the installed equivalent).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MG90_HOST="${MG90_HOST:-172.20.0.25}"
MG90_SSH_PORT="${MG90_SSH_PORT:-2222}"
MG90_SSH_USER="${MG90_SSH_USER:-root}"
MG90_ROOT_PASSWORD_FILE="${MG90_ROOT_PASSWORD_FILE:-/etc/mackesd/mg90-root-password}"
MG90_HTTP_USER="${MG90_HTTP_USER:-admin}"
MG90_HTTP_PASSWORD_FILE="${MG90_HTTP_PASSWORD_FILE:-/etc/mackesd/mg90-http-password}"
MG90_KNOWN_HOSTS_FILE="${MG90_KNOWN_HOSTS_FILE:-/etc/mackesd/mg90_known_hosts}"

# A checkout-local pin is useful before the RPM has installed the system pin. It
# is still strict: an absent or mismatching key fails closed.
if [[ ! -r "$MG90_KNOWN_HOSTS_FILE" ]]; then
    for candidate in "$HERE/mg90-known-hosts" "$HERE/../../share/magic-mesh/mg90-known-hosts"; do
        if [[ -r "$candidate" ]]; then
            MG90_KNOWN_HOSTS_FILE="$candidate"
            break
        fi
    done
fi

die() {
    echo "mg90-access: $*" >&2
    exit 2
}

usage() {
    cat <<'USAGE'
Usage:
  mg90-access.sh --self-test
  mg90-access.sh ssh-probe
  mg90-access.sh ssh-exec -- COMMAND [ARG ...]
  mg90-access.sh lci-get PATH
  mg90-access.sh app-get PATH
  mg90-access.sh status-listen PORT
  mg90-access.sh gps-udp-listen [PORT]
  mg90-access.sh gps-tcp-connect [PORT]
  mg90-access.sh inventory

Environment overrides are intentionally explicit:
  MG90_HOST, MG90_SSH_PORT, MG90_SSH_USER
  MG90_ROOT_PASSWORD_FILE       root-only file, default /etc/mackesd/mg90-root-password
  MG90_HTTP_USER                default admin
MG90_HTTP_PASSWORD_FILE       root-only file, default /etc/mackesd/mg90-http-password
MG90_KNOWN_HOSTS_FILE         pinned host keys, default /etc/mackesd/mg90_known_hosts
MG90_GPS_UDP_PORT              default 5067 for gps-udp-listen
MG90_GPS_TCP_PORT              default 9345 for gps-tcp-connect
USAGE
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

root_only_file() {
    local file="$1" label="$2" mode owner
    [[ -r "$file" ]] || die "$label is missing or unreadable: $file"
    mode="$(stat -c '%a' -- "$file" 2>/dev/null)" || die "cannot stat $label: $file"
    owner="$(stat -c '%u' -- "$file" 2>/dev/null)" || die "cannot stat $label: $file"
    [[ "$owner" == 0 ]] || die "$label must be owned by root: $file"
    case "$mode" in
        400|600) ;;
        *) die "$label must have mode 0400 or 0600 (got $mode): $file" ;;
    esac
}

password_file() {
    local file="$1" label="$2"
    root_only_file "$file" "$label"
    [[ -n "$(head -n 1 -- "$file")" ]] || die "$label is empty: $file"
}

# Keep authenticated GETs on the pinned MG90 origin and make the request target
# unambiguous. A raw URL, scheme-relative path, fragment, control character, or
# traversal segment must never reach curl's redirect/parser surface.
safe_http_path() {
    local path="$1" path_without_query segment
    [[ "$path" == /* && "$path" != //* ]] || return 1
    [[ "$path" != *[[:space:]]* ]] || return 1
    [[ "$path" != *$'\n'* && "$path" != *$'\r'* && "$path" != *$'\t'* ]] || return 1
    [[ "$path" != *'\\'* && "$path" != *'#'* ]] || return 1
    path_without_query="${path%%\?*}"
    local -a segments=()
    IFS='/' read -r -a segments <<< "$path_without_query"
    for segment in "${segments[@]}"; do
        case "$segment" in
            .|..) return 1 ;;
        esac
    done
}

validate_http_path() {
    local path="$1"
    safe_http_path "$path" || die "HTTP path must be an absolute, local, non-traversing path: $path"
}

known_hosts() {
    [[ -r "$MG90_KNOWN_HOSTS_FILE" ]] || die "pinned known-hosts file is missing: $MG90_KNOWN_HOSTS_FILE"
    ssh-keygen -F "[$MG90_HOST]:$MG90_SSH_PORT" -f "$MG90_KNOWN_HOSTS_FILE" >/dev/null 2>&1 \
        || die "no pinned key for [$MG90_HOST]:$MG90_SSH_PORT in $MG90_KNOWN_HOSTS_FILE"
}

ssh_opts=(
    "-p" "$MG90_SSH_PORT"
    "-o" "BatchMode=no"
    "-o" "NumberOfPasswordPrompts=1"
    "-o" "ConnectTimeout=8"
    "-o" "ConnectionAttempts=1"
    "-o" "StrictHostKeyChecking=yes"
    "-o" "GlobalKnownHostsFile=/dev/null"
    "-o" "UserKnownHostsFile=$MG90_KNOWN_HOSTS_FILE"
    "-o" "HostKeyAlgorithms=+ssh-rsa"
    "-o" "KexAlgorithms=+diffie-hellman-group1-sha1,diffie-hellman-group14-sha1"
    "-o" "PubkeyAcceptedAlgorithms=+ssh-rsa"
    "-o" "Ciphers=+aes128-cbc,3des-cbc"
    "-o" "PreferredAuthentications=password"
    "-o" "PubkeyAuthentication=no"
    "-o" "LogLevel=ERROR"
)

ssh_exec() {
    local command="$1"
    shift
    need sshpass
    password_file "$MG90_ROOT_PASSWORD_FILE" "MG90 root password file"
    known_hosts
    set +e
    sshpass -f "$MG90_ROOT_PASSWORD_FILE" ssh "${ssh_opts[@]}" \
        "$MG90_SSH_USER@$MG90_HOST" "$command" "$@"
    local rc=$?
    set -e
    if [[ "$rc" -eq 5 || "$rc" -eq 255 ]]; then
        echo "mg90-access: SSH failed (check the current root credential and host pin; do not guess passwords)" >&2
    fi
    return "$rc"
}

http_get() {
    local port="$1" path="$2" jar post base login
    need curl
    need python3
    password_file "$MG90_HTTP_PASSWORD_FILE" "MG90 HTTP password file"
    [[ "$path" == /* ]] || path="/$path"
    validate_http_path "$path"
    jar="$(mktemp)"
    post="$(mktemp)"
    chmod 600 -- "$post"
    trap 'rm -f -- "$jar" "$post"' RETURN
    base="http://$MG90_HOST:$port"
    login="$base/j_security_check"
    curl --silent --show-error --fail --max-time 12 --max-redirs 3 \
        -c "$jar" -b "$jar" -L "$base/" >/dev/null
    # Build the form from the password file so the password is neither in argv
    # nor sent with a trailing newline from the credential file.
    python3 - "$MG90_HTTP_USER" "$MG90_HTTP_PASSWORD_FILE" "$post" <<'PY'
import pathlib
import sys
import urllib.parse

user, password_file, output_file = sys.argv[1:]
password = pathlib.Path(password_file).read_text().splitlines()[0]
payload = urllib.parse.urlencode({"j_username": user, "j_password": password})
pathlib.Path(output_file).write_text(payload)
PY
    curl --silent --show-error --fail --max-time 12 --max-redirs 3 \
        -c "$jar" -b "$jar" -L \
        --data-binary "@$post" \
        "$login" >/dev/null
    # The target itself is not followed: login redirects are expected, but a
    # device-specific status page must not turn into an arbitrary cross-origin
    # fetch after authentication.
    curl --silent --show-error --fail --max-time 12 -b "$jar" "$base$path"
    rm -f -- "$jar"
    rm -f -- "$post"
    trap - RETURN
}

valid_port() {
    if ! [[ "${1:-}" =~ ^[0-9]+$ ]]; then
        die "port must be an integer from 1 to 65535: ${1:-<missing>}"
    fi
    if (( 1 > 10#$1 || 10#$1 > 65535 )); then
        die "port must be an integer from 1 to 65535: ${1:-<missing>}"
    fi
}

udp_listen() {
    local port="$1"
    valid_port "$port"
    need python3
    python3 - "$port" <<'PY'
import socket
import sys

port = int(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("0.0.0.0", port))
print(f"mg90-access: UDP listening on 0.0.0.0:{port}", flush=True)
while True:
    payload, peer = sock.recvfrom(65535)
    sys.stdout.write(f"[{peer[0]}:{peer[1]}] ")
    sys.stdout.buffer.write(payload)
    if not payload.endswith(b"\n"):
        sys.stdout.write("\n")
    sys.stdout.flush()
PY
}

tcp_connect() {
    local port="$1"
    valid_port "$port"
    need python3
    python3 - "$MG90_HOST" "$port" <<'PY'
import socket
import sys

host, port = sys.argv[1], int(sys.argv[2])
with socket.create_connection((host, port), timeout=10) as sock:
    print(f"mg90-access: TCP connected to {host}:{port}", flush=True)
    while True:
        payload = sock.recv(65535)
        if not payload:
            break
        sys.stdout.buffer.write(payload)
        sys.stdout.flush()
PY
}

self_test() {
    [[ "$MG90_SSH_PORT" =~ ^[0-9]+$ ]] || die "MG90_SSH_PORT must be numeric"
    [[ -n "$MG90_HOST" && -n "$MG90_SSH_USER" ]] || die "MG90_HOST and MG90_SSH_USER are required"
    [[ "$MG90_HOST" != *[[:space:]/\\]* ]] || die "MG90_HOST contains forbidden characters"
    [[ "$MG90_SSH_USER" != *[[:space:]/\\]* ]] || die "MG90_SSH_USER contains forbidden characters"
    need bash
    need stat
    need ssh-keygen
    safe_http_path "/MG-LCI/status/general.html"
    safe_http_path "/MG-LCI/wan/status/status.html?displayExtended=true"
    ! safe_http_path "http://example.invalid/redirect"
    ! safe_http_path "//example.invalid/redirect"
    ! safe_http_path "/MG-LCI/../secret"
    ! safe_http_path $'/MG-LCI/status\n'
    echo "mg90-access: self-test passed (no network operation performed)"
}

command_name="${1:-}"
case "$command_name" in
    --help|-h)
        usage
        ;;
    --self-test)
        self_test
        ;;
    ssh-probe)
        shift
        [[ "$#" -eq 0 ]] || die "ssh-probe takes no arguments"
        ssh_exec 'id -u; hostname; uname -srm; cat /var/run/omgtime.g.info'
        ;;
    ssh-exec)
        shift
        [[ "${1:-}" == "--" ]] || die "ssh-exec requires -- before the remote command"
        shift
        [[ "$#" -gt 0 ]] || die "ssh-exec requires a remote command"
        ssh_exec "$@"
        ;;
    lci-get)
        shift
        [[ "$#" -eq 1 ]] || die "lci-get requires exactly one path"
        http_get 80 "$1"
        ;;
    app-get)
        shift
        [[ "$#" -eq 1 ]] || die "app-get requires exactly one path"
        http_get 11532 "$1"
        ;;
    status-listen)
        shift
        [[ "$#" -eq 1 ]] || die "status-listen requires the configured UDP broadcast port"
        udp_listen "$1"
        ;;
    gps-udp-listen)
        shift
        [[ "$#" -le 1 ]] || die "gps-udp-listen takes zero or one port"
        udp_listen "${1:-${MG90_GPS_UDP_PORT:-5067}}"
        ;;
    gps-tcp-connect)
        shift
        [[ "$#" -le 1 ]] || die "gps-tcp-connect takes zero or one port"
        tcp_connect "${1:-${MG90_GPS_TCP_PORT:-9345}}"
        ;;
    inventory)
        shift
        [[ "$#" -eq 0 ]] || die "inventory takes no arguments"
        need curl
        printf 'MG90 host=%s ssh_port=%s lci_port=80 app_port=11532\n' "$MG90_HOST" "$MG90_SSH_PORT"
        printf 'tcp: '
        if (exec 3<>"/dev/tcp/$MG90_HOST/$MG90_SSH_PORT") 2>/dev/null; then
            exec 3>&-
            exec 3<&-
            printf 'ssh-up '
        else
            printf 'ssh-down '
        fi
        (curl --silent --output /dev/null --max-time 3 "http://$MG90_HOST/" && printf 'lci-up ') || printf 'lci-down '
        (curl --silent --output /dev/null --max-time 3 "http://$MG90_HOST:11532/" && printf 'app-up\n') || printf 'app-down\n'
        printf 'ssh: pinned-and-credential-gated (run ssh-probe for a live check)\n'
        ;;
    '')
        usage
        exit 2
        ;;
    *)
        die "unknown command: $command_name"
        ;;
esac
