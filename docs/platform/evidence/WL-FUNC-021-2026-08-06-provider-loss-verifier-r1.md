# WL-FUNC-021 — provider-loss verifier boundary audit (2026-08-06)

## Finding and bounded fix

The observation helper previously classified `active unavailable unavailable
ok` as `provider_loss`. That sample proves neither a provider-only outage nor a
healthy catalog boundary: both the direct provider ping and the catalog read
failed. Counting it could therefore accept a simultaneous provider/catalog
failure as the natural provider-loss transition described by the helper.

`install-helpers/verify-music-live-provider-loss.sh` now requires
`catalog=ok` for `provider_loss`. A provider-only loss remains the narrower
sample `active unavailable ok ok`; a sample with an unavailable catalog is
`ambiguous` and cannot advance the loss/recovery proof. The self-test covers
both cases. No live seat, provider, service, network interface, or playback
state was mutated.

## Verification

- `bash -n install-helpers/verify-music-live-provider-loss.sh` — passed.
- `install-helpers/verify-music-live-provider-loss.sh --self-test` — passed;
  no SSH was attempted.
- `shellcheck install-helpers/verify-music-live-provider-loss.sh` — the
  script remains syntax-valid but ShellCheck reports existing SC2251 notices
  for negated self-test predicates and SC2317 notices for the EXIT-trap
  cleanup path. These are unrelated to the classifier change.

The operator-authorized Dell observation on 2026-08-06 produced 15 consecutive
`service=active provider=ok catalog=ok state=ok` samples over 45 seconds, then
returned the expected refusal because no natural provider loss occurred. No
provider interruption or playback change was requested. A naturally occurring
live loss and recovery therefore remain unclaimed.
