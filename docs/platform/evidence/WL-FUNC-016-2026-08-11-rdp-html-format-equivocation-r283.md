# RDP HTML registered-format equivocation evidence — 2026-08-11

- Scope: CLIPRDR accepts registered `HTML Format` only when the advertised list
  binds one wire ID to that descriptor without contradictory aliases.
- Failure behavior: two IDs claiming HTML, or one ID carrying HTML and a
  different/unnamed descriptor, fail negotiation. Exact duplicate descriptors
  remain idempotent and produce one bounded CF_HTML request.
- Production path: the live-connect clipboard backend uses the same negotiation
  before binding a format-data response to HTML.
- Farm gate: BigBoy `.130`, slot 3 with `live-connect`: **1 passed, 0 failed,
  102 filtered**.
- Scoped `git diff --check`: passed.
