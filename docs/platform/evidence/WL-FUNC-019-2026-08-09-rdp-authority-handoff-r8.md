# WL-FUNC-019 RDP authority handoff — r8

Date: 2026-08-09

Base revision: `5c57e172d047ee06f0b3397ec4623a261c66f613` plus this lane.

## Production correction

An authenticated, approval-gated Desktop/RDP catalog action can now reach the
existing masked Windows credential prompt without becoming an editable Manual
source. The shell binds approval to the exact catalog revision, content digest,
resource, action, transport, capability, endpoint, and expiry, then publishes
one root-authorized resource action.

The prompt is exposed only after the router has dispatched the exact Open and
the shell has cryptographically verified its signed
`VdiAuthorityCompletionReply` with the existing host-bound `cloud-arm-key`.
A nonempty or substituted signature is insufficient. The resulting ephemeral
source is labeled `Catalog authority`, not mDNS or Manual.

Cancellation remains effective after the short catalog/approval window. The
shell retains the signed Open receipt and submits an exact Close with a fresh
cancellation request. The daemon verifies the receipt HMAC, original request,
session, card/action/transport binding, downstream message identity, and digest
before allowing the cleanup-only catalog/approval bypass. User cancellation and
catalog revocation dispatch that Close; a failed Close retains live state for a
visible retry instead of merely hiding the request. Ordinary external Close is
not weakened and no second local approval is fabricated.

## Focused farm verification

Machine 196 (`172.20.0.196`) passed the shell receipt/forgery/cancellation
regression:

```text
cargo test -p mde-shell-egui --locked \
  chooser::resources::tests::rdp_handoff_requires_an_exact_accepted_authority_receipt \
  -- --exact
```

Result: **1 passed, 0 failed**.

The corrected daemon route was then verified in the isolated
`func019-rdp-handoff-r8-final` slot:

```text
cargo test -p mackesd --lib --features async-services \
  workers::service_aggregator::resource_actions::tests::approval_gated_vdi_requires_and_preserves_the_exact_local_binding \
  --locked -- --exact
```

Result: **1 passed, 0 failed; 4,381 filtered out**. The test covers absent and
substituted approval, receipt-bound Close without a current catalog, refusal of
an unsigned ordinary external Close, cross-request/action substitution, and a
forged completion signature. Exact-file formatting passed for the two changed
implementation modules; the chooser parent retains unrelated pre-existing
format drift outside this lane. Scoped `git diff --check` passed.

One earlier diagnostic omitted `--lib`, compiled unrelated test targets, and
failed an obsolete refusal-code assertion. It is not counted as verification;
the assertion was corrected and the exact success-critical command above is the
final result. No further broad test was run.

## Live boundary

Seat 15 still detects `172.20.146.54:3389`, but this checkpoint does not claim a
live Windows login or rendered session. Live proof still requires the
authoritative resource-publisher HMAC/attestation on the seat, the host-bound
cloud authority credential, operator-supplied Windows credentials, and an
observed Open/login/Close/revocation round trip.
