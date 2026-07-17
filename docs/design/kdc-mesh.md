# KDC-MESH — KDE Connect over the Nebula mesh (mesh-aware phones)

Operator-locked 2026-07-04 (16-Q `/plan` survey). Make the platform's KDE Connect
integration (the existing `crates/kdc/` subsystem + the `kdc_host` worker + the SEC-5
mesh-shunt) run **over the Nebula overlay** so a phone — running the **Android Nebula
client + the KDE Connect app** — is a **full mesh member** with access to **all
services on all nodes**, not tied to any one node.

Three operator goals: **A** traffic travels the Nebula network; **B** all KDE Connect
features online; **C** mesh-aware — the phone reaches every node's services.

## What exists today (grounding)
- `crates/kdc/{mde-kdc-host, mde-kdc-proto}` — the host impl + wire protocol; RSA-4096
  device identity (H1), TOFU pairing + fingerprint-pin (SEC-4), a proptest'd codec.
- `workers/kdc_host.rs` — binds **TCP 1716** on **Workstation-rank** nodes (LAN), pairing
  store, `LanTransport`; `kdc_outbound` drains ring/sms/clipboard/share/phone-hub over the LAN.
- **SEC-5 mesh-shunt** — each peer publishes its paired phones to `<root>/kdc-phones/<host>.json`;
  `run_host` collects neighbors' files → `SyntheticAnnounce` → injects into the roster the
  desktops read. So phone *rosters* already relay mesh-wide — the **transport** is still LAN.
- `firewall_preset.rs` opens 1716 only where `kdc_host` runs (headless nodes keep it closed).

## Locked decisions (16)

