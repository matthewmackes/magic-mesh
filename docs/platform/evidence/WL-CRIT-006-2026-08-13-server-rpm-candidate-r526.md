# WL-CRIT-006 — governed Server RPM candidate (r526)

## Result

The first-release output boundary now has a role-specific producer and owning
reverifier for the `magic-mesh-server` x86_64 RPM variant. Production copies the
already-signed input into a private, no-replace candidate directory as mode
`0400`, records a canonical mode-`0400` companion manifest, and reverifies that
exact candidate before atomic publication.

The manifest binds the `server-rpm` release role, the
`magic-mesh-server/headless-workstation-v1` variant, exact source revision,
canonical NEVRA, payload SHA-256, whole-RPM SHA-256, and full governed signing
fingerprint. The `reverify` interface directly accepts the release collector's
`{artifact}`, `{source_revision}`, and `{signing_identity}` values plus the
companion manifest and governed public key.

Both modes fail closed on a Workstation/Lighthouse or wrong-architecture RPM,
stale embedded build identity, unexpected signer or key, changed candidate
bytes, wrong role/variant/revision/NEVRA/digest manifest fields, duplicate or
malformed JSON, writable inputs, symlinks, and pre-existing/substituted output
authority. Generic safe RPM snapshot, signature, canonical JSON, and atomic
publication primitives are reused from the existing governed RPM candidate
module rather than duplicated.

Input mode admission follows the repository-wide rule: canonical owner-writable
`0644` signed RPMs and the tracked release key are accepted, while any
group/other-writable authority is rejected. Stable snapshots and identity
checks still detect mutation. The hostile suite covers both canonical `0644`
inputs and `0664` refusal, preventing fixtures from hiding a real-key mode
incompatibility.

## Farm evidence

All commands ran from `/root/magic-mesh` with explicit host and isolated slot.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=crit006-server-rpm-hostile-r526 install-helpers/xcp-build.sh sync
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.50 'cd magic-mesh-farm-crit006-server-rpm-hostile-r526 && python3 packaging/server-rpm/test-server-rpm-candidate.py'
PASS: test-server-rpm-candidate: hostile producer/reverifier integration passed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=crit006-server-rpm-compile-r526 install-helpers/xcp-build.sh sync
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.170 'cd magic-mesh-farm-crit006-server-rpm-compile-r526 && python3 -m py_compile packaging/server-rpm/produce-server-rpm-candidate.py packaging/server-rpm/test-server-rpm-candidate.py'
PASS (exit 0)

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=crit006-server-rpm-tabnanny-r526 install-helpers/xcp-build.sh sync
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.130 'cd magic-mesh-farm-crit006-server-rpm-tabnanny-r526 && python3 -m tabnanny packaging/server-rpm/produce-server-rpm-candidate.py packaging/server-rpm/test-server-rpm-candidate.py'
PASS (exit 0)
```

Local `git diff --check -- packaging/server-rpm
docs/platform/evidence/WL-CRIT-006-2026-08-13-server-rpm-candidate-r526.md`
also passed. ShellCheck and Rust build/clippy are not applicable to this
Python-only, packaging-boundary slice.

## Remaining acceptance

Cut and sign the real Server RPM from the pinned first-release source revision,
produce this candidate with the governed release key and expected full signing
fingerprint, then supply `candidate.rpm` and `candidate-manifest.json` to the
seven-role release-output collection plan. The first full release and its
artifact-integrity verification remain pending; post-release one-node live
proof is intentionally deferred and non-blocking.
