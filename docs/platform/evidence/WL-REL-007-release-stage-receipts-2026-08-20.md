# WL-REL-007 evidence — candidate-bound release-stage resume

Date: 2026-08-20  
Scope: `automation/promotion/` orchestration and helper files only  
Status: focused implementation evidence; not production-release evidence

## Exact gap

`mcnf-promotion-cycle.sh` previously appended gate lines to
`evidence.jsonl`, but the cycle had no restart-safe execution checkpoint. After
an interruption it always restarted at inventory/build and could repeat already
completed destructive stages. The existing evidence log was not a
candidate-bound compare-and-swap stage journal and therefore could not safely
authorize resumption.

## Implemented behavior

`release-stage-journal.sh` now writes one atomic `ReleaseStageReceiptV1` JSON
receipt per stage, keyed to the candidate RPM SHA-256 and source revision. A receipt is written
only after the stage returns successfully. Each next stage requires a passing
receipt for the same candidate from its declared predecessor; an existing
different receipt cannot be replaced. `mcnf-promotion-cycle.sh cycle` uses the
journal to skip completed matching stages and resume at the first incomplete
stage. The journal is execution state under `MCNF_PROMOTION_STATE_DIR`, not a
substitute for signed production evidence.

## Focused regression verification

Farm lane: `172.20.0.50`, slot `1` (admitted with 19G free; 3/10 farm slots
were active at topology inspection).

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh sync
ssh ... mm@172.20.0.50 'cd ~/magic-mesh-farm-1 &&
  automation/promotion/release-stage-journal.sh self-test'
release-stage-journal: ALL PASS
```

The self-test covers receipt creation, matching-stage idempotent resume,
predecessor enforcement, candidate mismatch refusal, and schema presence.
Local `bash -n` syntax checks also passed for both promotion scripts.

No provider, signing key, live topology, installed seat, publication, or
production evidence was fabricated or asserted by this change. Full
WL-REL-007 remains open pending its real six-role release chain and gates.
