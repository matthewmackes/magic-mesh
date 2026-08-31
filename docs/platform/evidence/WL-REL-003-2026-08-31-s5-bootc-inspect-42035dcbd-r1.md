# WL-REL-003 S5 dest-cut bootc receipt inspect — r1

Date: 2026-08-31  
Classification: dest receipt inspect; **not** live tag re-resolve, derivatives,
publication, or live enroll  
`published: false`  
`production_admitted: false`

## Inspect

From dest-cut checkout `42035dcbd` / `1788153988`:

`produce-bootc-digest-receipt.py inspect` against private dest
`/root/mcnf-private/bootc-all-roles-digest-42035dcbd.json` with expected pin
`quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`,
architecture `amd64`, role `all-roles`. PASS. Android absent.

Stored receipt still names tag `quay.io/fedora/fedora-bootc:44` and resolved
digest `sha256:3a5e74e6…`. Inspect does not hit the live registry. Live
`:44` has moved (index prefix `e91da1af`); do not follow it and do not
rebuild third-party bytes.

## Leftover

S4 Browser/App VM derivatives still need a dest-cut Browser VM base
receipt; that producer requires a live-pullable manifest and the dest-cut
digest is `manifest unknown` on quay. S6 six-role plan waits on S4.
`github-required` on freeze SHA remains queued (no farm runner). Native
F44 `.131` remains down.
