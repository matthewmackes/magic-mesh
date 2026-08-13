# WL-ARCH-008 — portable Browser import (r551)

Date: 2026-08-13

## Production slice

`install-helpers/migrate-browser-profile.py` previously produced a verified
portable bundle but had no executable guest-side import operation. The tool now
accepts `--import-bundle` with explicit profile, downloads, and policy
destinations. It verifies the complete bundle before mutation, routes each
admitted category to its guest-owned root, rejects unsafe or credential-bearing
manifest mappings, publishes new files atomically, retains byte-identical files
on a repeated import, and refuses conflicting guest state.

The focused boundary fixture now proves:

- profiles, bookmarks, history, sessions, downloads, policies, and extensions
  retain their exact category/path mapping;
- the first import materializes all six portable fixture payloads;
- the second import is a no-op (`0 imported`, `6 retained`);
- the downloaded fixture survives byte-for-byte;
- cookie, password, passkey, sealed-credential, token, and local-storage
  fixtures do not materialize in the guest destination; and
- conflicting existing guest history is not overwritten.

## Farm evidence

Farm inventory before the gate wave was 8/10 heavy slots active across 5/5
online nodes. The two free `.90` lanes ran distinct gates.

Focused import/export self-test on `172.20.0.90`, slot
`arch008-browser-import-selftest-r551`:

```text
python3 install-helpers/migrate-browser-profile.py --self-test
migrate-browser-profile: self-test passed
```

Portable boundary and redacted-secret fixture gate on `172.20.0.90`, slot
`arch008-browser-import-boundary-r551`:

```text
python3 install-helpers/verify-browser-portable-boundary.py --self-test
migrate-browser-profile: self-test passed
browser portable boundary: PASS
verify-browser-portable-boundary.py: self-test passed
```

Tiny local syntax and scoped hygiene probes also passed:

```text
python3 -m py_compile install-helpers/migrate-browser-profile.py install-helpers/verify-browser-portable-boundary.py
git diff --check -- install-helpers/migrate-browser-profile.py install-helpers/verify-browser-portable-boundary.py
```

## Remaining ARCH-008 acceptance

S2 still requires the first-release migration against admitted real legacy and
guest roots and a second live pass proving the resulting installed state.
Release installation and live one-node Browser VM image/audio/runtime proof are
deferred and non-blocking until after the first full release. S1 standalone
repository/history and any remaining S3 host-stack reachability work remain
separate acceptance criteria.
