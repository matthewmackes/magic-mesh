# WL-CRIT-006 — first-full-release phase driver (r530)

`install-helpers/run-first-full-release.sh` composes the existing governed
release boundaries without acquiring signing authority or promoting output.

The `prepare` phase resolves the exact clean source revision and epoch before
preflight and again immediately before each canonical Fedora 44 farm cut. It
builds Workstation/Lighthouse and Server in disjoint farm slots, records each
unsigned RPM's role, NEVRA, SHA-256 payload identity, whole-file digest, and
size, then emits an immutable `promotion: forbidden` operator-signing handoff.

The explicit `resume` phase rejects cross-revision and cross-Fedora handoffs.
Before derivative mutation it extracts the three signed RPMs from the canonical
seven-role plan input, requires their NEVRA and payload digests to match the
prepared unsigned RPMs, and binds the derivative argv to those exact admitted
Workstation and Lighthouse candidates. It then invokes the existing derivative
builder, release-output plan producer, and collector. It does not sign, publish,
promote, or invoke live acceptance.

Hostile coverage includes wrong initial source identity, source movement at a
build boundary, duplicate output, Fedora-target mismatch, revision mismatch,
same-revision/different-payload signed RPM substitution, and promotion state.

Farm evidence:

- `172.20.0.50`, slot `crit006-first-release-driver-test-r530`:
  `install-helpers/test-run-first-full-release.sh` passed.
- Same farm workspace: strict ShellCheck of the driver and focused test passed.
- Local Bash syntax and `git diff --check` passed.

No real release cut was attempted: governed release inputs and operator-signed
candidates are intentionally required at the live phase boundary.
