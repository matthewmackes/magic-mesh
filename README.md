# Magic Mesh

**A secure, no-fixed-center workgroup mesh — and its native-Rust Carbon GUIs —
on stock Fedora-Cosmic.**

Magic Mesh is the mesh fabric and its applications, split out of the
[MackesWorkstation](https://github.com/matthewmackes/MackesWorkstation) monorepo
(the labwc / Windows-era *MackesDE* desktop, now end-of-life) by the **E11 "Magic
Mesh" pivot**. The desktop shell is gone — **Cosmic provides the desktop**
(panel, notifications, settings, window management, lock, greeter). What carries
forward is everything Cosmic *doesn't* give you: the encrypted overlay mesh, the
fleet automation, mesh storage, telephony, KDE-Connect, and the file manager —
shipped as Cosmic apps + applets with a strict IBM-Carbon identity.

## What's here

| Group | Crates | Role |
|---|---|---|
| `platform` | `mde-bus`, `mde-role` | the internal pub/sub backbone + deployment-role gating |
| `mesh` | `mackesd`, `mackes-{config,mesh-types,nebula-https-tunnel,transport}`, `magic-fleet` | the supervised control-plane daemon, Nebula overlay, transport/types/config, and the **Automation Mesh** node engine |
| `services` | `mde-files`, `mde-voice-{hud,config}`, `mde-music`, `mde-musicd` | the file manager, voice/SIP HUD + config, music player + daemon |
| `workbench` | `mde-workbench` | the Cosmic **control surface** (fleet, devices, maintain, mesh health) |
| `shared` | `mde-theme`, `mde-iced-components`, `mde-card`, `mde-disclaimer` | the iced-0.14 **Carbon** look stack |
| `kdc` | `mde-kdc-host`, `mde-kdc-proto` | the canonical KDE Connect host |

`salvage/from-mde-binary/` holds two surfaces (`birthright`, `mesh_status`)
salvaged from the deleted desktop binary, pending re-home onto Cosmic — see its
README.

## Architecture locks

The load-bearing identity (full detail in [`AI_GOVERNANCE.md`](AI_GOVERNANCE.md)):

- **Mesh:** Nebula encrypted overlay · **no fixed center** (any node authors
  fleet revisions; peers gossip them) · LizardFS mesh storage.
- **Bus, not D-Bus:** surfaces and `mackesd` talk over `mde-bus`; FDO interop
  (`org.freedesktop.*`) only.
- **Security:** maximum crypto — Ed25519 node identity, AES-256-GCM /
  ChaCha20-Poly1305, RSA-4096 KDC identity.
- **Look:** strictly **IBM Carbon** (carbondesignsystem.com), Gray 10 / 90 / 100
  themes, single-sourced in `mde-theme`. Pure-Rust stack (rustls, cosmic-text).
- **Boundary:** no mesh-side crate depends on a desktop-shell crate (the split is
  gated).

## Build

```sh
cargo build --workspace        # needs gtk3-devel + alsa-lib-devel (the audio chain)
cargo test
cargo clippy --all-targets
cargo fmt --all
```

GPL-3.0-or-later. See [`DISCLAIMER.md`](DISCLAIMER.md).
