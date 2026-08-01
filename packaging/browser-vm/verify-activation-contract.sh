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

require 'enum BrowserVmWorkload' "$WEB"
require 'Self::BrowserVm => "browser-vm"' "$WEB"
require 'workload: BrowserVmWorkload::BrowserVm' "$WEB"
require 'transport: BrowserVmTransport::SunshineMoonlight' "$WEB"
require 'alternate_transport: BrowserVmTransport::Rdp' "$WEB"
require 'resume: true' "$WEB"
require 'self.browser_vm_route = Some(BrowserVmRoute::select_resume());' "$WEB"
require 'self.web.ensure_live_tab(seat_present);' "$MAIN"
require 'Surface::Browser' "$SURFACES"
require 'VisualBoundary::BrowserVmGuest' "$SURFACES"
require 'DeliveryType::DesktopVm' "$WORKLOAD"
require 'BrowserVmProfile::default().workload_spec(node, name)' "$WORKLOAD"

# Once a Browser VM route exists, user tab/reload actions must not reach the
# legacy helper constructors. Keep these checks scoped to the guarded methods
# so unrelated historical compatibility code cannot satisfy the contract.
drain_block=$(mktemp)
respawn_block=$(mktemp)
trap 'rm -f "$drain_block" "$respawn_block"' EXIT
awk '/fn drain_live_tab_requests\(/,/^    fn respawn_live\(/' "$WEB" > "$drain_block"
awk '/fn respawn_live\(/,/^    fn open_with\(/' "$WEB" > "$respawn_block"
[ -s "$drain_block" ] || die "could not isolate Browser tab activation guard"
[ -s "$respawn_block" ] || die "could not isolate Browser reload activation guard"

require 'if self.browser_vm_route.is_some()' "$drain_block"
require 'self.open_requested.clear();' "$drain_block"
require 'if self.browser_vm_route.is_some()' "$respawn_block"
require 'guest recovery is pending' "$respawn_block"

# The guarded route is allowed to wait for VDI, but it must not reach a host
# session constructor before the route check. This ordering assertion catches
# a future fallback inserted above the existing early return.
guard_precedes_spawn() {
    local label=$1 block=$2 guard_line spawn_line
    guard_line=$(grep -n -m1 'self\.browser_vm_route\.is_some()' "$block" | cut -d: -f1 || true)
    spawn_line=$(grep -n -m1 'WebSession::spawn' "$block" | cut -d: -f1 || true)
    if [ -n "$spawn_line" ] && { [ -z "$guard_line" ] || [ "$guard_line" -ge "$spawn_line" ]; }; then
        die "$label reaches a host session constructor before the Browser VM guard"
    fi
}
guard_precedes_spawn "Browser activation" "$drain_block"
guard_precedes_spawn "Browser reload" "$respawn_block"

echo "Browser VM activation contract passed: Surface::Browser -> browser-vm -> DesktopVm/VDI"
