#!/usr/bin/env bash
# Convert an existing Browser VM definition to the Workload Display1 contract.
#
# The migration never deletes, recreates, copies, or resizes a guest disk.  A
# running guest receives only a normal ACPI shutdown; if it does not stop in
# the bounded window, this script exits before touching its definition.
set -euo pipefail
set +x
umask 077

readonly PROGRAM_NAME=migrate-browser-vm-display1
readonly DEFAULT_DOMAIN=browser-vm
readonly DEFAULT_BACKUP_ROOT=/var/lib/mackesd/browser-vm-migrations
readonly SHUTDOWN_TIMEOUT_SECONDS=120

usage() {
  cat >&2 <<'EOF'
usage:
  migrate-display1-domain.sh inspect --target HOST [--user USER] [--identity PATH] [--domain browser-vm]
  migrate-display1-domain.sh apply --target HOST [--user USER] [--identity PATH] [--domain browser-vm]
  migrate-display1-domain.sh --self-test

`apply` preserves every disk and only changes the inactive libvirt XML: it
adds QEMU Display1 (D-Bus P2P), changes the primary video model to virtio with
3D enabled, and retains the existing SPICE recovery graphics device. A live
guest is asked to shut down normally and is never force-destroyed. The VM is
left stopped so request-browser-vm-workload can start it through the typed
StartAndAttach operation.
EOF
}

fail() { printf '%s: %s\n' "$PROGRAM_NAME" "$1" >&2; exit 1; }
valid_host() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,252}$ ]]; }
valid_user() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$ ]]; }
valid_domain() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$ ]]; }

ssh_args() {
  SSH_ARGS=(ssh -T -o BatchMode=yes -o PasswordAuthentication=no
    -o KbdInteractiveAuthentication=no -o StrictHostKeyChecking=yes
    -o ConnectTimeout=8)
  [[ -z "$IDENTITY" ]] || SSH_ARGS+=(-i "$IDENTITY")
}

