# Media Node — Local Airsonic Access

**Status:** implementation target, operator-locked 2026-08-01.

This design supersedes the historical `Lighthouse_Media` hosting model. A
Media node is an enrolled mesh node on the local network that can reach one or
more operator-configured Airsonic-compatible servers. It does not need to host
Navidrome/Airsonic and it is not required to proxy music traffic.

## Locked behavior

- Airsonic, Navidrome, and compatible Subsonic APIs are supported.
- Endpoints are operator-configured; unrestricted LAN scanning is not used.
- HTTP and HTTPS endpoints are accepted. Credentials are never logged or
  published in plaintext.
- Credentials live in the mesh secret store. All enrolled Workstations may use
  the advertised service through normal mesh authorization.
- A node may advertise multiple server records. Each record contains its
  endpoint, implementation, operator priority, health, measured latency, and
  secret reference.
- Health is refreshed every five minutes using authenticated API access and a
  library query.
- Workstations connect directly to the configured server endpoint. The Media
  node is discovery and health authority, not a mandatory proxy.
- The default server is the highest-priority healthy record; latency breaks
  ties. Workstations may override the choice per seat.
- Switching away from a failed selected server requires user approval.
- Cached library metadata remains browsable offline, and fully cached tracks
  remain playable. Live library operations require a healthy server.

## Migration

Existing `media-registry.json` documents remain readable during migration.
Legacy hosted-Navidrome rows are converted into server records only when they
contain a valid endpoint and credential reference. Plaintext `shared_account`
payloads are never re-published; existing local credentials may be consumed
until the operator migrates them into the mesh secret store.

## Acceptance

1. A Workstation can configure a reachable LAN Airsonic server and see its
   library without requiring a `Lighthouse_Media` role.
2. Multiple healthy servers resolve deterministically by priority and latency.
3. A failed selected server produces an approval prompt before switching.
4. Dell can browse cached metadata and play a fully cached track with the
   Airsonic server offline.
5. Farm tests cover schema migration, health/selection, secret references,
   Workstation selection, failover approval, and offline playback.
