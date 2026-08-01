# VDI live-target discovery

`discover-vdi-live-targets.sh` inventories only explicitly named proof seats.
It is a read-only preflight for `verify-vdi-live-proof.py`, not framebuffer or
input evidence.

Run it from an operator workstation with an already-configured SSH agent and
known-host entry for each approved seat:

```bash
install-helpers/discover-vdi-live-targets.sh \
  --seat proof-15=172.20.0.15 \
  --seat bench-dell=172.20.146.225
```

The JSON records a seat as `reachable` only when SSH succeeds.  Its endpoints
are merely listeners on VNC `5900`–`5909`, SPICE `5930`, or RDP `3389`; an empty
endpoint list and an `unavailable` seat are both honest discovery outcomes.
Pass a candidate endpoint explicitly to the framebuffer verifier only after
operator review.

The helper accepts no credential, ticket, password, or key-file option. It uses
non-interactive SSH with strict known-host verification and discards SSH
diagnostics. It records no protocol banners or raw logs, and prints no secret
material. Its only supported deterministic check is:

```bash
install-helpers/discover-vdi-live-targets.sh --self-test
```
