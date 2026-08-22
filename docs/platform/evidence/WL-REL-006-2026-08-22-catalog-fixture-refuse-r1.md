# WL-REL-006 catalog fixture refuse — r1

Date: 2026-08-22  
Classification: producer refuse; **not** a production curated Flatpak
catalog and **not** `production_admitted`  
Source revision: after `7d27ac7f2` (this change)  
`production_admitted: false`

`packaging/app-vm/produce-catalog-receipt.py` already required non-empty
`curated` refs of the form `name@sha256:<64 hex>`. A digest-pinned
`org.example.App` still minted a receipt. That IANA reserved example id
is the in-tree test fixture and must not become a production catalog.

## Act

The producer now refuses any ref whose app id starts with `org.example.`.
The happy-path self-test uses `org.mcnf.test.CatalogPin@sha256:` plus 64
hex (local fixture only; not a production dest). The preflight self-test
catalog fixture matches that pin so the producer still admits the
script's throwaway object.

No curated refs were invented. No private catalog dest was written.
Existing private dests were not replaced.

## Verification

Local (tiny helper, no cargo):

- `python3 packaging/app-vm/test-produce-catalog-receipt.py` → PASS
- `install-helpers/test-release-input-preflight.sh` → PASS

Leftover is still a real operator-approved digest-pinned curated catalog
(not `org.example.*`, not `org.mcnf.test.*` as production).
