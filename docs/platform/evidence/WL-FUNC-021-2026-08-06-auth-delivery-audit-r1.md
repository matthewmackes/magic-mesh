# WL-FUNC-021 Music mutation authorization delivery audit (2026-08-06)

Status: evidence/design record only. This note does not change the active
worklist, the authorization implementation, a unit file, or any credential.

## Executive finding

The current Music Bus mutation verifier is intentionally fail-closed, but its
production delivery path is incomplete for a user service:

* `mde-musicd` verifies the same short-lived, exact-body HMAC capability format
  used by `mackesd`, and looks for `cloud-arm-key` below
  `$CREDENTIALS_DIRECTORY`.
* The packaged `mde-musicd.service` is a user unit. It sets the shared Bus root
  and starts `mde-musicd serve`, but has no `LoadCredential*` directive.
* The existing provisioning helper materializes the encrypted root credential
  and installs its drop-in only for `mackesd.service` and the root DRM shell.
* Therefore the user service has no `CREDENTIALS_DIRECTORY/cloud-arm-key` and
  cannot verify an armed Music mutation. An unsigned request is correctly
  refused; the root cloud-arm secret must not be copied into the user unit.

The safe implementation choice is still open. The preferred boundary is a
root-mediated Music authorization path or a new asymmetric contract with a
public verifier in `mde-musicd`. A separate per-seat Music-only credential is a
possible lower-assurance alternative, but it must never reuse `cloud-arm-key`
and requires an explicit seat-user trust decision.

## Implemented facts

### Music daemon verifier and mutation scope

`crates/services/mde-musicd/src/bus_responder.rs:40-56` documents that the
daemon intentionally duplicates the `mackesd::ipc::action_auth` wire contract:
schema version 1, canonical request digest, v2 armed tokens, a 30-second
maximum lifetime, and durable nonce claims. The verifier names its credential
`cloud-arm-key` at `:52-56`.

`crates/services/mde-musicd/src/bus_responder.rs:114-128` constructs the
production authorizer. `load_production_key()` requires an absolute
`CREDENTIALS_DIRECTORY`, reads `cloud-arm-key` from that private directory, and
decodes exactly 32 HMAC bytes (`:190-214` in the same file). If loading fails,
the authorizer logs that Music mutations are disabled and keeps no key.

`crates/services/mde-musicd/src/bus_responder.rs:2373-2384` maps each mutable
Music verb to `music-<verb>`, the local node, and a closed target scope such as
`queue`, `transport`, `playlists`, or `peer-takeover`. Read-only browse/state
verbs do not enter this authorization gate. Missing, malformed, expired,
wrong-scope, body-tampered, or replayed tokens are rejected before mutation.

`crates/services/mde-musicd/src/main.rs:166-176` starts the responder through
`mde-musicd serve`; the packaged daemon therefore owns the `action/music/*`
responder but does not own the root signing authority.

### Root signing authority

`crates/desktop/mde-shell-egui/src/iac/mod.rs:93-112` makes the production
cloud signer root-only, requires an absolute `CREDENTIALS_DIRECTORY`, reads
`cloud-arm-key`, decodes it with the shared cloud type, and refuses otherwise.
The bounded reader at `:114-180` rejects non-regular files, follows no final
Linux symlink, and caps the read.

`crates/desktop/mde-shell-egui/src/iac/mod.rs:182-242` mints a v2 token with a
random nonce, 30-second expiry, exact verb/node/target, and the canonical
request digest. Tests use a deterministic fixture key only under `cfg(test)`;
production uses the root/systemd loader.

The shared wire shape is implemented in
`crates/mesh/mackes-mesh-types/src/cloud.rs:1196-1374`:
`CloudArmedToken`, `CloudArmSigner`, `cloud_request_digest`, and the
`armed_token` encoding. The root verifier is separately implemented in
`crates/mesh/mackesd/src/ipc/action_auth.rs:17-160`; the Music copy is required
to remain byte-compatible while it exists.

### Package and provisioning boundary

`packaging/systemd/mde-musicd.service:8-27` is a user-manager unit. Its only
relevant environment setting is `MDE_BUS_ROOT=/run/mde-bus` (`:17-22`); there
is no `User=`, `CREDENTIALS_DIRECTORY`, `LoadCredential=`, or
`LoadCredentialEncrypted=` line. `packaging/README.md:14-23` explicitly calls
it a user unit wanted by `default.target`.

