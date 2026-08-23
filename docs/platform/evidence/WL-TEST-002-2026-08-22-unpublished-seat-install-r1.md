# WL-TEST-002 S2 unpublished workstation install — r1

Date: 2026-08-22  
Classification: live seat package install; **not** freeze, publication,
six-role qualification, or enroll-bar closure  
`published: false`  
`production_admitted: false`

Operator 2026-08-22: install the unpublished signed 13.0.0 dest fresh on
all proof seats.

## Alert

`seat-update-warning.sh` ran on each seat before mutation. Each published
`AI-GENERATED-ALERT` (broker persisted `--no-broker`) and waited 5s.
Control-host `mde-bus` is absent; the toast ran on the seats.

Hold completed 2026-08-22T22:30:23-04:00.

## Targets and results

Admitted dest workstation RPM `magic-mesh-13.0.0-35.x86_64` signed by
`06B1C27EA0E08A225155EB3314018AA1497DDC7C`. Each seat imported that
public key and `rpm --checksig -v` reported the governed fingerprint OK
before `dnf install`. Source identity in the installed `mackesd`:
`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`.

| Seat | Address | Fedora | Before | After | mackesd |
|---|---|---|---|---|---|
| Seat 15 | `172.20.0.15` `mm` | 44 | `12.1.6-35` | `13.0.0-35` | `13.0.0` · `7e3474eeb` · inactive |
| Dell | `172.20.146.225` `mm` | 44 | `12.1.6-35` | `13.0.0-35` | same · inactive |
| Surface | `172.20.146.79` `root` | 44 | `12.1.6-35` | `13.0.0-35` | same · inactive |

Surface `mm` is wheel but `sudo -n` requires a password; install used
`root@` with the mesh key. Surface `dnf` warned that retiring
`rtpengine-mde.service` / `kamailio-mde.service` files were already
absent (FUNC-033 stack deleted). Transaction still completed.

`systemctl is-failed mackesd` is not failed. Units are inactive. That is
honest: leftover FUNC-023 mint + enroll/offboard+reenroll is still due.
No reboot ran. No lighthouse/server RPM was installed on these
workstations. `production_admitted` stays false.

Independent reread 2026-08-22 confirmed the three NEVRAs and the same
`mackesd` identity string.
