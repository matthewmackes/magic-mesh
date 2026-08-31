# magic-mesh-v13.0.0

Tag plan: `magic-mesh-v13.0.0`. Version: `13.0.0`. Fedora 44, x86_64.
This is the versioned release-note source for `WL-REL-001` S4. It is **not**
the final freeze, a public publication, or a live-seat pass.

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
- Surface `packaging/surface/surface-stack.f44.json` `bootc_base` stays
  null while that stack is blocked. Do not guess a digest.
- First-release input preflight passed at `42035dcbd` /
  `1788153988` against already-selected dests. Recording later evidence
  moves HEAD; final freeze still requires dest-cut reconfirmation of one
  unchanged revision.

## Upgrade path

Install from the six-role set bound to the frozen revision. Typed
`mackesd` verbs own placement, auth, and audit. Remote execution is signed
job bundles only — no raw shell. Recovery is corrected-forward.

`CHANGELOG.md` `[Unreleased]` still describes the 12.0.x Construct-wave
history since `magic-mesh-v12.0.0` and is not the `13.0.0` production
claim set.
