# WL-ARCH-009 — Process-isolated mackesd and unified Workers interface

**Disposition: Done — implementation closed; live/package proof requirement
lifted by operator decision (2026-08-15).**

The six supervised worker groups, bounded contracts, single SQLite writer,
Workers/Action Console ownership, legacy route normalization, Network
Operations cutover, and typed KVM health ownership are recorded in the ARCH-009
farm and route evidence, including:

- `docs/platform/evidence/WL-ARCH-009-2026-08-14-mackesd-full-farm-gate-r1.md`
- `docs/platform/evidence/WL-ARCH-009-2026-08-13-workers-sole-authority-cutover-r493.md`
- `docs/platform/evidence/WL-ARCH-009-2026-08-13-network-operations-route-cutover-r547.md`

The operator explicitly lifted the remaining fleet/package/live and
three-lighthouse proof requirement. This disposition does not claim that
those live artifacts exist; they remain optional post-release evidence under
the release-proof process. No additional seat requirement is imposed.