| # | Area | Lock |
|---|------|------|
| 1 | Phone on mesh | **Full Nebula member** — the Android Nebula client enrolls the phone as a real peer (overlay IP + signed cert); KDE Connect rides the overlay peer-to-peer. |
| 2 | Discovery | **Mesh roster → directed announce** — reuse the mesh-shunt roster + peer directory; the phone learns every host's overlay IP, hosts directed-announce to the phone's overlay IP (no UDP broadcast, which Nebula doesn't carry). |
| 3 | Transport | **Nebula overlay ONLY** — all KDC traffic rides the overlay always (encrypted E2E by Nebula, one code path); no LAN direct even when co-located. |
| 4 | Pairing | **Pair once, over the overlay** — the SEC-4 TOFU + fingerprint-pin flow reaching a host by overlay IP. |
| 5 | Identity | **One mesh-wide identity, paired once** — the phone is a single trusted device to the WHOLE mesh; every node recognizes it without re-pairing (the pairing record is shared mesh-wide). |
| 6 | Serving node | **All nodes simultaneously** — every node serves the phone at once (a phone notification appears on every desktop; clipboard syncs everywhere). |
| 7 | Any-node reach | **Mesh service directory, pick the node** — the phone can target ANY node's KDC services (files, run-commands) directly over the overlay via a mesh-wide service directory. |
| 8 | Presentation | **One 'Mesh' device + a node picker** — the mesh presents as a single "Quazar Mesh" device for the follow-everywhere features, with drill-in to individual nodes for node-specific actions. *(See the Android-constraint note.)* |
| 9 | Notifications | **Bidirectional, mesh-wide** — phone notifications appear on every desktop AND the mesh/system notify feed (CHAT-FIX-2) pushes to the phone; reply-from-desktop where supported. |
| 10 | Remote/media | **Both** — the phone is a touchpad + keyboard for the active desktop (remote input) AND controls media (play/pause/next/vol) + presenter on whichever node plays. |
| 11 | Files | **Both directions, any node** — send/receive (done) + browse the phone's FS from a desktop (SFTP) + browse any node's shared files from the phone (via the Q7 directory). |
| 12 | More features | **All** — run-commands (incl. **a series of OpenStack lifecycle commands across all nodes** — start/stop/reboot/etc. from the phone), battery + connectivity report, telephony (call/SMS alerts), find-my-devices (both ways). |
| 13 | Phone hub | **A 'Phones' hub in the shell** — lists paired phone(s), mesh identity, battery/signal, per-feature toggles, the file browser, the run-command editor (incl. the OpenStack set), and pairing. Folds in the existing phone-hub/kdc pieces (§6). |
| 14 | Pairing UX | **QR from the shell, pair-to-mesh** — the hub shows a QR encoding the mesh enroll + a KDC pairing token; one phone scan enrolls onto Nebula AND pairs to the mesh. |
| 15 | Which nodes | **Every node (universal, rank-0)** — `kdc_host` runs on ALL nodes (lighthouses + headless included) so any-node + all-nodes-simultaneously actually work. Overlay-only transport means NO public port opens (it binds the overlay iface, not the public NIC). |
| 16 | Security | **Pairing is enough** — a paired, mesh-enrolled phone can trigger anything (the Nebula cert + the KDC pairing IS the authorization; no per-command confirm). Every action is still recorded in the KDC hash-chained audit log (`events::append_event`). |

## The Android-side constraint (key architectural note)
The phone runs the **stock** Android Nebula client + the **stock** KDE Connect Android app
(both out of this repo). We control only the **host/mesh side**. Stock KDE Connect lists
each host as a separate device. So **lock #8 ("one Mesh device")** is realized host-side:
- Every node runs `kdc_host` sharing ONE mesh-wide pairing (#5/#15); to stock KDE Connect
  they appear as multiple devices that all trust the phone.
- A designated **mesh endpoint** (the follow-the-user node, else a stable primary)
  advertises as **"Quazar Mesh"** — the single device the user interacts with for the
  follow-everywhere features (#6/#9/#10); a mackesd **mesh-fanout** relays its actions to
  all nodes and aggregates their responses. Node-specific actions (#7) target a node by
  its own overlay identity via the service directory / the desktop Phones hub.
- A first-class "one Mesh device" UX end-to-end would need a mesh-aware KDE Connect Android
  fork — **out of scope v1**; the host-side fanout gives the experience with stock apps.

## Architecture (host/mesh side)
- **Overlay transport** (`crates/kdc/mde-kdc-host`): a new `OverlayTransport` replacing/
  beside `LanTransport` — bind the node's **Nebula overlay IP** on 1716, dial peers/phones
  by overlay IP; overlay-only (#3). The `kdc_outbound` drainer sends over it.
- **Directed discovery** (`kdc_host` worker): consume the mesh-shunt roster + peer directory
  to learn the phone's overlay IP; directed-announce hosts↔phone (#2). No broadcast.
- **Mesh-wide pairing** (`PairingStore` + a shared record): one pairing, replicated on the
  substrate so every node's `kdc_host` recognizes the phone (#5); QR encodes enroll + pair
  token (#14).
- **Universal role** (`worker_role.rs`): `kdc_host` → **rank 0** (every node, #15); the
  firewall preset opens 1716 on the **overlay iface only**, never the public NIC.
- **Mesh service directory**: each node publishes its KDC service set (files, run-commands,
  media) to the substrate; the phone (via the mesh endpoint) + the desktop hub browse it and
  target any node (#7).
- **Feature workers** (complete the set, §7 each): bidirectional notifications wired into the
  CHAT-FIX-2 notify producer (#9); remote input via the shell's evdev/input injection (#10);
  media control via the media surface transport seam (#10); SFTP both ways (#11);
  run-commands incl. the OpenStack lifecycle set driving the QC `state/openstack` verbs (#12);
  battery/connectivity/telephony/find-my-devices (#12) — all over the overlay transport, all
  audited (`events::append_event`, #16).
- **Phones hub surface** (`mde-shell-egui`): the desktop-side management surface (#13).

## Acceptance (runtime-observable; per-task in the worklist)
- A phone running the Android Nebula client + KDE Connect, enrolled via the hub QR, pairs
  ONCE and is recognized by every node; all KDC traffic rides the overlay (verified: 1716
  bound on the overlay IP, nothing on the public NIC).
- A phone notification appears on every desktop; the mesh notify feed reaches the phone;
  clipboard/media/remote-input work follow-everywhere; the phone browses any node's files
  and triggers run-commands (incl. an OpenStack instance reboot) on a chosen node.
- `kdc_host` runs on every node (census rank-0); the public boundary stays default-deny
  (overlay-only bind — asserted).
- Every phone-triggered action lands in the hash-chained audit log.

## Risks
- **Stock Android apps** (above) — the "one Mesh device" is a host-side fanout, not a true
  single device; a fork is the only way to a fully-unified Android UX (out of scope).
- **All-nodes-simultaneously noise** — a notification on every desktop can be duplicative;
  needs de-dup/coalescing so one phone notification isn't N desktop toasts unpleasantly.
- **"Pairing is enough" blast radius** (#16) — a paired phone can drive OpenStack lifecycle
  across the fleet with no per-command confirm; the audit log is the only brake. Flag this
  posture in the hub; a lost/stolen paired phone = fleet control until unpaired (make unpair
  fast + mesh-wide).
- **Overlay-only + big file transfers** — SFTP/large shares over the overlay (no LAN
  shortcut) may be slower on the same segment; acceptable per #3, note it.
- **Mesh-wide pairing replication** — the shared pairing record must converge before a new
  node trusts the phone; honest-gate a node that hasn't synced it yet.

## Out of scope (v1)
- A mesh-aware KDE Connect Android fork (stock apps + host fanout only).
- Per-command arming / per-node RBAC for the phone (pairing-is-enough, #16).
- Non-Nebula transports (overlay-only, #3).

## Tasks → `docs/WORKLIST.md` KDC-MESH-1..N.
