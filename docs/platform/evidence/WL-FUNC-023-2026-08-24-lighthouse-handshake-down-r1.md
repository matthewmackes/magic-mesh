# WL-FUNC-023 leftover — lighthouse overlay handshake down (2026-08-24)

Read-only. No dest invented. No `production_admitted` change. Surface was
not offboarded.

## Observation

Control-host TCP `104.236.118.177:4242` is closed and ICMP to that
underlay fails. Seat 15 overlay `10.42.0.5` cannot ping lighthouse
`10.42.0.1` or Surface `10.42.0.7`. Surface `nebula.service` is active
with `nebula1` `10.42.0.7/17`; journal shows handshake send then timeout
to `10.42.0.1` at underlay `104.236.118.177:4242` and to
`192.168.100.1` at `100.64.22.11:4242`.

Surface collaboration-identity dest remains refused: mesh SecretStore
needs etcd over overlay. Writing `etcd-endpoints` while the lighthouse
handshake times out would not admit the dest.

## Result

FUNC-023 live-seat leftover stays overlay/lighthouse reachability, not a
missing receipt producer. Dell still has no SSH route.
