# WL-FUNC-023 / identity dest — Surface collaboration receipt admitted (2026-08-24)

Follows overlay recovery
(`WL-FUNC-023-2026-08-24-lighthouse-overlay-recovered-r1.md`). Red
`AI-GENERATED-ALERT` via `/usr/libexec/mackesd/seat-update-warning` as root
on Surface (`172.20.146.79` hostname `SURFACE`), then five-second hold.
`WARN_RC=0` at 2026-08-24T16:42Z. No dest invented. No seed, GPG private
key, or WAN IP recorded. `production_admitted` unchanged.

## Dest

`/usr/libexec/mackesd/setup-etcd --client-only --anchors 10.42.0.1,10.42.0.2,10.42.0.3`
wrote `/etc/mackesd/etcd-endpoints` to the live quorum (same list as Seat 15).
`mackesd secret put collaboration/node-signing-seed` sealed 32 UTF-8 bytes
(stdin only). SHA-256 of the stored value is
`aadf991334c84e44160caa5f0611b20d425feab689f573f92a2e651192a12472`.

Producer `install-helpers/produce-collaboration-identity-receipt.py` signed
the node-scoped receipt with the governed release key (fingerprint
`06B1C27EA0E08A225155EB3314018AA1497DDC7C` against
`/etc/pki/rpm-gpg/RPM-GPG-KEY-magic-mesh`). Installed as root-owned `0400`:

- `/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json`
- `/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json.asc`

Non-secret receipt body:

```
{"kind":"mcnf-collaboration-identity-admission","public_key_hex":"1a1cc7d5204ff8915e2f79167fe4fc2ce63e3624cccdb9d43cd50ebe0b39e52a","release_signer":"06B1C27EA0E08A225155EB3314018AA1497DDC7C","schema_version":1,"seed_sha256":"aadf991334c84e44160caa5f0611b20d425feab689f573f92a2e651192a12472","source_revision":"7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac","target_node":"peer:SURFACE","target_user":"system:mackesd"}
```

`source_revision` is the **installed** binary (`7e3474eeb`), not current HEAD.
Control-host temp seed, Spaces private-key copy, and signing `GNUPGHOME`
were shredded after install.

## Result

`mcnf-collaboration-identity.service` active. Materializer wrote:

- `/var/lib/mackesd/collaboration-identity-admission.json` (`0400`)
- `/var/lib/mackesd/node-signing.key` (`0600`, 32 bytes)

`Requires=` was **not** weakened. Orphan Aug-23 `mackesd serve --group data`
and `--group integrations` PIDs were SIGKILL'd after `systemctl stop` left
them owning the group lock. Units then started:

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

Overlay file `/var/lib/mackesd/nebula/overlay-ip` is `10.42.0.7`. Overlay
ping to Seat 15 `10.42.0.5` and LH1 `10.42.0.1` succeeds.

## Leftover

Privileged Bus mutations stay disabled:
`systemd cloud arming credential is unavailable`.
`nebula_supervisor` warns `replicated Nebula bundle relay trust authority
does not match the local enrollment pin` and does not rewrite config;
the existing overlay still forwards. Dell still has no SSH (powered off).
