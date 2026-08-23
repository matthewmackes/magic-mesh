# WL-TEST-002 S2 unpublished workstation install — Eagle + T480 r2

Date: 2026-08-23  
Classification: live seat package install; **not** freeze, publication,
six-role qualification, or enroll-bar closure  
`published: false`  
`production_admitted: false`

Operator 2026-08-23: bring Eagle and T480 up to par with the three proof
seats already on unpublished signed `13.0.0-35`.

## Alert

`seat-update-warning.sh` ran on each target before mutation. Each published
`AI-GENERATED-ALERT` (broker persisted `--no-broker`) and waited 5s.

| Seat | Hold completed |
|---|---|
| Eagle | `2026-08-23T07:34:54-04:00` |
| T480 | `2026-08-23T07:35:40-04:00` |

## Privilege

Neither seat has passwordless `sudo -n` or working `root@` on the mesh
key. Eagle `mm@172.20.146.88` pubkey works; T480 LAN pubkey is refused.
T480 is reachable as `mm@10.42.0.8` from Dell overlay, and as `mm` on
LAN with the existing promotion password sidecar
(`/root/.mcnf-xapi-cred`, same contract as
`automation/promotion/mcnf-promotion-cycle.sh`). Install used pubkey
(or password SSH) plus `sudo -S` with that sidecar. The secret was not
logged.

## Targets and results

Admitted dest workstation RPM `magic-mesh-13.0.0-35.x86_64`
(`d72fa0cfdfa808da60f6addb4763e1401e0b618871fdf2b33fe9f64db8905fa0`)
signed by `06B1C27EA0E08A225155EB3314018AA1497DDC7C`. Each seat imported
that public key and `rpm --checksig -v` reported the governed fingerprint
OK before `dnf install`. `dnf` warned that OpenPGP checks were skipped
for `@commandline`; pre-checksig is the gate. Source identity in the
installed `mackesd`: `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`.

Both seats also logged a missing local mirror
`file:///mnt/mesh-storage/mirrors/magic-mesh/repodata/repomd.xml`. The
transaction used the staged dest RPM only.

| Seat | Address | Fedora | Before | After | mackesd |
|---|---|---|---|---|---|
| Eagle | `172.20.146.88` `mm` | 44 | `12.1.6-35` | `13.0.0-35` | `13.0.0` · `7e3474eeb` · inactive |
| T480 | `172.20.146.68` `mm` | 44 | `12.1.6-35` | `13.0.0-35` | same · inactive |

`systemctl is-failed mackesd` is not failed. Units are inactive. That is
honest: leftover FUNC-023 mint + enroll/offboard+reenroll is still due.
No reboot ran. No lighthouse/server RPM was installed.
`production_admitted` stays false.

Independent reread 2026-08-23 confirmed all five workstations now share
the same NEVRA and `mackesd` identity:

| Seat | Address | NEVRA |
|---|---|---|
| Seat 15 | `172.20.0.15` | `magic-mesh-13.0.0-35.x86_64` |
| Dell | `172.20.146.225` | `magic-mesh-13.0.0-35.x86_64` |
| Surface | `172.20.146.79` | `magic-mesh-13.0.0-35.x86_64` |
| Eagle | `172.20.146.88` | `magic-mesh-13.0.0-35.x86_64` |
| T480 | `172.20.146.68` | `magic-mesh-13.0.0-35.x86_64` |
