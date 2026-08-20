# WL-REL-007 evidence — candidate-bound release-stage resume

Date: 2026-08-20  
Scope: `automation/promotion/` orchestration and helper files only  
Status: focused implementation evidence; not production-release evidence

## Exact gap

`mcnf-promotion-cycle.sh` previously appended gate lines to
`evidence.jsonl`, but stage execution had no owner claim and the cycle did not
recheck that the current checkout still matched the source revision bound into
the receipts. After an interruption, another coordinator could repeat a stage
or resume a receipt against a moved checkout.

## Implemented behavior

`release-stage-journal.sh` now writes one atomic `ReleaseStageReceiptV1` JSON
receipt per stage, keyed to the candidate RPM SHA-256 and source revision. A receipt is written
only after the stage returns successfully. Each next stage requires a passing
receipt for the same candidate from its declared predecessor; an existing
different receipt cannot be replaced. `mcnf-promotion-cycle.sh cycle` uses the
journal to skip completed matching stages and resume at the first incomplete
stage. The journal is execution state under `MCNF_PROMOTION_STATE_DIR`, not a
substitute for signed production evidence. Before any stage, the coordinator
now atomically claims `<stage>.owner.json`; a different owner or source cannot
run that stage. The coordinator also compares the repository's current `HEAD`
to `MCNF_RELEASE_SOURCE_REVISION` before resuming or advancing, so a moved
checkout fails closed instead of reusing a stale receipt.

## Focused regression verification

Farm lane: `172.20.0.90`, slot `2` (admitted with 19G free; 5/10 farm slots
were active at topology inspection).

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 install-helpers/xcp-build.sh sync
ssh -i ~/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'cd ~/magic-mesh-farm-2 &&
  automation/promotion/release-stage-journal.sh self-test'
release-stage-journal: ALL PASS
```

The self-test covers receipt creation, matching-stage idempotent resume,
predecessor enforcement, candidate mismatch refusal, competing-owner refusal,
and owner schema presence. Local `bash -n`, `git diff --check`, and the same
journal self-test also passed.

No provider, signing key, live topology, installed seat, publication, or
production evidence was fabricated or asserted by this change. Full
WL-REL-007 remains open pending its real six-role release chain and gates.
