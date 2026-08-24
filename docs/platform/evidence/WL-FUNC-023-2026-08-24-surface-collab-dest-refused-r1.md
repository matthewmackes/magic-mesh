# WL-FUNC-023 / identity dest — Surface collaboration receipt refused (2026-08-24)

Operator authorized creating required keys and completing remaining
worklist dests. Red `AI-GENERATED-ALERT` via
`/usr/libexec/mackesd/seat-update-warning` as root on Surface
(`172.20.146.79` hostname `SURFACE`), then five-second hold. `WARN_RC=0`
at 2026-08-24T15:47Z. `production_admitted` unchanged. Dell
(`172.20.146.225`) had no SSH route and was not mutated.

## Attempt

A node-scoped receipt was produced for `peer:SURFACE`, revision
`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`, signer
`06B1C27EA0E08A225155EB3314018AA1497DDC7C`,
`public_key_hex=43c743de27a5fcd1ee533b7b003dfbc94eb57ba19ab27d957614fa40e72b1edb`,
`seed_sha256=5779a06e7e7394c71012a932e02809690fc96ae3fd015efdc47e5e5f58e60960`.
The 32-byte seed, Spaces private-key copy, and temp `GNUPGHOME` were
shredded after the store refused. No receipt was installed.

`mackesd secret put collaboration/node-signing-seed` failed closed:

```
mcnf-secret: no etcd endpoint — set MCNF_ETCD or write /etc/mackesd/etcd-endpoints
```

Surface has `/opt/mcnf/automation/secrets/mcnf-secret.sh`, so
`SecretStore::resolve` selects the mesh store, not LocalAead. Overlay
interface `nebula1` is `10.42.0.7/17` and `nebula.service` is active, but
`/var/lib/mackesd/nebula/overlay-ip` is absent and overlay ping to
`10.42.0.1` and Seat 15 `10.42.0.5` loses 100%. Materializer was not
weakened; `--local` was not used.

## Result

Surface `mcnf-collaboration-identity.service` stays failed. Integrations
stay inactive. Seat 15 dest from earlier this day is unchanged. Next act
is overlay reachability (FUNC-023 leftover), not an invented etcd file.
