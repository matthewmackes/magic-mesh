# Surface Pro 5/6 acceptance evidence collection

This is a bounded operations procedure, not a second worklist. The only active
tracker remains `docs/platform/WORKLIST.md`.

`install-helpers/collect-surface-acceptance.py` takes a read-only inventory of a
Surface candidate. It does not start or stop services, trigger udev, install
packages, query for firmware updates, enroll keys, change radio state, suspend,
reboot, play or record audio, or open a camera stream. Camera evidence is limited
to libcamera enumeration. Network connection names, addresses, MAC addresses,
and common secret forms are excluded or redacted.

Run it locally on the named seat. Use an output path on a root-only local
filesystem; the tool refuses to overwrite a path and writes files mode `0600`.

```bash
sudo install-helpers/collect-surface-acceptance.py collect \
  --seat Surface --expected-generation 6 \
  --out /var/tmp/surface-pro6-acceptance

sudo install-helpers/collect-surface-acceptance.py validate \
  /var/tmp/surface-pro6-acceptance
```

For the later Surface Pro 5 parity seat, select generation 5 and use a distinct
seat label and output directory. Both generations require exact DMI vendor
`Microsoft Corporation`. Generation 5 requires product name `Surface Pro` plus
exact product SKU `Surface_Pro_1796` (Wi-Fi) or
`Surface_Pro_1807` (LTE). Generation 6 requires product name `Surface Pro 6` and
also records its SKU. A mismatch makes the collection incomplete.

The collector and validator exit `0` only when every inventory probe succeeds,
`3` for a hash-valid but incomplete bundle, and `2` for invalid arguments,
unsafe output, tampering, or corrupt evidence. A `complete` collection is not a
physical acceptance result. The manifest always records
`physical_acceptance_claimed: false` and lists the hands-on checks still needed:
touch, pen, Type Cover, rotation, camera preview, audio, suspend/S0ix recovery,
power/volume buttons, microSD, hibernation support, and boot/upgrade/rollback
recovery. The collector records whether the kernel advertises suspend and
hibernation modes but never enters either state.

The bundle contains structured JSON for DMI identity, Fedora/package NEVRAs,
kernel/module signing, iptsd instances, input classes, SAM/IIO, DRM connector,
mode, framebuffer, and available atomic-state inventory, libcamera,
Wi-Fi/Bluetooth state without identifiers, read-only fwupd device inventory,
audio nodes without media activity, battery/power/S0ix snapshots, and core
service/binary revisions. Button candidates, the MMC reader/media topology, and
the current/available platform performance profiles are also recorded without
pressing a button, reading media, or changing a profile. `manifest.json` binds
every artifact by byte count and SHA-256. Validate the bundle before copying it
into a governed evidence path.

Run the parser/admission regression without hardware:

```bash
install-helpers/collect-surface-acceptance.py --self-test
```