The root credential drop-in is exactly
`packaging/systemd/cloud-arm-credential.conf:1-4`:

```ini
[Service]
LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key
```

`install-helpers/provision-cloud-arm-credential.sh:7-11` names the mesh-wide
secret and root ciphertext. Its materialization uses a root-only temporary
directory and `systemd-creds encrypt` (`:85-123`). Its install loop at
`:126-140` writes the drop-in only for `mackesd.service` and
`mde-shell-egui.service`; `mde-musicd.service` is deliberately absent.
`packaging/systemd/mcnf-cloud-arm-credential.service:1-23` runs this helper as
a best-effort root post-start provisioning step.

`packaging/bootc/units/mde-shell-egui.service:60-71` records the current
policy: the root DRM shell is the only production token minter, and its
credential drop-in appears only after host ciphertext exists.

### Why the user unit cannot load the root encrypted key

The failure is a service-manager and trust-boundary mismatch, not an Airsonic
credential problem:

1. `/etc/credstore.encrypted/cloud-arm-key` is host-bound encrypted material
   owned by `root:root` with mode `0600` on the observed workstation. Its
   ciphertext is not a user-service input.
2. `LoadCredentialEncrypted=` creates a private credential directory for the
   service manager that successfully loads the directive and then exposes its
   path through `CREDENTIALS_DIRECTORY`. It is not a global environment
   variable and is not inherited from `mackesd.service` or the DRM shell.
3. The current provisioning helper installs the directive only in the two
   root system-service drop-ins listed above. The user `mde-musicd` manager has
   neither that directive nor a usable credential directory.
4. A read-only user-manager smoke test on the non-production seat attempted
   to load the root ciphertext with:

   ```sh
   systemd-run --user --wait --pipe \
     -p LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key \
     /usr/bin/sh -c 'test -r "$CREDENTIALS_DIRECTORY/cloud-arm-key"'
   ```

   It returned `243/CREDENTIALS`. No key bytes were printed or copied. The
   installed `mde-musicd` journal separately reports
   `music mutation authorization unavailable; mutations are disabled`, which
   is the expected fail-closed result.

Adding the same line to the user unit is not an approved fix: it would still
need a credential that the user manager can decrypt/read, and granting the
user service the root cloud-arm key would give a seat-user process the mesh-wide
root mutation capability. Copying the ciphertext or plaintext to a user path,
putting it in an environment variable, or weakening the verifier would violate
the current boundary.

The Airsonic provider credential is a distinct path. `crates/services/mde-musicd/src/creds.rs:36-55`
loads the seat user's `~/.local/share/mde/airsonic-creds.json` and validates
provider configuration; it is not a Bus mutation signing credential and must
not be repurposed as one.

## Safe options and exact code/package boundaries

These are design candidates, not implemented changes.

### Option A — root-mediated Music authorization (preferred if HMAC is retained)

Keep `cloud-arm-key` exclusively inside root `mackesd`/the root shell. Add a
narrow Music authorization endpoint owned by a root component that authenticates
the local seat peer, validates the closed Music verb/target/body contract, and
returns or forwards only the authorized typed result. `mde-musicd` must not
receive the root HMAC key.

Likely boundaries:

* Add the request/reply contract and peer-identity checks under
  `crates/mesh/mackesd/src/ipc/` (or a dedicated root worker), reusing the
  existing `crates/mesh/mackesd/src/ipc/action_auth.rs` verifier and the
  `mackes-mesh-types` canonical digest types.
* Change the Music mutation ingress in
  `crates/services/mde-musicd/src/bus_responder.rs` only at the trust boundary;
  queue/provider/playback logic remains owned by `mde-musicd`.
* Change the shell/UI publisher path in
  `crates/desktop/mde-shell-egui/src/` to call the narrow Music endpoint rather
  than minting a token for a user daemon.
* Package a private root IPC endpoint or root worker with the existing
  `mackesd.service` boundary. Do not add `cloud-arm-key` to
  `packaging/systemd/mde-musicd.service`.

Required proof: same-body authorization, wrong verb/target, replay, expiry,
seat-identity mismatch, malformed JSON, and broker-unavailable refusal. The
broker must never accept an arbitrary action topic or return the root key.

### Option B — asymmetric per-seat verification (preferred if Music must verify locally)

Replace the shared HMAC requirement for the Music lane with a versioned
signature contract: a root-only private signer remains in `mackesd`/the shell,
while `mde-musicd` receives only a non-secret public verification key. This
preserves local exact-body, scope, expiry, and replay checks without giving the
user service a signing secret.

