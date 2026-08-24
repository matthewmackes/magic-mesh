#!/usr/bin/env bash
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
unit="$repo/packaging/systemd/mcnf-collaboration-identity.service"
dropin="$repo/packaging/systemd/mackesd-collaboration-identity.conf"
optional="$repo/packaging/systemd/mackesd-collaboration-identity-optional.conf"
manifest="$repo/crates/mesh/mackesd/Cargo.toml"
grep -Fxq 'WantedBy=mackesd.target' "$unit"
grep -Fxq 'Requires=mcnf-collaboration-identity.service' "$dropin"
grep -Fxq 'After=mcnf-collaboration-identity.service' "$dropin"
grep -Fxq 'Wants=mcnf-collaboration-identity.service' "$optional"
grep -Fxq 'After=mcnf-collaboration-identity.service' "$optional"
if grep -Fxq 'Requires=mcnf-collaboration-identity.service' "$optional"; then
  echo 'compute/observation drop-in must not hard-require collaboration identity' >&2
  exit 1
fi
for service in control observation actions data compute integrations; do
  grep -Eq "^Before=.*mackesd-${service}\\.service" "$unit"
done
for asset in produce-collaboration-identity-receipt.py materialize-collaboration-identity.py mcnf-collaboration-identity.service mackesd-collaboration-identity-optional.conf; do
  grep -Fq "$asset" "$manifest"
done
for service in control actions data integrations; do
  grep -Fq "packaging/systemd/mackesd-collaboration-identity.conf\", dest = \"/usr/lib/systemd/system/mackesd-${service}.service.d/40-collaboration-identity.conf" "$manifest"
done
for service in observation compute; do
  grep -Fq "packaging/systemd/mackesd-collaboration-identity-optional.conf\", dest = \"/usr/lib/systemd/system/mackesd-${service}.service.d/40-collaboration-identity.conf" "$manifest"
done
grep -Eq '^systemctl enable .*mcnf-collaboration-identity.service' "$manifest"
if sed '/^Requires=mcnf-collaboration-identity.service$/d' "$dropin" | grep -Fxq 'Requires=mcnf-collaboration-identity.service'; then
  echo 'hostile activation fixture retained duplicate requirement' >&2; exit 1
fi
echo 'test-collaboration-identity-activation: ordering contract passed'
