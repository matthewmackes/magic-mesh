# Surface Pro 5/6 governed physical acceptance

This procedure records operator observations; it is not a second worklist and
does not itself exercise or mutate hardware. The recorder only reads an
existing `collect-surface-acceptance.py` bundle and an operator-authored JSON
document, then writes a new mode `0600` evidence record. It never overwrites a
record.

Run and validate the canonical Pro 6 (`Surface`) first. The collector bundle
must identify exact model `Surface Pro 6` and generation 6. Prepare an
observations file with this exact top-level shape:

```json
{
  "schema_version": 1,
  "revision": "0123456789abcdef0123456789abcdef01234567",
  "observer": "operator-id",
  "observations": []
}
```

The `observations` array must contain each of these IDs exactly once and no
unknown IDs: `touch`, `pen`, `type-cover`, `buttons`, `microsd`, `rotation`,
`camera-privacy`, `audio-microphone`, `suspend-s0ix`, `reboot-upgrade`,
`drm-modes`, and `fingerprint`. Each entry has exactly this structure:

```json
{
  "check": "touch",
  "performed": true,
  "outcome": "pass",
  "observed_at_utc": "2026-08-09T18:30:00Z",
  "observation": "Operator exercised ten contacts and both display edges",
  "limitations": []
}
```

For `camera-privacy`, use the Surface card's separately authorized and armed
one-frame proof when the fixed libcamera provider is available, and record its
closed outcome plus the operator's physical privacy-indicator observation. The
proof retains no frame and is not a preview. For `fingerprint`, record only a
hands-on operator result or an explicit `blocked`/`unsupported` limitation;
read-only fprintd enumeration is capability evidence, not biometric function.
Missing hardware, provider support, seat access, or an operator is an external
blocker and must remain `blocked`/`unsupported`, never an inferred pass.

Allowed outcomes are `pass`, `fail`, `blocked`, and `unsupported`. `pass` and
`fail` require `performed: true`; `blocked` and `unsupported` require
`performed: false`. A pass cannot declare a limitation. Every non-pass must
declare at least one explicit limitation. This makes incomplete and
contradictory structured claims invalid without attempting to infer a pass
from prose. Observations must be timestamped no earlier than the collector
bundle and may not be future-dated. Credential-like strings and network
identifiers are refused rather than copied into evidence.

Record and independently validate Pro 6:

```bash
sudo install-helpers/record-surface-physical-acceptance.py record \
  --bundle /var/tmp/surface-pro6-acceptance \
  --observations /var/tmp/surface-pro6-observations.json \
  --out /var/tmp/surface-pro6-physical-acceptance.json

sudo install-helpers/record-surface-physical-acceptance.py validate \
  --bundle /var/tmp/surface-pro6-acceptance \
  --record /var/tmp/surface-pro6-physical-acceptance.json
```

The record binds the exact 40-character deployment revision, DMI model/SKU and
generation, seat, collection and observation timestamps, collector manifest
and every artifact SHA-256, collector SHA-256, the installed `magic-mesh`
package identity, all limitations, and every
operator observation. The revision is explicitly marked as operator-declared;
the recorder does not pretend the older collector can derive it from a binary.
Exit `0` means the inventory was complete and every explicitly performed check
passed. Exit `3` means the
record is structurally complete and hash-valid but at least one inventory probe
or physical check was non-passing. Exit `2` means invalid, incomplete,
duplicate, unknown, contradictory, tampered, or unsafe input. A valid record
never turns `blocked` or `unsupported` into a pass.

Only after a completed Pro 6 record exists, collect and record the optional Pro
5. Its bundle must use a distinct seat label and identify exact model
`Surface Pro` with SKU `Surface_Pro_1796` or `Surface_Pro_1807`. The Pro 5
record requires and hashes the validated prior Pro 6 record:

```bash
sudo install-helpers/record-surface-physical-acceptance.py record \
  --bundle /var/tmp/surface-pro5-acceptance \
  --observations /var/tmp/surface-pro5-observations.json \
  --prior-pro6-bundle /var/tmp/surface-pro6-acceptance \
  --prior-pro6-record /var/tmp/surface-pro6-physical-acceptance.json \
  --out /var/tmp/surface-pro5-physical-acceptance.json

sudo install-helpers/record-surface-physical-acceptance.py validate \
  --bundle /var/tmp/surface-pro5-acceptance \
  --record /var/tmp/surface-pro5-physical-acceptance.json \
  --prior-pro6-bundle /var/tmp/surface-pro6-acceptance \
  --prior-pro6-record /var/tmp/surface-pro6-physical-acceptance.json
```

Parser-only hostile admission tests require no hardware:

```bash
install-helpers/record-surface-physical-acceptance.py --self-test
```
