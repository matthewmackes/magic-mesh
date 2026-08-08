#!/usr/bin/env bash
# WL-ARCH-008 — keep the Browser surface bound to the Browser VM/VDI route.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WEB="$ROOT/crates/desktop/mde-shell-egui/src/web/mod.rs"
MAIN="$ROOT/crates/desktop/mde-shell-egui/src/main.rs"
SURFACES="$ROOT/crates/desktop/mde-shell-egui/src/surfaces.rs"
WORKLOAD="$ROOT/crates/mesh/mackesd/src/workers/cloud/verbs/browser.rs"
REQUEST_HELPER="$ROOT/install-helpers/request-browser-vm-workload.sh"
MIGRATION_HELPER="$ROOT/packaging/browser-vm/migrate-display1-domain.sh"

die() {
    echo "verify-browser-vm-activation: $*" >&2
    exit 1
}

for file in "$WEB" "$MAIN" "$SURFACES" "$WORKLOAD" "$REQUEST_HELPER" "$MIGRATION_HELPER"; do
    [ -f "$file" ] || die "activation seam is missing: $file"
    [ ! -L "$file" ] || die "activation seam must not be a symlink: $file"
done

require() {
    local needle=$1 file=$2
    grep -Fq -- "$needle" "$file" || die "missing '$needle' in $file"
}

require 'const VM_WORKLOAD: &str = "browser-vm"' "$WEB"
require 'workload: VM_WORKLOAD' "$WEB"
require 'struct BrowserVmRoute' "$WEB"
require 'struct BrowserVmConnect' "$WEB"
require 'resume: true' "$WEB"
require 'BrowserVmRoute::select_resume()' "$WEB"
require 'Surface::Browser' "$SURFACES"
require 'VisualBoundary::BrowserVmGuest' "$SURFACES"
require 'DeliveryType::DesktopVm' "$WORKLOAD"
require 'const BROWSER_VM_WORKLOAD_NAME: &str = "browser-vm"' "$WORKLOAD"
require 'BrowserVmProfile::default().workload_spec(node, name)' "$WORKLOAD"
require 'ACTION_TOPIC = "action/workload/operation"' "$REQUEST_HELPER"
require 'DEFAULT_ACTION = "start_and_attach"' "$REQUEST_HELPER"
require 'SUPPORTED_ACTIONS = frozenset({' "$REQUEST_HELPER"
require 'IMAGE_REQUIRED_ACTIONS = frozenset({"start_and_attach", "start"})' "$REQUEST_HELPER"
require 'EXISTING_WORKLOAD_ACTIONS = frozenset({"stop", "restart", "resume", "destroy"})' "$REQUEST_HELPER"
require 'workload-not-admitted' "$REQUEST_HELPER"
require 'ATTACHMENT = "qemu_display1_dmabuf"' "$REQUEST_HELPER"
require 'workload_id(node)' "$REQUEST_HELPER"
require 'guest did not shut down within the bounded window' "$MIGRATION_HELPER"
require 'never force-destroyed' "$MIGRATION_HELPER"
require 'type": "dbus"' "$MIGRATION_HELPER"

if grep -Fq -- 'action/cloud/browser-provision' "$REQUEST_HELPER"; then
    die 'Browser helper still publishes the retired browser-provision action'
fi

# The caller may submit only the existing Workload actions; backend lifecycle
# commands remain unreachable from the Browser package boundary.
for forbidden in virsh qemu-system systemctl systemd-run podman ansible-playbook; do
    if grep -Fq -- "$forbidden" "$REQUEST_HELPER"; then
        die "Browser caller contains a direct backend command: $forbidden"
    fi
done

# The reachable shell Browser surface must contain no host helper seam.
for forbidden in 'mde-web-preview' 'mde-web-cef' 'WebSession::spawn' 'BrowserEngine' 'live-helper' 'MDE_CEF' 'MDE_WEB'; do
    if grep -Fq -- "$forbidden" "$WEB" "$MAIN"; then
        die "host Browser seam remains reachable: $forbidden"
    fi
done

# The guest contract is retained in magic-mesh, but the extracted host engine
# and helper source must not quietly return alongside it.
for forbidden_path in \
    crates/desktop/mde-web-cef \
    crates/desktop/mde-web-preview \
    crates/desktop/mde-web-preview-client \
    crates/desktop/mde-web-sandbox \
    crates/desktop/mde-web-wire \
    crates/mesh/mde-browser-workers \
    packaging/browser; do
    if [[ -e "$ROOT/$forbidden_path" || -L "$ROOT/$forbidden_path" ]]; then
        die "extracted host Browser path still exists: $forbidden_path"
    fi
done

echo "Browser VM activation contract passed: Surface::Browser -> browser-vm -> DesktopVm/VDI"
