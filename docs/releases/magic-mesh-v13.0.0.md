# magic-mesh-v13.0.0

Tag plan: `magic-mesh-v13.0.0`. Version: `13.0.0`. Fedora 44, x86_64.
Frozen source: `42035dcbd76b03b8323399892052b21a96e2e233` epoch
`1788153988` on protected `master`. This is the `WL-REL-001` S4 note
source for that revision. It is not a public publication, tag, or
live-seat pass. Drain-branch documentation after the freeze SHA is not
the freeze tree.

## What ships

Construct is one egui DRM thin-client shell. Native apps run in governed VMs
or approved collaboration/media surfaces. There is no host compositor and no
host browser engine.

The production set is exactly six roles:

- Workstation RPM
- Server RPM
- Lighthouse RPM
- Browser VM
- App VM
- bootc image

Visible interfaces: **Construct** and **Car**. Overlay transport is Nebula.
Coordination is etcd over Nebula; files sync on Syncthing. `mackesd` is the
only daemon authority; `mde-bus` is the only platform bus.

## Compatibility

- Target OS/arch: Fedora 44 on x86_64. ARM64 is not in this envelope.
- Android / Cuttlefish is **Deferred** beyond `13.0.0`. It is not a role,
  input, or acceptance gate. Lifecycle view renders Android as `Deferred`.
- Upgrade is corrected-forward. Do not roll back a failed seat mutation.

## Known limitations

- Live-seat, provider, and operator-testing leftovers execute on
  `WL-TEST-003` only after a testing Beta. Dest-cut
  `bc14a22d7` (`13.0.0-35` / lighthouse `13.0.0-11`) is not that Beta.
- Surface `packaging/surface/surface-stack.f44.json` stays blocked
  (unsigned Surface RPMs). The selected bootc dest pin is private
  `quay.io/fedora/fedora-bootc@sha256:3a5e74e6…`; in-tree `bootc_base`
  remains null.
- `github-required` is now a required check on `master`. The freeze-SHA
  workflow was dispatched and has not yet passed.

## Upgrade path

Install from the six-role set bound to the frozen revision. Typed
`mackesd` verbs own placement, auth, and audit. Remote execution is signed
job bundles only — no raw shell. Recovery is corrected-forward.

`CHANGELOG.md` `[Unreleased]` still describes the 12.0.x Construct-wave
history since `magic-mesh-v12.0.0` and is not the `13.0.0` production
claim set.
