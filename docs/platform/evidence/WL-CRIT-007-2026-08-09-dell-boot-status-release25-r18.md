# WL-CRIT-007 — Dell truthful pre-Construct boot status, release 25

Date: 2026-08-09

## Reported failure and before-state

Dell (`172.20.146.225`, overlay `10.42.0.4`) was reachable again after an
operator-reported hang. Its first measured boot on
`magic-mesh-12.1.6-21.x86_64` spent more than a minute at a blinking cursor
before the Construct splash. `systemd-analyze time` reported:

```text
firmware 6.085s + loader 8.793s + kernel 1.020s + initrd 3.864s
userspace 1min 43.457s; total 2min 3.220s
multi-user.target reached after 1min 36.122s
```

The live critical chain proved that `mde-shell-egui.service` explicitly waited
for `mackesd.target`, which did not converge until about 1min 34s. The two
retired host-Browser policy units still installed from an older package added
44.344s (`mde-web-preview-selinux.service`) and 33.648s
(`mde-web-cef-selinux.service`). The kernel command line included `rhgb quiet`,
so Plymouth covered unit/start-job progress during this interval.

The installed release also had an unrelated stale Browser VM launch failure
(`Display 'dbus' is not available`), failed peer-recovery/Syncthing state, and
only one of four managed peers connected. Those observations are not claimed
as causes of the pre-splash blank interval and are retained as separate live
recovery gates.

## Corrected-forward package contract

Release 25 removes only the shell's ordering dependency on `mackesd.target`;
the target remains a `Wants=` dependency, and typed panels retain their honest
loading/empty states while daemon groups converge. Its post-install transaction
removes `rhgb`, adds `systemd.show_status=1 rd.systemd.show_status=1` to every
installed kernel, and disables/removes the two named retired SELinux units and
helpers. Kernel chatter remains quiet while systemd reports real boot work,
then the DRM shell replaces tty status with the Construct splash as soon as its
own prerequisites are ready.

Source commit: `339678bb3f766d810646bf0e3ef9e4ebbc892fbc`.

Focused contract gate on BigBoy (`172.20.0.130`, slot `boot-status-r90`):

```text
install-helpers/test-boot-status-upgrade.sh
```

Result: PASS. A subsequent `systemd-analyze verify` parsed the unit and reported
only that `/usr/bin/mackesd` and `/usr/bin/mde-shell-egui` are not installed on
that generic farm VM; it did not report a unit ordering or syntax error.

## Native Fedora 44 build and Dell deployment

The dedicated native Fedora 44 builder (`172.20.0.131`, BigBoy) was started
after confirming the normal `.130` VM had no active build. Its first start was
network-dark; a forced guest reboot recovered it. A direct XCP RFB capture
proved Fedora was at tty1 with `172.20.0.131` before SSH became reachable. The
builder was revalidated at Rust 1.94.0, 10 CPUs, 23 GiB RAM, and mpv 2.5.0.

The clean detached `339678bb` workspace produced:

```text
magic-mesh-12.1.6-25.x86_64.rpm
size: 90,417,907 bytes (86.2 MiB; 90 MiB gate passed)
SHA-256: ad89fbc660886e767c364bbcc10b0ff81d1c4611914faf1ed8351f648de10543
```

The complete RPM payload gate passed. Its native requirements include
`libavcodec.so.62`, `libavformat.so.62`, `libavutil.so.60`,
`libswresample.so.6`, and `libswscale.so.9`, matching Fedora 44 rather than the
incompatible Fedora 42 epoch.

The exact staged hash matched on Dell. The installed warning helper published
the red `AI-GENERATED-ALERT` and held its required five-second window before a
separate `rpm -Uvh --test`, which passed. The real transaction then installed
`magic-mesh-12.1.6-25.x86_64`. A second visible warning and five-second wait
preceded the reboot.

Post-install inspection before reboot proved:

- `mde-shell-egui.service` has `Wants=mackesd.target` and no
  `After=mackesd.target`;
- all three installed kernel entries lack `rhgb` and include
  `systemd.show_status=1 rd.systemd.show_status=1`;
- both retired host-Browser SELinux units and files are absent; and
- Construct, `mackesd.target`, and Nebula were active.

## Live reboot result

Dell left ping at 16:57:25 and returned at 16:58:06 (41 seconds); SSH returned
at 16:58:15. The boot ID changed from
`0cb8eff4-2ffe-4d4f-b8ec-a65440243615` to
`575294f6-c914-4e45-bfb4-e1dd574b2334`. The live command line is:

```text
BOOT_IMAGE=(hd0,gpt1)/vmlinuz-7.1.4-204.fc44.x86_64 root=/dev/mapper/fedora-root ro rd.lvm.lv=fedora/root quiet systemd.show_status=1 rd.systemd.show_status=1
```

There is no `rhgb`; systemd status is explicitly enabled for the initrd and
host manager. The post-boot timing is:

```text
firmware 6.216s + loader 8.794s + kernel 1.071s + initrd 5.171s
userspace 57.818s; total 1min 19.072s
multi-user.target reached after 52.724s in userspace
mde-shell-egui active at 28.434277s
mackesd.target active at 58.966270s
```

The shell therefore painted without waiting roughly 30.5 seconds for the full
mesh target and became active about 66 seconds earlier than the old 1min34s
dependency chain. Total boot improved by 44.148 seconds (2min3.220s to
1min19.072s). Its live journal records shell start at 25.664s, active at
28.434s, seat milestone at 33.802s, surfaces/mesh snapshot at 44.02s, and splash
handoff at 44.280s. `NRestarts=0`; the grouped daemon target converged with no
remaining jobs. Installed binary hashes are:

```text
829c01a02f29108a814ad2a4bd6e756d6dcfc55e998c5889b6af7648f539c75f  /usr/bin/mde-shell-egui
613e9c51a919e1929b026166d3f7531d08e67d740d0f20019eb9dcec8660dc57  /usr/bin/mackesd
```

Syncthing and peer recovery are active after reboot, correcting their release
21 failed state. No camera or pre-DRM framebuffer capture was available on the
physical Dell, so visible text is proved by the exact live kernel status
contract rather than a photographed frame.

The retained `browser-vm` still fails autostart independently with
`Display 'dbus' is not available`. Release 25 does not claim that separate
Workloads/Browser VM correction; the domain remains shut off and no RDP-ready
claim is made from this boot evidence.