remote_run() {
  local mode=$1 destination
  destination="$USER_NAME@$TARGET"
  "${SSH_ARGS[@]}" -- "$destination" sudo -n /usr/bin/python3 - \
    "$DOMAIN_NAME" "$DEFAULT_BACKUP_ROOT" "$mode" "$SHUTDOWN_TIMEOUT_SECONDS" <<'PY'
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

domain, backup_root_raw, mode, timeout_raw = sys.argv[1:]
timeout = int(timeout_raw)

def command(*args):
    result = subprocess.run(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, timeout=15, check=False)
    if result.returncode:
        detail = result.stderr.strip().replace("\n", " ")[:240]
        raise RuntimeError(f"libvirt command failed: {detail or result.returncode}")
    return result.stdout

def domstate():
    return command("/usr/bin/virsh", "--connect", "qemu:///system", "domstate", domain).strip().lower()

def source_fingerprints(root):
    devices = root.find("devices")
    if devices is None:
        raise RuntimeError("domain XML has no devices element")
    fingerprints = []
    for disk in devices.findall("disk"):
        source = disk.find("source")
        if source is None:
            raise RuntimeError("disk XML has no source element")
        attrs = tuple(sorted((key, value) for key, value in source.attrib.items()))
        fingerprints.append((disk.get("type", ""), disk.get("device", ""), attrs))
    if not fingerprints:
        raise RuntimeError("Browser VM has no disks; refusing conversion")
    return tuple(fingerprints)

def display_state(root):
    devices = root.find("devices")
    if devices is None:
        return False, False, 0
    dbus = [entry for entry in devices.findall("graphics") if entry.get("type") == "dbus"]
    video = devices.find("video/model")
    # libvirt normalizes a D-Bus graphics device by dropping the inert
    # `<listen type='none'/>` child from a subsequent dumpxml.  `p2p='yes'`
    # is the authoritative Display1 property and must survive that round trip.
    dbus_ok = len(dbus) == 1 and dbus[0].get("p2p") == "yes"
    video_ok = video is not None and video.get("type") == "virtio" and video.find("acceleration[@accel3d='yes']") is not None
    return dbus_ok, video_ok, len(dbus)

def transform(root):
    before_disks = source_fingerprints(root)
    devices = root.find("devices")
    assert devices is not None
    dbus_entries = [entry for entry in devices.findall("graphics") if entry.get("type") == "dbus"]
    if len(dbus_entries) > 1:
        raise RuntimeError("domain already has multiple Display1 graphics entries")
    if dbus_entries:
        dbus = dbus_entries[0]
        dbus.attrib.clear()
        dbus.set("type", "dbus")
        dbus.set("p2p", "yes")
        for child in list(dbus):
            dbus.remove(child)
    else:
        dbus = ET.Element("graphics", {"type": "dbus", "p2p": "yes"})
        graphics = devices.findall("graphics")
        index = list(devices).index(graphics[0]) if graphics else 0
        devices.insert(index, dbus)
    ET.SubElement(dbus, "listen", {"type": "none"})
    video = devices.find("video")
    if video is None:
        video = ET.SubElement(devices, "video")
    model = video.find("model")
    if model is None:
        model = ET.SubElement(video, "model")
    model.attrib.clear()
    model.set("type", "virtio")
    acceleration = model.find("acceleration")
    if acceleration is None:
        acceleration = ET.SubElement(model, "acceleration")
    acceleration.attrib.clear()
    acceleration.set("accel3d", "yes")
    after_disks = source_fingerprints(root)
    if after_disks != before_disks:
        raise RuntimeError("conversion would change disk sources")
    dbus_ok, video_ok, dbus_count = display_state(root)
    if not dbus_ok or not video_ok or dbus_count != 1:
        raise RuntimeError("converted XML does not satisfy Display1 contract")
    return root

def xml_root():
    raw = command("/usr/bin/virsh", "--connect", "qemu:///system", "dumpxml", domain)
    try:
        return raw, ET.fromstring(raw)
    except ET.ParseError as error:
        raise RuntimeError(f"libvirt returned malformed XML: {error}") from error

state = domstate()
raw, root = xml_root()
disk_count = len(source_fingerprints(root))
dbus_ok, video_ok, dbus_count = display_state(root)
if mode == "inspect":
    print(json.dumps({"schema_version": 1, "domain": domain, "state": state,
                      "disk_count": disk_count, "display1": dbus_ok,
                      "virtio_video": video_ok, "display1_entries": dbus_count,
                      "migration_required": not (dbus_ok and video_ok and dbus_count == 1)},
                     sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)
if mode != "apply":
    raise RuntimeError("invalid migration mode")
if state == "running":
    command("/usr/bin/virsh", "--connect", "qemu:///system", "shutdown", domain)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        state = domstate()
        if state == "shut off":
            break
        time.sleep(1)
    else:
        raise RuntimeError("guest did not shut down within the bounded window; definition unchanged")
elif state != "shut off":
    raise RuntimeError(f"domain is {state!r}; it must be running or shut off")
raw, root = xml_root()
before_disks = source_fingerprints(root)
root = transform(root)
backup_root = Path(backup_root_raw)
backup_root.mkdir(mode=0o700, parents=True, exist_ok=True)
if backup_root.is_symlink() or not backup_root.is_dir():
    raise RuntimeError("backup root is not a real directory")
stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
backup = backup_root / f"{domain}.{stamp}.{hashlib.sha256(raw.encode()).hexdigest()[:16]}.xml"
with backup.open("x", encoding="utf-8") as handle:
    handle.write(raw)
os.chmod(backup, 0o600)
candidate = backup_root / f".{domain}.{stamp}.display1.xml"
ET.indent(root, space="  ")
ET.ElementTree(root).write(candidate, encoding="utf-8", xml_declaration=True)
os.chmod(candidate, 0o600)
try:
    command("/usr/bin/virsh", "--connect", "qemu:///system", "define", str(candidate))
finally:
    candidate.unlink(missing_ok=True)
_raw, verified = xml_root()
if source_fingerprints(verified) != before_disks:
    raise RuntimeError("post-define disk source check failed; original XML is retained in backup")
dbus_ok, video_ok, dbus_count = display_state(verified)
if not dbus_ok or not video_ok or dbus_count != 1:
    raise RuntimeError("post-define Display1 verification failed; original XML is retained in backup")
print(json.dumps({"schema_version": 1, "domain": domain, "state": "shut off",
                  "disk_count": len(before_disks), "display1": True, "virtio_video": True,
                  "backup": str(backup), "next": "request-browser-vm-workload StartAndAttach"},
                 sort_keys=True, separators=(",", ":")))
PY
}

self_test() {
  valid_host 172.20.146.225
  valid_user mm
  valid_domain browser-vm
  ! valid_host 'host;rm -rf /'
  ! valid_user 'bad user'
  ! valid_domain 'browser vm'
  python3 - <<'PY'
import xml.etree.ElementTree as ET
xml = "<domain><devices><disk type='file' device='disk'><source file='/var/lib/mde-vms/browser.qcow2'/></disk><graphics type='spice'/><video><model type='qxl'/></video></devices></domain>"
root = ET.fromstring(xml)
devices = root.find("devices")
dbus = ET.Element("graphics", {"type": "dbus", "p2p": "yes"})
ET.SubElement(dbus, "listen", {"type": "none"})
devices.insert(0, dbus)
model = devices.find("video/model")
model.attrib.clear(); model.set("type", "virtio")
ET.SubElement(model, "acceleration", {"accel3d": "yes"})
assert devices.find("disk/source").get("file") == "/var/lib/mde-vms/browser.qcow2"
assert len([entry for entry in devices.findall("graphics") if entry.get("type") == "dbus"]) == 1
assert devices.find("video/model").get("type") == "virtio"
assert devices.find("video/model/acceleration").get("accel3d") == "yes"
PY
  printf '%s: self-test passed\n' "$PROGRAM_NAME"
}

if [[ "${1:-}" == --self-test ]]; then
  (($# == 1)) || { usage; exit 2; }
  self_test
  exit 0
fi
(( $# >= 1 )) || { usage >&2; exit 2; }
MODE=$1; shift
TARGET= USER_NAME=mm IDENTITY= DOMAIN_NAME=$DEFAULT_DOMAIN
while (($#)); do
  case "$1" in
    --target) (($# >= 2)) || { usage; exit 2; }; TARGET=$2; shift 2 ;;
    --user) (($# >= 2)) || { usage; exit 2; }; USER_NAME=$2; shift 2 ;;
    --identity) (($# >= 2)) || { usage; exit 2; }; IDENTITY=$2; shift 2 ;;
    --domain) (($# >= 2)) || { usage; exit 2; }; DOMAIN_NAME=$2; shift 2 ;;
    *) usage >&2; exit 2 ;;
  esac
done
[[ "$MODE" == inspect || "$MODE" == apply ]] || { usage >&2; exit 2; }
[[ -n "$TARGET" ]] || fail target-required
valid_host "$TARGET" || fail unsafe-target
valid_user "$USER_NAME" || fail unsafe-user
valid_domain "$DOMAIN_NAME" || fail unsafe-domain
[[ -z "$IDENTITY" || ( -f "$IDENTITY" && ! -L "$IDENTITY" ) ]] || fail unsafe-identity
ssh_args
remote_run "$MODE"
