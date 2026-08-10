# Surface Pro 5/6 acceptance evidence collection

This is a bounded operations procedure, not a second worklist. The only active
tracker remains `docs/platform/WORKLIST.md`.

`install-helpers/collect-surface-acceptance.py` takes a read-only inventory of a
Surface candidate. It does not start or stop services, trigger udev, install
packages, query for firmware updates, enroll keys, change radio state, suspend,
reboot, play or record audio, or open a camera stream. It inventories libcamera
and separately reads the newest result from the exact local
`state/hardware/surface/<local-node>/camera-proof` Bus lane. That result must be
a shared schema-1 `passed` result for the exact Pro 5/6 model and generation,
no more than 90 seconds old (with at most five seconds future skew). The
collector opens `/run/mde-bus/index.sqlite` read-only and never requests a new
proof. Network connection names, addresses, MAC addresses,
and common secret forms are excluded or redacted.

The interactive Surface card has a separate functional camera proof. The
collector never triggers that action; sealing only reads its closed result.
The action requires an exact-body capability,
the fixed operator phrase, and exact Pro 5/6 generation, asks the fixed provider
for one frame, records only a closed outcome, and immediately discards the frame.
It accepts no output path or device selector and never publishes image bytes.
Fingerprint collection remains read-only, non-claiming fprintd device enumeration;
neither path proves biometric enrollment or authentication.

Run it locally on the named seat. Use output paths on a root-only local
filesystem; the tool refuses to overwrite a path and writes files mode `0600`.
Use the two-phase flow so slow inventory cannot consume the camera result's
90-second lifetime:

```bash
sudo install-helpers/collect-surface-acceptance.py prepare \
  --seat Surface --expected-generation 6 \
  --out /var/tmp/surface-pro6-prepared

# In the local Surface card, type PROVE CAMERA and run the authorized action.
# Do not preview, save, or copy a frame: the provider obtains exactly one frame,
# decides the closed result, and discards that frame immediately.

sudo install-helpers/collect-surface-acceptance.py seal \
  --prepared /var/tmp/surface-pro6-prepared \
  --out /var/tmp/surface-pro6-acceptance

sudo install-helpers/collect-surface-acceptance.py validate \
  /var/tmp/surface-pro6-acceptance
```

`prepare` binds its start and completion timestamps, the exact local node,
seat, generation, collector SHA-256, and
every inventory artifact's size, SHA-256, and status. It contains no
`camera-proof.json`, final `manifest.json`, or acceptance claim. `seal` accepts
prepared inventory for at most five minutes after preparation completes and
rejects preparation lasting more than 30 minutes. It rechecks the exact file set,
regular-file type, bounds, hashes, statuses, identity, node, and collector
revision, including a fresh comparison with local DMI, before copying through
no-follow file descriptors. It then reads a
fresh successful local camera result and atomically publishes a new final
bundle. Missing, modified, stale, future-dated, foreign, failed, or identifying
input fails closed and leaves no final bundle. If five minutes elapse, discard
the prepared directory and run `prepare` again; never edit or refresh its
timestamp.

The final schema-1 `captured_at_utc` is the seal time. Its inventory artifacts
were collected during the preceding bounded 30-minute preparation, which must
have completed no more than five minutes before sealing.
This limitation is intentional and recorded here; changing an artifact or
collector between phases requires recollection. The legacy `collect` command
remains a fail-closed one-shot convenience, but it can only use a proof that is
still fresh after all serial probes. Prefer `prepare` and `seal`.

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
`physical_acceptance_claimed: false` and lists the recorder's canonical twelve
hands-on checks: touch; pen; Type Cover; buttons; microSD; rotation; camera
privacy; audio/microphone; suspend/S0ix; reboot/upgrade; DRM modes; and
fingerprint. The collector records whether the kernel advertises suspend and
hibernation modes but never enters either state. It does not claim fingerprint
authentication, media contents, audio quality, or any other physical result.

Run the Surface card's explicitly armed `PROVE CAMERA` action between `prepare`
and `seal`. `camera-proof.json` retains only the exact node, normalized model,
generation, completion time, closed `passed` outcome, and SHA-256 of the shared
result. It explicitly records that frame bytes, device identifiers, and the
request identifier were not retained. An absent, stale, failed, foreign, or
mismatched result prevents sealing; it never becomes a manual camera-preview
claim. Existing bundles carrying the former grouped nine-item checklist are a
different schema-1 contract and must be recollected before recording under the
canonical twelve-check workflow; do not hand-edit their manifests.

The bundle contains structured JSON for DMI identity, Fedora/package NEVRAs,
kernel/module signing, iptsd instances, input classes, SAM/IIO, DRM connector,
mode, framebuffer, and available atomic-state inventory, libcamera,
the privacy-safe hash-bound camera functional proof,
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
