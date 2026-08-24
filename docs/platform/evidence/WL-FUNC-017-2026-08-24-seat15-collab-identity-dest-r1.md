# WL-FUNC-017 — admit Seat 15 collaboration-identity dest (2026-08-24)

Operator chose dest admission (option 1), not a systemd `Requires=` bypass
and not a split of the vehicle worker. Red `AI-GENERATED-ALERT` via
`/usr/libexec/mackesd/seat-update-warning` on Seat 15 (broker persisted
`--no-broker`), then five-second hold. `WARN_RC=0` at 2026-08-24T15:33Z.
No dest invented. No seed, GPG private key, MG90 password, or WAN IP
recorded. `production_admitted` unchanged. MG90 was **not** rebooted.

## Target

| Field | Value |
|---|---|
| Seat | `172.20.0.15` `Basement-Test-Workstation` |
| Installed RPM | `magic-mesh-13.0.0-35` (`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`) |
| Gateway | `172.20.0.25:2222` ESN `ND84720078011035` MGOS `4.3.0.1` |
| Node scope | `peer:Basement-Test-Workstation` |
| Release signer | `06B1C27EA0E08A225155EB3314018AA1497DDC7C` |

## Dest

`mackesd secret put collaboration/node-signing-seed` sealed 32 UTF-8 bytes
on the seat (stdin only; not argv). SHA-256 of the stored value is
`4b3bc82ec613952f9e026beffb08721e0027c4775693db35f2fb3c07f8d44f3a`.

Producer `install-helpers/produce-collaboration-identity-receipt.py` signed
the node-scoped receipt with the governed release key (fingerprint match
against `/etc/pki/rpm-gpg/RPM-GPG-KEY-magic-mesh`). Installed as root-owned
`0400`:

- `/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json` (432 bytes)
- `/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json.asc` (228 bytes)

Non-secret receipt body:

```
{"kind":"mcnf-collaboration-identity-admission","public_key_hex":"451ccebb7184c47b691f701785d9a32ec575431f004dd108c61cadab1e7c9aeb","release_signer":"06B1C27EA0E08A225155EB3314018AA1497DDC7C","schema_version":1,"seed_sha256":"4b3bc82ec613952f9e026beffb08721e0027c4775693db35f2fb3c07f8d44f3a","source_revision":"7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac","target_node":"peer:Basement-Test-Workstation","target_user":"system:mackesd"}
```

`source_revision` is the **installed** binary (`7e3474eeb`), not current HEAD.
Control-host temp seed, Spaces private-key copy, and signing `GNUPGHOME` were
shredded after install.

## Result

`mcnf-collaboration-identity.service` ExecStart exited 0 (`RemainAfterExit`
active). Materializer wrote:

- `/var/lib/mackesd/collaboration-identity-admission.json` (`0400`, 372 bytes)
- `/var/lib/mackesd/node-signing.key` (`0600`, 32 bytes)

`Requires=` on integrations / actions / control / data was **not** weakened.
Those units started after the dest:

| Unit | State |
|---|---|
| `mcnf-collaboration-identity` | active (exited) |
| `mackesd-integrations` | active |
| `mackesd-actions` | active |
| `mackesd-control` | active |
| `mackesd-data` | active |
| `mackesd-compute` | active |
| `mackesd-observation` | active |

Integrations journal: `starting worker` `vehicle`. Seat 15
`mg90-access ssh-probe` returns ESN `ND84720078011035`. Bus
`state/vehicle/Basement-Test-Workstation/ND84720078011035` is publishing:
`online true`, firmware `4.3.0.1`, WAN `Cellular A`.

## Leftover

Privileged Bus mutations stay disabled on this seat:
`systemd cloud arming credential is unavailable`. Typed vehicle mutations
(`set-mcu` / `set-gps` / `reboot`) therefore still need the arming dest;
this identity dest only unblocked the worker process.

Installed `13.0.0-35` still lacks the newer inspect/set-mcu/set-gps verbs.
Dell and Surface still have no node-scoped collaboration receipt (out of
scope unless the operator expands).
