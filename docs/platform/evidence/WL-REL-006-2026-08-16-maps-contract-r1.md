# WL-REL-006 Maps contract gate — current checkout

The farm ran the Maps producer and materializer hostile suites from the current
checkout. Both passed:

```text
offline catalog producer hostile suite passed
offline Maps materializer hostile suite passed
```

- Farm host: `172.20.0.170`
- Farm slot: `rel006-maps`
- Producer: `packaging/maps/test-produce-offline-catalog.py`
- Materializer: `packaging/maps/materialize-offline-catalog-test.py`

These tests prove fail-closed handling for changed bytes, wrong hashes,
duplicate/overlapping tiles, unsafe paths, mutable or linked inputs, quota
violations, stale epochs, and output substitution. They do not create a
production Maps input. The producer intentionally never fetches map data, and
the workspace contains no operator-approved OSM-derived tile bundle or approval
receipt to admit. S2 therefore remains externally gated on those exact
provider bytes, attribution/license terms, and approval metadata.
