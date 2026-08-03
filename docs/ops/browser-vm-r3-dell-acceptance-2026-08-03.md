# Browser VM r3 Dell acceptance — 2026-08-03

Status: current R1 Chromium Workspace milestone evidence. This record proves
the immutable Dell Browser VM desktop path described below; it does not close
WL-ARCH-008 or the fleet production gates.

## Outcome

Dell `172.20.146.225` (`DELL-LAPTOP`) is online with the immutable Fedora 44
Browser VM r3. The guest renders the real Chromium/Sway desktop over RDP,
accepts keyboard input, and reconnects with the compressed tier. The domain
survived an unplanned Dell reboot through libvirt autostart and was left
running with RDP reachable at `192.168.122.58:3389`.

Every Dell mutation in this cutover ran only after the installed
`/usr/libexec/mackesd/seat-update-warning` published
`AI-GENERATED-ALERT` and completed its five-second wait. During one overlay
broker outage the helper persisted the local event for retry and still
enforced the wait; no mutation bypassed the helper.

## Immutable build and provenance

- Source fix: `9ccb8e903f204c3c0b292815de219d095e40d06a`
  (`fix browser VM desktop session startup`).
- Profile pin: `639f26eb2db1155d8aa09f6b8cbe18d8021b3ab5`.
- Builder: Fedora 44 farm worker `172.20.0.131` on BigBoy, from a clean detached
  worktree at the profile pin.
- Container image ID:
  `4aed9590cdd5300743b6b2f27cee8aa579ae0ab1f774764921d2320d64ede5aa`.
- qcow2 SHA-256:
  `99110644809cea6a4d0d8031854d591051f62ee7c147567497f7c2d71faa0ec6`.
- qcow2 physical size: `1,852,965,888` bytes; virtual size:
  `68,719,476,736` bytes (64 GiB). `qemu-img check` reported no errors.
- NoCloud seed SHA-256:
  `d5e5320e382f2d9464bccfcb7ec0caeb84b1019d1ef929cfa2a80e3a7aa7797a`;
  session `session:ecac4583-2127-45a8-8d58-b7c3ad3ce9a7`.

The source contract passed on farm `.90`. The container verifier passed the
Chromium, thin-lighthouse, Sway, RDP/SPICE, PipeWire/ALSA, VA-API, guest-agent,
device-group, `xrdp-selinux`, and host-Browser-absence checks. A separate
read-only inspection of the finished disk confirmed:

- runtime inputs use `/etc/mcnf-browser-vm`, outside protected
  `/etc/mackesd`;
- sesman selects `DefaultWindowManager=startwm.sh`;
- the image-owned entrypoint is `/usr/libexec/xrdp/startwm.sh` with SELinux
  `bin_t`;
- the active xrdp policy module is present at
  `/etc/selinux/targeted/active/modules/400/xrdp`;
- the nested session fixes `WLR_BACKENDS=x11` and `WLR_RENDERER=pixman`.

## Dell deployment identity

- Domain: `browser-vm`.
- UUID: `a1100a2f-5b65-4064-ac9f-925e1affa1fb`.
- Immutable base:
  `/var/lib/libvirt/images/browser-vm-chromium.qcow2`, `root:qemu`, mode
  `0440`.
- Writable overlay:
  `/var/lib/libvirt/images/browser-vm-r3-overlay.qcow2`, `qemu:qemu`, mode
  `0640`, with one direct backing edge to the immutable base.
- Seed: `/var/lib/libvirt/images/browser-vm-r3-seed/seed.iso`.
- The schema-v2 deployment receipt passed the checked-in validator. Its private
  durable copy is
  `/var/lib/libvirt/images/browser-vm-r3-evidence-20260803/deployment-receipt.json`,
  mode `0600`, SHA-256
  `e139090d27b92fb2db0f869baa34e9f1a620c6820b5caadbb0c519588e3d2234`.
- The hotfixed r2 base, writable overlay, seed, adjusted domain XML, and hashes
  remain under
  `/var/lib/libvirt/images/browser-vm-r2-hotfix-rollback-before-r3-9ccb8e90`
  and the existing r2 paths. This is cutover provenance, not a replacement for
  corrected-forward fleet recovery.

The fixed `mcnf-browser` password was rotated after r3 boot. Its matching
credential is host-bound ciphertext at
`/etc/credstore.encrypted/browser-vm-rdp`; the live proof decrypted it only
inside a root-only `/run` directory and verified that directory was removed.

## Strict RDP proof

The farm-built `mde-vdi-rdp` live test required at least eight distinct colors,
so xrdp's two-color pre-session surface could not count as the desktop. On r3
it produced:

```text
FRAME OK 1024x768 rects=11 fnv1a64=0xd64f412be8133421 distinct_colors=73
settled baseline fnv1a64=0x20b7f2afb5dfe510
INPUT ECHOED before=0x20b7f2afb5dfe510 after=0x2445d253cff1e8bb
RECONNECTED tier=Compressed compression=Some(Rdp6)
TIER FRAME OK 1024x768 rects=1 fnv1a64=0x1adc82df209d8295 distinct_colors=6739
test result: ok. 1 passed; 0 failed; finished in 39.75s
STRICT_PROOF_CREDENTIALS_CLEAN
```

## Guest evidence

The root-only evidence bundle is
`/var/lib/libvirt/images/browser-vm-r3-evidence-20260803` on Dell. It contains
the domain XML, backing-chain metadata, deployed hashes, admitted input records,
runtime/media records, VA-API and PipeWire probes, xrdp logs, and a complete
`SHA256SUMS` manifest.

- Runtime evidence SHA-256:
  `23f10c21d77185e8d9c6b222cf2131ff5f687a761c13fdb5b74436cd016cbb7f`.
  The validator confirmed source/image identity, `gpu_status=passed`,
  `audio_status=wired`, and one playback plus one capture endpoint.
- Media evidence SHA-256:
  `8b329c99070155bd6e45407354095e69b61c84f74a8112502cc04b65cb4c848d`.
  Chromium's fixed local MKV reached video and audio ready-state 4, decoded
  four frames, and dropped zero frames.
- Relevant SELinux-denial evidence is empty, SHA-256
  `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.

These validators deliberately do not convert endpoint wiring or guest-local
decode into an audible-host claim. R1 still requires direct Chromium-to-Dell
audibility/capture and reconnect recovery evidence, separate pointer-focus
proof, Workloads automatic admission/launch proof, and the 15-minute five-tab
performance run. Fleet and six-node production gates also remain open.
