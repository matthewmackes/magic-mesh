# WL-FUNC-023 / identity dest — Dell collaboration receipt admitted (2026-08-24)

Operator reported Dell up. Red `AI-GENERATED-ALERT` via
`/usr/libexec/mackesd/seat-update-warning` as `mm` on Dell
(`172.20.146.225` hostname `DELL-LAPTOP`), then five-second hold.
`WARN_RC=0` at 2026-08-24T18:05Z. No dest invented. No seed, GPG private
key, or WAN IP recorded. `production_admitted` unchanged. DHCP mapping
`DELL-LAPTOP` / `be:61:cf:5b:ea:4d` / `172.20.146.225` matches live ARP.

## Dest

`/usr/libexec/mackesd/setup-etcd --client-only --anchors 10.42.0.1,10.42.0.2,10.42.0.3`
wrote `/etc/mackesd/etcd-endpoints` to the live quorum. Overlay iface is
`10.42.0.4/17`; ping to LH1, Seat 15 (`10.42.0.5`), and Surface
(`10.42.0.7`) succeeds. Installed RPM `magic-mesh-13.0.0-35`
(`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`).

`mackesd secret put collaboration/node-signing-seed` sealed 32 UTF-8 bytes
via `sudo -n` (stdin only). SHA-256 of the stored value is
`70099f45e93fc0018551409838a38b9045d664527557e02996e98a6cd3a66b65`.

Producer `install-helpers/produce-collaboration-identity-receipt.py` signed
the node-scoped receipt with the governed release key (fingerprint
`06B1C27EA0E08A225155EB3314018AA1497DDC7C`). Installed as root-owned `0400`
under `/etc/mcnf/release-inputs/collaboration/`.

Non-secret receipt body:

```
{"kind":"mcnf-collaboration-identity-admission","public_key_hex":"df06969607977e209867a0f3720ff4b8aa2afd5d23e9d0226e8ff0507ea8a998","release_signer":"06B1C27EA0E08A225155EB3314018AA1497DDC7C","schema_version":1,"seed_sha256":"70099f45e93fc0018551409838a38b9045d664527557e02996e98a6cd3a66b65","source_revision":"7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac","target_node":"peer:DELL-LAPTOP","target_user":"system:mackesd"}
```

Control-host temp seed, Spaces private-key copy, and signing `GNUPGHOME`
were shredded after install.

## Result

`mcnf-collaboration-identity.service` active. Materializer wrote
`/var/lib/mackesd/collaboration-identity-admission.json` (`0400`) and
`/var/lib/mackesd/node-signing.key` (`0600`, 32 bytes). Aug-23 orphan
`mackesd serve --group data` / `--group integrations` PIDs were SIGKILL'd
so the group lock could be claimed. `Requires=` was not weakened.

| Unit | State |
|---|---|
| `mcnf-collaboration-identity` | active (exited) |
| `mackesd-control` | active |
| `mackesd-integrations` | active |
| `mackesd-actions` | active |
| `mackesd-data` | active |
| `mackesd-compute` | active |
| `mackesd-observation` | active |
| `nebula` | active |

`/var/lib/mackesd/nebula/overlay-ip` is `10.42.0.4`.

## Leftover

Privileged Bus mutations stay disabled:
`systemd cloud arming credential is unavailable`. FUNC-023 leftover (3)
offboard+reenroll was not repeated on Dell this unit.