Likely boundaries:

* Add a new versioned Music capability/signature type—not an ad hoc change to
  `CloudArmedToken`—under `crates/mesh/mackes-mesh-types/src/cloud.rs` or a
  dedicated shared auth module.
* Update the root producer in
  `crates/mesh/mackesd/src/ipc/action_auth.rs` and
  `crates/desktop/mde-shell-egui/src/iac/mod.rs` to sign with a private,
  root-only credential.
* Update `crates/services/mde-musicd/src/bus_responder.rs` to verify the public
  key and retain the existing bounded body, exact scope, TTL, and nonce ledger.
* Package the public key as non-secret, revisioned data readable by the user
  daemon; keep private-key provisioning in a new root-only helper/drop-in
  adjacent to `install-helpers/provision-cloud-arm-credential.sh`, not in the
  Music user unit.

Required proof includes key-rotation/version skew, wrong public key, signature
tampering, replay, expiry, and a secret scan proving no private key reaches the
user package, Bus body, environment, catalog, or logs. This is a cryptographic
contract change and needs review before implementation.

### Option C — separate Music-only seat credential (simpler, lower assurance)

Issue a new credential with a new name and key material exclusively for Music
mutations. Deliver it through a user-readable, mode-0600 seat credential path
or a user-manager credential source that the selected seat user can actually
read. It must authorize only the closed `music-*` verbs and Music scopes; it
must not verify cloud, workload, firmware, or other root actions.

Likely boundaries:

* Add a distinct credential name and verifier contract in
  `crates/services/mde-musicd/src/bus_responder.rs` (or shared types), with
  explicit Music-only domain separation in the signed payload.
* Add a new, separately reviewed provisioning helper and user-unit drop-in;
  do not extend `install-helpers/provision-cloud-arm-credential.sh` or
  `packaging/systemd/cloud-arm-credential.conf`.
* Keep the existing root cloud signer and root drop-ins unchanged.
* Add package tests for ownership/mode, missing credential, rotation, scope
  confusion, replay, and no secret-bearing Bus/catalog output.

This option makes the seat user's trust boundary explicit: any process already
trusted as that seat user could potentially use the Music-only credential.
It is acceptable only if that is the intended authority. It is not an
acceptable way to make the existing root cloud capability available to
`mde-musicd`.

### Rejected delivery shortcuts

The following are not safe options and are intentionally not implemented:

* adding `LoadCredentialEncrypted=cloud-arm-key:...` to the user unit;
* copying `/etc/credstore.encrypted/cloud-arm-key` or decrypted key bytes into
  a user home, `/run/user`, an environment variable, or the Bus;
* allowing unsigned Music mutations because the daemon is “local”;
* reusing `airsonic-creds.json` as a signing key;
* running the whole Music daemon as root merely to inherit the cloud credential.

## Proposed next-step sequence

1. Choose Option A, B, or C with an explicit seat-user threat model and owner.
2. Freeze a Music-only capability contract: version, verbs, targets, exact-body
   digest, expiry, nonce/replay store, key rotation, and audit fields.
3. Implement the smallest producer, verifier, and package changes within the
   boundaries above; preserve unsigned/refusal behavior while the credential is
   unavailable.
4. Add focused hostile tests before any farm gate: no credential, wrong scope,
   body tamper, expiry, replay, rotation, and credential-path permissions.
5. Run package/unit verification and a non-production seat proof covering one
   authorized mutation plus the complete refusal matrix. Record source hashes
   and never include key contents.

## Tiny checks and evidence commands

The audit used repository-only inspection commands such as:

```sh
rg -n '(cloud-arm|CREDENTIALS_DIRECTORY|LoadCredential|authorize_root_mutation_body|music_action_auth)' \
  crates packaging install-helpers
sed -n '1,80p' packaging/systemd/mde-musicd.service
sed -n '40,220p' crates/services/mde-musicd/src/bus_responder.rs
sed -n '90,245p' crates/desktop/mde-shell-egui/src/iac/mod.rs
sed -n '1,150p' install-helpers/provision-cloud-arm-credential.sh
```

The only post-edit local checks for this design record were:

```sh
bash -n install-helpers/provision-cloud-arm-credential.sh
git diff --check
```

No worklist edit, service restart, credential provisioning, secret readout,
farm build, or remote deployment was performed for this audit.
