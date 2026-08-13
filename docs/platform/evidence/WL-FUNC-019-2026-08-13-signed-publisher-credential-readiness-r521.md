# WL-FUNC-019 signed publisher credential readiness — r521

Date: 2026-08-13

Commit: pending at gate time

## Result

The first-release resource-publisher credential now has one governed producer
and one installed admission boundary. The producer reads a root-only transient
export of the existing `resource/publisher-hmac` SecretStore value, but publishes
only a SHA-256 commitment plus a detached OpenPGP signature. Its receipt binds:

- the exact Ed25519 primary identity in the committed release public key;
- the matching governed release secret authority used only by the producer;
- the exact source Git revision;
- one canonical `peer:<node>` target and one Lighthouse/Workstation role; and
- the exact existing HMAC bytes without writing those bytes to output.

The installed `provision-resource-publisher-credential` helper snapshots and
verifies the receipt, detached signature, project public key, and role file
before producing any encrypted systemd credential or drop-in. It then compares
the approved SecretStore bytes to the signed commitment. Missing, malformed,
wrong-role, wrong-node, wrong-key, replaced, multiply-linked, symlinked, or
unsigned input fails before activation output. The HMAC remains in the existing
SecretStore and host-bound `systemd-creds` path; neither the release signing
secret nor a second trust store reaches the daemon.

The audit also found and fixed an existing validator discrepancy: a trailing
newline was documented as invalid but was accepted because record separators
are not matched by `grep` and command substitution strips trailing newlines.
The materializer now requires exactly zero newline records.

## Farm evidence

- `.50`, slot `func019-publisher-producer-r521`: the hostile producer suite
  passed. It covered governed Ed25519 signing, public-only output, exact
  revision/node/role binding, missing and control-bearing credentials, invalid
  node scope, wrong secret authority, and no-replace publication.
- `.170`, slot `func019-publisher-materializer-r521`: the installed helper
  self-test passed against a real ephemeral Ed25519 signature and rejected
  wrong-role, wrong-HMAC, replaced-receipt, retry-policy, and malformed-key
  fixtures.
- `.196`, slot `func019-publisher-static-r521`: Python bytecode compilation
  passed for the producer and hostile test.
- `.50`, the completed producer workspace reused after its first command exited:
  strict ShellCheck passed for the materializer with no exclusions.
- Local `bash -n`, Python compilation, and scoped `git diff --check` passed.

The initial direct farm dispatch omitted the canonical SSH identity and was
rejected before command execution. A first fixture run then exposed the
read-only mutation setup defect described during the run; no result from either
attempt is claimed. `.90` and `.196` were also found not to carry ShellCheck, so
the unique static gate was routed to `.50` rather than duplicated.

## Remaining acceptance

For the exact first release, the operator must export the already-governed
`resource/publisher-hmac` value into a root-only transient file, run the producer
with the release signing authority for each intended node/role, and install the
resulting receipt/signature at
`/etc/mcnf/release-inputs/resource-publisher/` alongside the committed release
public key. The full release must then verify and package that handoff. The
post-release one-node publisher activation/rotation, universal resource routes,
recovery, authenticated Windows RDP rendering, and visual capture remain
deferred and non-blocking under the current release policy.
