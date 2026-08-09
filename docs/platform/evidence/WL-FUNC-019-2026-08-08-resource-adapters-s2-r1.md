# WL-FUNC-019 resource adapters S2 — 2026-08-08

The production service aggregator now augments its universal resource catalog
from the canonical peer directory, node-scoped typed Workload state, admitted
App VM and Android catalogs, and the typed Media roster. Workload and catalog
topics are derived only from the bounded approved peer set plus the local
publisher; the adapter does not wildcard-discover Bus topics or inspect provider
commands, URLs, paths, or credentials. File shares are projected only when they
arrive as the existing typed `MediaKind::FileShare`; there is no fabricated
second generic file/share authority.

Each source publishes an explicit retained adapter status. Missing, malformed,
stale, and directory-authority-unavailable inputs remain visible rather than
becoming an empty successful catalog. Peer, Workload, App VM, Android, Media,
and typed file-share observations project stable Mesh identities with provenance
and freshness. Exact duplicate cards collapse deterministically; conflicting
observations under one stable resource ID produce one unavailable, actionless
card and a visible conflict row. Workload cards advertise generation-bound
Start, Resume, or Launch only when the observed state makes that action safe;
otherwise they expose Inspect only. App, Android, and Media cards remain
actionless until their typed action authority is implemented.

Bounds are 64 peer rows, 64 approved nodes, the shared 256 KiB Workload wire
limit, and 1,024 adapted cards. The existing resource contract performs final
catalog/card validation before the digest and retained mirrors are published.

## Verification

- Farm `.170`, slot `func019-workload-cancel-card-s5-r1`:
  `cargo test --locked -p mackesd --lib resource_adapters --features
  async-services -- --nocapture` passed 12/12.
- Fixtures cover stable order; peer, Workload, admitted App VM, admitted Android,
  Media, and typed file-share projection; deterministic conflict collapse;
  unavailable peer authority; stale Workload state; and oversized peer refusal.
- Scoped formatting and whitespace checks passed. The production aggregator
  registration is exercised through the existing supervised worker path.

## Remaining acceptance gap

There is no independent canonical generic file/share production projection, so
only typed file shares in the Media roster can be admitted today. The closed
action schema has no standalone Cancel card action; cancellation remains bound
to an accepted action receipt. Full UI presentation and live loss/rejoin proof
also remain, so FUNC-019 stays `Remaining`.
