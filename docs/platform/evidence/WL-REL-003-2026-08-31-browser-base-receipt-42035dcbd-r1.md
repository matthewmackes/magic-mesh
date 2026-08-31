# WL-REL-003 Browser VM dest-cut base receipt — r1

Date: 2026-08-31  
Classification: dest recovery of already-selected bootc digest; **not** a
new dest, live `:44` follow, derivative cut, or publication  
`published: false`  
`production_admitted: false`

## Cause

`skopeo inspect docker://quay.io/fedora/fedora-bootc@sha256:3a5e74e6…`
returned `manifest unknown`. Live `quay.io/fedora/fedora-bootc:44` moved
to index `sha256:e91da1af…`. That is not the freeze dest.

## Correction

The dest-cut digest still exists on the Fedora registry. Raw bytes hash
to `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
(`application/vnd.oci.image.index.v1+json`, amd64 platform
`sha256:68a6e45b…`). Did not follow moved `:44`.

`packaging/browser-vm/produce-base-image-receipt.py produce` from dest-cut
checkout `42035dcbd` / `1788153988` wrote
`/root/mcnf-private/browser-vm-base-digest-42035dcbd.json` (0400). Inspect
PASS. Copy on BigBoy:
`/home/mm/mcnf-s3-42035dcbd/browser-vm-base-digest-42035dcbd.json`.

Image reference in the receipt:
`registry.fedoraproject.org/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`.

## Leftover

S4 derivative image build still to run. Native F44 `.131` RAM handoff
must not halt `.130` while freeze-SHA `farm-gate` uses it.
