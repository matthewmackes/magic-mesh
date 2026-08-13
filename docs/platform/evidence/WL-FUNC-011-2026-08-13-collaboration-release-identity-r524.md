# WL-FUNC-011 — governed Collaboration release identity (r524)

- Date: 2026-08-13
- Scope: first-install authenticated Chat/Collaboration publication readiness.
- Production result: `collaboration/node-signing-seed` remains in the existing
  mackesd SecretStore. A governed detached-signature receipt binds its SHA-256,
  exact Ed25519 public identity, source revision, target node, and
  `system:mackesd` scope. The materializer installs the exact seed and a
  root-owned read-only admission marker. Both Chat and Collaboration now use one
  startup gate that re-attests marker inode/mode, seed digest, public key,
  compiled revision, node, and user before either worker receives signing or
  publication authority.
- Private authority: no seed or signing key is written to the repository or
  receipt. The producer consumes only a transient owner-only SecretStore export;
  the installed materializer reads through `mackesd secret get`.
- Activation: all six split mackesd services require and order after the
  Collaboration identity unit. Workstation, Lighthouse, and Server package
  manifests carry the same canonical helper/service/drop-in assets.

## Farm evidence

- `.50`, slot `func011-activation-final`: hostile producer/materializer
  integration passed; node, SecretStore material, and signature substitutions
  were rejected. Activation/package ordering passed. Python compilation and
  ShellCheck passed.
- `.90`, slot `func011-worker-admission-r2`:
  `collaboration_signer_requires_exact_release_admission` passed 1/1 with 4,962
  filtered; wrong node/user/revision, replaced seed, and writable marker fail
  closed.
- `.90`, same warmed slot:
  `chat_and_collaboration_share_the_release_identity_startup_gate` passed 1/1
  with 70 filtered; neither authenticated worker retains the old direct
  `load_or_create` bypass.
- BigBoy `.130`, slot `func011-strict-clippy-r3`: strict production Clippy was
  attempted and reached mackesd, but the synchronized committed tree has an
  unrelated `cuttlefish_guest.rs` unused import under `-D warnings`. No green
  Clippy result is claimed for that attempt and the out-of-scope Android file
  was not edited.

## Remaining first-release input

The release operator must place the exact 32-byte Collaboration seed in
SecretStore, run the receipt producer for the promoted revision/node, and ship
the signed receipt beside the governed public release key. The first full RPM
build must verify the installed payload and service graph. Installed one-node
Chat/Files/Messages/Tasks publication and recovery proof remains deferred and
non-blocking until after that release.
