# WL-FUNC-018 signed App VM catalog S1 — 2026-08-08

The App VM catalog now has a bounded Ed25519 envelope covering catalog identity,
monotonic revision, provider/repository origin, validity window, application
identity, version, icon identity, permissions, actions, readiness, and explicit
search inputs. Admission requires the exact locally trusted signer, a valid
signature, current freshness, canonical ordering, and a maximum 24-hour TTL.

Untrusted JSON is capped at 512 KiB before parsing. Unknown or duplicate fields,
duplicate/unordered app IDs, unsupported permissions, URLs, paths, secret-like
content, malformed signatures, future/stale catalogs, and schema skew fail
closed. Search validates its query and ranks deterministically by match class,
signed weight, then stable app ID; it does not synthesize launch metadata.

## Verification

- `.196`, slot `func018-app-catalog-s1-r1`: focused catalog tests passed 12/12.
- The complete `mackes-mesh-types` suite passed 480/480; doc tests passed.
- Scoped `git diff --check` passed.
- No operational tests were removed.

## Remaining acceptance gap

The production importer still must load the root-owned trust key, enforce
monotonic/last-good persistence, and project only admitted rows. App VM image,
lifecycle, Front Door UX, package/security gates, and live-seat proof remain, so
FUNC-018 stays `Remaining`.
