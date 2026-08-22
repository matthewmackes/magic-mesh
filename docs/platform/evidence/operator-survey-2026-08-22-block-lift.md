# Operator survey 2026-08-22 — block-lift rulings

Recorded from the drain-branch survey. Newest operator lock wins.

| ID | Ruling |
|---|---|
| Q9 mesh-PBX | Delete after fleet-negative. `WL-FUNC-033` is Remaining. |
| Q26 Files | Keep `Surface::Files` as its own OS surface. |
| PR #71 | Mark Ready. Branch HEAD is the input-generation candidate. |
| FUNC-023 bar | Final freeze waits on a real-seat enroll/offboard over SSH. |
| Maps S2 | Fetch Geofabrik New York PBF + official Erie/Niagara geometry; render locally; never public OSM tiles. |
| Preflight S7 | Agent writes a redacted template; operator fills secrets and `chmod 0400`. |
| TEST-002 | Dell, Seat 15, and Surface may be mutated (red alert + 5s) when the unpublished candidate exists. Use sealed Vitelity/SIP creds. |

## What this does not lift

`WL-REL-001` final freeze, and therefore `WL-REL-002`–`005` and `WL-TEST-002`, stay Blocked until the live enroll exists and REL-006 admits current-revision inputs against the same SHA.

## Private preflight template

Path (not in Git): `/root/mcnf-private/release-preflight.template.json`

Replace every `REPLACE_*` field, bind `source_revision` / `source_epoch` to the candidate, then `chmod 0400`. Do not copy secrets into the repo.
