#!/usr/bin/env bash
# WL-ARCH-008 — keep the Browser surface bound to the Browser VM/VDI route.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WEB="$ROOT/crates/desktop/mde-shell-egui/src/web/mod.rs"
MAIN="$ROOT/crates/desktop/mde-shell-egui/src/main.rs"
SURFACES="$ROOT/crates/desktop/mde-shell-egui/src/surfaces.rs"
WORKLOAD="$ROOT/crates/mesh/mackesd/src/workers/cloud/verbs/browser.rs"

die() {
    echo "verify-browser-vm-activation: $*" >&2
    exit 1
}

for file in "$WEB" "$MAIN" "$SURFACES" "$WORKLOAD"; do
    [ -f "$file" ] || die "activation seam is missing: $file"
    [ ! -L "$file" ] || die "activation seam must not be a symlink: $file"
done

require() {
    local needle=$1 file=$2
    grep -Fq -- "$needle" "$file" || die "missing '$needle' in $file"
}

require 'const VM_WORKLOAD: &str = "browser-vm"' "$WEB"
require 'workload: VM_WORKLOAD' "$WEB"
require 'preferred: BrowserVmTransport::Rdp' "$WEB"
require 'alternate: Some(BrowserVmTransport::Sunshine)' "$WEB"
require 'resume: true' "$WEB"
require 'BrowserVmRoute::select_resume()' "$WEB"
require 'Surface::Browser' "$SURFACES"
require 'VisualBoundary::BrowserVmGuest' "$SURFACES"
require 'DeliveryType::DesktopVm' "$WORKLOAD"
require 'const BROWSER_VM_WORKLOAD_NAME: &str = "browser-vm"' "$WORKLOAD"
require 'BrowserVmProfile::default().workload_spec(node, name)' "$WORKLOAD"

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
