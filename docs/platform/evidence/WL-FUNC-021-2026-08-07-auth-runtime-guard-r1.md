# WL-FUNC-021 — Music credential retrieval fail-closed guard (2026-08-07)

## Finding

Dell `.225` has the release-5 Music credential provisioner and an enabled
one-shot unit, but `/etc/mde/music-action-public-key` and the encrypted Music
credential are absent. The shared secret name
`music/action-ed25519-seed` is present in the store, while the Dell age
identity cannot currently retrieve it reliably: bounded range requests to the
configured etcd endpoints intermittently timed out or returned an empty body.

The old provisioner treated every secret-helper failure as “absent” when
`--init` was supplied. That could replace an existing but temporarily
unreadable mesh secret. `install-helpers/provision-music-action-credential.sh`
now initializes only when the secret helper returns its documented exit code
3 for an absent secret; transport, decryption, and other retrieval failures
refuse initialization.

## Runtime observation

- Dell package: `magic-mesh-12.1.6-5.x86_64`.
- `mcnf-music-action-credential.service`: enabled, inactive, prior result
  `success`; generated Music credential paths remained absent.
- Health probes for `http://10.42.0.1:2379` and `http://10.42.0.2:2379`
  returned healthy, but key-range calls were intermittent.
- An authorized `--init --restart` attempt exited non-zero from the secret
  store JSON path; no credential/public-key files were materialized.
- A subsequent `--refresh --restart` correctly refused because the current
  installed helper could not retrieve the sealed seed. No further mutation was
  attempted.

## Verification

```text
bash -n install-helpers/provision-music-action-credential.sh       PASS
./install-helpers/provision-music-action-credential.sh --self-test  PASS
farm .50 music-auth-provision-guard-r1 syntax/self-test             PASS
```

## Post-provision addendum

After the bounded etcd read recovered, the already-authorized Dell mutation
was retried with the release-5 provisioner. It installed the host-bound public
key, encrypted private key, and shell environment drop-in; the derived public
key matched the installed public key without exposing the seed. `mde-musicd`
and `mde-shell` were restarted and returned active.

A bounded signed, idempotent `set_volume=1000` request was then submitted
through `mde-bus`. The request was rejected as typed `unauthorized`, so no
Music state mutation is claimed. A subsequent Dell network outage made a
second signature trace and rotation attempt unavailable. The fail-closed
guard remains required in the rebuilt RPM and live rotation remains open.
The outage was independently confirmed from seat 15: SSH and ICMP to Dell
`.225` both failed, so the review sync was not silently skipped by the
orchestrator alone.

Source audit found the likely rejection cause: the root shell signer in
`crates/desktop/mde-shell-egui/src/iac/mod.rs` preferred `$HOSTNAME`, while
`mde-musicd` canonicalizes its node with the `hostname` command. The signer now
uses that same command first, then `/etc/hostname`, then `$HOSTNAME`, so stale
user-manager environment cannot invalidate an otherwise correct capability.
The deterministic selector and shell-to-daemon signer contract tests passed
2/2 on BigBoy; the Fedora 44 release rebuild compiled this change and passed
both RPM payload gates.

The Fedora 44 farm rebuild completed with both RPM payload gates passing:

```text
magic-mesh-12.1.6-5.x86_64.rpm       83.5 MiB  PASS
magic-mesh-lighthouse-12.1.6-5.x86_64.rpm  11.9 MiB  PASS
standard RPM SHA-256: 4c39aa35e11944a7914b5758189f142b0e4afdeabc2b2c6f8fe63a351715aaad
lighthouse RPM SHA-256: 9716a6891e124cc14a24d3c74248d44eb8f5a2672fd5102b7d8d0c65f2c373e4
```
