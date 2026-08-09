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

This checkpoint proves corrected packaging, domain admission, disk-preserving
migration, VM launch, DHCP, and RDP transport readiness. It does not claim a
captured Chromium frame or end-to-end pointer injection; those remain part of
the Browser/Workloads epic's five-seat presentation proof.
