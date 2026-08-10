# WL-ARCH-009 — mirror-sync process owner gate (r109)

Date: 2026-08-10

Base revision: `4ba2428d`

## Ownership defect

Each grouped process called the common probe/observability spawn path. Although
`Supervisor` correctly refused Data-owned `mirror_syncd` outside the Data
group, the caller unconditionally constructed the worker and appended its name
to the process roster afterward. Five groups could therefore advertise a
worker they did not run, corrupting the bounded runtime snapshot.

## Correction

Construction, supervisor spawn, and roster insertion now share one
`accepts_worker("mirror_syncd")` gate. A rejected process performs none of the
three operations. The production helper is exercised for all six groups; only
Data receives a runtime row, and that row retains the canonical Data contract.

Machine 193 slot `arch009-mirror-owner` passed the exact six-group regression
(1/1, 68 filtered out) and `git diff --check`. File-wide formatting still
reports unrelated existing drift and was not rewritten.
