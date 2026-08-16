# WL-REL-006 input contract gates — current checkout

The canonical hostile and integration suites passed from an immutable clone of
the current checkout on farm host `172.20.0.196`, slot `rel006-preflight`.

- `install-helpers/test-release-input-preflight.sh` — PASS
- `python3 install-helpers/test-release-input-argv.py` — PASS
- `install-helpers/test-release-output-plan-collector-integration.py` — PASS
- Checkout: `ba0e819822e652015ac043c5c9e020f5dd3b43dc`

The preflight suite verified receipt binding for revision, epoch, architecture,
role, reference, manifest, and platform digest; it also proved missing or
mismatched inputs stop before build mutation. The passing “all mandatory inputs”
line is the suite’s isolated fixture admission, not a production release claim.
The real production bundle remains incomplete because Maps provider bytes and
Cuttlefish image/host artifacts are absent.
