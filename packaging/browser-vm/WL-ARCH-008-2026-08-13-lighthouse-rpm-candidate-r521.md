# WL-ARCH-008 — governed Lighthouse RPM candidate input (r521)

The Browser image lane now has a canonical, non-secret producer for its exact
`magic-mesh-lighthouse` RPM input. Before publication, the producer snapshots a
single-link immutable RPM, authenticates its signature against the governed
release public key, resolves the reported signing key to one full fingerprint,
and binds the x86_64 NEVRA, whole-file SHA-256, RPM payload SHA-256, source
revision embedded in the installed `mackesd` ELF, thin-Lighthouse variant, and
immutable `browser-vm-chromium-v1` target.

The producer never builds, signs, rewrites, installs, or executes the RPM and
never handles a private key. Publication is mode-0700/mode-0400 and atomic
no-replace. Its verifier mode repeats every admission check against the exact
candidate bytes, so a substituted RPM, signer, target, variant, architecture,
revision, NEVRA, payload digest, or manifest is rejected before Browser image
mutation.

Exact gates:

- `.50`, `arch008-lighthouse-candidate-test-r521b`: hostile producer/verifier
  integration passed, including byte, authority, identity, architecture,
  revision, symlink, and duplicate-field substitutions.
- `.90`, `arch008-lighthouse-candidate-compile-r521b`: bytecode-free Python
  compilation passed for the producer and test.
- `.130`, `arch008-lighthouse-candidate-tabnanny-r521b`: Python tabnanny passed
  for the producer and test.
- Local scoped `git diff --check`: passed. The first `.130` attempt's AST parse
  passed, but its Git check was discarded because farm syncs intentionally omit
  `.git`; it is not acceptance evidence.

The remaining ARCH-008 release inputs are the real release-signed Lighthouse RPM,
its candidate manifest for the exact release revision, an immutable Browser
base image, and the first full Browser image/package build and verification.
Sunshine/alternate transport implementation remains coding work; installed
one-node VDI/audio/migration/reconnect/performance proof is deferred and
non-blocking until after the first release.
