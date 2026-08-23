# WL-FUNC-028 S2 — Transfers editor CLI parity — r1

Date: 2026-08-23  
Classification: GUI producer parity; **not** live seat GUI proof or
`production_admitted`  
Source revision: fold of unit `4acf5203439c` onto `31a5c5636`  
Farm: worker `172.20.0.196` slot 1
`xcp-build.sh cargo test -p mde-collab-egui` → **181 passed, 0 failed**

## Slice

Communications Transfers editor now matches the CLI producer:

- Edit is replace-by-ID (`mackesd transfer sync-pair add --id`). A renamed
  draft cannot mint a second store row.
- Vanished projection still publishes `Save` (CLI upsert). Only remove
  refuses unknown ids.
- Successful save/remove keep the CLI queued-next-tick notice after the
  editor closes. The store still updates only when the worker drains the
  inbox.
- Refusal copy matches the CLI.

## Non-claims

This is not live Construct-on-seat proof. Dell leftover-028 was a CLI add.
`production_admitted` was not flipped.
