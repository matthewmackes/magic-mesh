# WL-ARCH-008 / WL-ARCH-010 — Dell Display1 and RDP recovery, release 26

Date: 2026-08-09

## Failure boundary

The release 25 Dell reboot proof exposed a separate Browser VM autostart
failure. QEMU rejected `-display dbus` because Fedora packages that backend
separately as `qemu-ui-dbus`, but the Magic Mesh RPM did not require it. After
installing that backend, the retained domain exposed two additional incompatible
legacy seams: its virtio GL video requested GL without enabling it on the D-Bus
display, and its SPICE graphics head and `spicevmc` channel could not coexist
with the D-Bus GL display.

Release 26 hard-requires `qemu-ui-dbus`. New domains and the bounded migration
now use one graphics authority:

```xml
<graphics type='dbus' p2p='yes'>
  <listen type='none'/>
  <gl enable='yes'/>
</graphics>
```

The migration removes only incompatible SPICE graphics and `spicevmc`
channels. Guest RDP remains the independent recovery transport. It fingerprints
every disk before and after definition and refuses any disk-set change.

## Gates

The RPM requirements self-test and source-requirements gate passed locally:

```text
install-helpers/verify-rpm-payload.sh --self-test
install-helpers/verify-rpm-payload.sh requirements
```

Machine 9 (`172.20.0.90`), slot `browser-display1-r92`, compiled the complete
`mackesd` library test target and passed the focused generated-domain contract:

```text
workers::workload_vm::tests::definition_uses_display1_and_escapes_untrusted_fields
test result: ok. 1 passed; 0 failed; 4624 filtered out
```

The migration self-test also passed with a fixture containing both legacy
SPICE graphics and a `spicevmc` channel.

## Live Dell correction

The installed warning helper presented the mandatory red
`AI-GENERATED-ALERT` and five-second intervention window before package and
domain mutations. Fedora's `qemu-ui-dbus-10.2.2-1.fc44.x86_64` was installed
without removals. Failed intermediate definitions did not replace the live
domain. The final migration preserved both disk sources and left timestamped
XML backups under `/var/lib/mackesd/browser-vm-migrations/`.

Live `virsh dumpxml browser-vm` now contains exactly one D-Bus graphics device,
with `p2p=yes`, GL enabled, and `/dev/dri/renderD128`; it contains no SPICE
graphics or `spicevmc` channel. `virsh dominfo` reports the domain running,
persistent, and autostart-enabled with 4 vCPUs and 8 GiB RAM. The guest acquired
`192.168.122.58`, and a direct TCP probe passed on port 3389.

This initial checkpoint proved corrected packaging, domain admission,
disk-preserving migration, VM launch, DHCP, and RDP transport readiness. The
release deployment proof below subsequently adds a real rendered frame, input,
and reconnect on Dell. The strict Chromium menu-pointer challenge and five-seat
presentation proof remain open.

## Native release 26 deployment

After source commit `0b437f92`, the dedicated BigBoy Fedora 44 builder produced
`magic-mesh-12.1.6-26.x86_64.rpm` in slot
`dell-display1-release26-r93`. The artifact is 90,417,682 bytes (86.2 MiB) with
SHA-256:

```text
db7d577a8a7201f2020f29ca49a0a8e6f44b1b3ef10876c176143874b0096cf4
```

The complete payload gate passed. The RPM header hard-requires
`qemu-ui-dbus`; its native media requirements are `libavcodec.so.62`,
`libavformat.so.62`, `libavutil.so.60`, `libswresample.so.6`, and
`libswscale.so.9`, matching Fedora 44.

Dell's staged hash matched exactly. A visible warning and five-second window
preceded a successful `rpm -Uvh --test`; a fresh warning preceded the real
upgrade. The installed state is now `magic-mesh-12.1.6-26.x86_64` with
`qemu-ui-dbus-10.2.2-1.fc44.x86_64`. The grouped daemon target and Construct
shell are active; the shell reports `Result=success` and `NRestarts=0`.
Post-install replacement briefly reset the system bus while optional transient
jobs were submitted, but convergence checks found no Magic Mesh failed unit.
The unrelated `fwupd-refresh.service` remains failed.

The Browser VM remained running and autostart-enabled through the package
upgrade, retained `192.168.122.58`, and passed the TCP 3389 probe again. Its
live XML still has exactly the D-Bus GL graphics head and no SPICE seam. The
dedicated F44 builder was then halted and normal BigBoy Fedora 42 farm capacity
was restored.

## Production RDP frame and input proof

The real `mde-vdi-rdp` live test was farm-built on restored BigBoy from the
integrated source, then executed on Dell so the host-bound password never left
the seat. Decryption and password extraction occurred only in a root-only
`/run` directory with a cleanup trap. The generic production-path acceptance
passed in 88.61 seconds:

```text
CONNECTED tier=Full desktop=1024x768
FRAME OK 1024x768 rects=17 fnv1a64=0x8a176d065c1dea85 distinct_colors=43
INPUT ECHOED before=0xe5b14aedf830acc3 after=0xde81d67e8c28b705
RECONNECTED tier=Compressed desktop=1024x768
TIER FRAME OK 1024x768 rects=1 fnv1a64=0x3624e1ca8b51b089 distinct_colors=665
test result: ok. 1 passed; 0 failed; finished in 88.61s
```

This proves the assembled client rendered inbound guest pixels, sent four real
input events and observed a framebuffer response, then reconnected and rendered
again at the requested tier. The optional strict Chromium app-menu pointer
challenge was not requested, so it remains part of the open five-seat proof.
