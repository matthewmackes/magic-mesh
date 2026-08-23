# WL-REL-001 — Construct brand epoch includes 13.x — r1

Date: 2026-08-23  
Classification: version-surface repair; **not** S1/S4 freeze or
`production_admitted`  
Source revision: post-`144188849` plus this change  
Control host: `rocky9-kvm2`  
Farm: `172.20.0.170` slot 1 (`xcp-build.sh cargo test -p mde-theme`)

## Defect

`mde-theme` `codename_for` mapped only major `12` to `Construct`. Workspace
identity is `13.0.0`, so farm `cargo test -p mde-theme` failed three brand
tests with empty codename (`19 passed; 3 failed` on `.170` slot 2 before
this map).

## Repair

`codename_for(12 | 13)` returns `Construct`. Unknown majors still return
`""`. Visible product name stays Construct; package id stays `magic-mesh`.

## Farm

`test result: ok. 22 passed; 0 failed` on `mde-theme` lib. This is not a
freeze. Dest-cut SHA remains `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`.
`production_admitted` was not flipped.
