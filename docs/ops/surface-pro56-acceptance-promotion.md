# Surface Pro 5/6 acceptance promotion input

`verify-surface-acceptance-promotion.py` creates a fail-closed Surface input for
the existing WL-CRIT-006 promotion process. It does not promote an image, edit a
worklist or evidence ledger, change a service, or exercise hardware. It writes
one new mode `0600` JSON lock and never overwrites one.

Promotion admission requires all of the following:

- the Fedora 44 Surface provenance manifest is `ready`, and its exact RPMs and
  signing key pass `verify-surface-stack.sh` against the supplied artifact
  directory;
- a fresh, read-only canonical Pro 6 deployment preflight binds that manifest,
  a clean exact revision, admitted remote revision, and collector hash;
- the canonical `Surface` Pro 6 physical record and its collector bundle pass
  the governed record validator with verdict `accepted`;
- deployed Surface package NEVRAs exactly equal the signed candidate NEVRAs,
  while the exact `magic-mesh` package identity and 40-character revision are
  bound into the output;
- explicit passing observations mention every required audio path, the complete
  suspend/resume/S0ix plus Wi-Fi/Bluetooth/mesh recovery sequence, and cold
  boot/reboot/upgrade/rollback/Secure-Boot recovery; and
- the hash-bound `audio.json`, `power.json`, `radios.json`, and `services.json`
  collector artifacts all have status `ok`; and
- the hash-bound `camera-proof.json` is a successful shared camera proof for the
  exact local node/model/generation, was fresh at collection, and explicitly
  retained no frame bytes, device identifier, or request identifier.

Physical records and collector snapshots older than seven days are stale. The
current preflight format has no internal timestamp, so the verifier transparently
uses its filesystem modification time and requires it to be no older than 24
hours; the preflight bytes and observed mtime are both recorded in the output.
Future-dated or misordered timestamps are refused. Re-run the read-only
preflight rather than touching its timestamp.

Run for the required Pro 6 acceptance:

```bash
sudo install-helpers/verify-surface-acceptance-promotion.py \
  --manifest packaging/surface/surface-stack.f44.json \
  --artifact-dir packaging/surface/artifacts \
  --preflight /var/tmp/surface-pro6-deployment-preflight.json \
  --pro6-bundle /var/tmp/surface-pro6-acceptance \
  --pro6-record /var/tmp/surface-pro6-physical-acceptance.json \
  --out /var/tmp/surface-acceptance-promotion.json
```

When the optional Pro 5 parity seat has an accepted record, add both inputs:

```bash
  --pro5-bundle /var/tmp/surface-pro5-acceptance \
  --pro5-record /var/tmp/surface-pro5-physical-acceptance.json
```

The Pro 5 record must hash-bind the admitted Pro 6 record, use an allowlisted
Pro 5 SKU, run the same exact deployment revision and `magic-mesh` identity,
and match every signed Surface package. Supplying only one Pro 5 input is an
error. Omitting both records that the optional seat was not part of this
promotion input; it never fabricates parity acceptance.

There is no manual override, assumed pass, incomplete acceptance, or
foreign-model compatibility path. Exit `0` and a newly written lock are the
only ready result. Any missing, stale, blocked, non-passing, mismatched, or
tampered input exits `2` without writing a promotion lock.
Collector bundles and physical records created before the camera-proof binding
are deliberately not grandfathered; repeat the read-only collection and
governed recording steps.

Run hostile parser/contract tests without artifacts or hardware:

```bash
install-helpers/verify-surface-acceptance-promotion.py --self-test
```
