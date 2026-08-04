# WL-ARCH-008 Dell r8 released-shell integration evidence — 2026-08-04

This is bounded evidence for the single active worklist in
`docs/platform/WORKLIST.md`; it is not a parallel tracker. It proves the released
shell's automatic Browser-to-Workloads-to-VDI path on Dell. It does not promote
WL-ARCH-008 or claim the still-open GPU, audio, performance, cleanup, or fleet
criteria.

## Exact released candidate

- Source and pushed branch head:
  `6f28404b71d37508172959182af081ef998ca48d`
- Isolated BigBoy build directory:
  `/home/mm/mcnf-shell-6f28404b.TW9HtI`
- Build command:
  `cargo build --release --locked -p mde-shell-egui --features drm,live-vdi,media-mpv`
- Focused regression command:
  `cargo test --release --locked -p mde-shell-egui --features drm,live-vdi,media-mpv browser_vm -- --nocapture`
- Regression result: 16 passed, 0 failed, 1,437 filtered out.
- Deployed binary SHA-256:
  `2a3569b0845663a38334768ffdba1a36eb7553380fdde0ae94d3f276e8f96cd3`
- Deployed size: 54,545,512 bytes.
- Rollback copy:
  `/var/lib/mackesd/shell-rollbacks/20260804T1028Z-6f28404b/mde-shell-egui.before`
- Prior binary SHA-256:
  `688e18804700aee634961ab80837820a0077b32afaf19a1ba353a03a3fb48325`

The exact artifact was copied from the isolated farm build. Dell's
`mde-shell-egui.service` returned active with `NRestarts=0`. Every deployment,
restart, proof-only configuration change, and restoration was preceded by the
centered red `AI-GENERATED-ALERT` and its enforced five-second wait.

## Automatic Browser handoff

The first routed readback correctly remained behind Dell's secure boot curtain.
It published no VDI request because the locked shell does not render application
bodies. This was not a Workloads or VM-health failure: Dell's fresh
`state/cloud/DELL-LAPTOP` mirror advertised `browser-vm` as `active` and
`reachable=true`, and the encrypted `browser-vm-rdp` systemd credential was
present in the shell's private credential directory.

The established proof-only fixture then set `require_login_at_boot:false` for
one restart, selected `MDE_DRM_PROOF_SURFACE=browser`, and requested a bounded
native readback. No application code or credential was changed. The released
shell completed the full typed path without operator input:

1. `action/vdi/session` open ULID
   `01KZ666XR2ZRVCMMR9ZF7BTR1V` created session
   `vdi-1785840498433-browser-vm` for serving peer `DELL-LAPTOP`, VM
   `browser-vm`, and client `DELL-LAPTOP`.
2. `state/vdi/console` ULID `01KZ666Z7HWF06WMVBW1E4YSA1` resolved that exact
   session as `brokered`, protocol `rdp`, endpoint `10.42.0.4:3389`.
3. `action/vdi/session` ULID `01KZ666ZJQZY3CKCNFZ3SH47Q9` advanced the same
   session to `active`.
4. The shell held an established TCP connection to the brokered RDP endpoint;
   Dell's bounded tunnel simultaneously held the guest leg to
   `192.168.122.58:3389`.

The accepted native readback is
[`WL-ARCH-008-2026-08-04-dell-r8-released-shell-chromium.png`](WL-ARCH-008-2026-08-04-dell-r8-released-shell-chromium.png),
SHA-256
`3c06faa9c5832a62b1cf60e0315cb4438d7296272d7b57b2a4870a54d02ac126`.
It is 1366x768 with 5,970 distinct colors and visibly contains guest-owned
Chromium inside the Construct Browser viewport while the Construct taskbar
remains available. The guest desktop retained a second Chromium diagnostic
view at the right edge, so this frame proves routing and live presentation, not
a pristine guest-session layout or final visual-polish signoff.

## Secure restoration

After capture, another five-second alert preceded restoration. The temporary
`/run/mde-bus/power-honor.json` fixture was removed, all three
`MDE_DRM_PROOF_*` manager variables were unset, and the shell was restarted.
Post-restore checks proved:

- `mde-shell-egui.service` active with `NRestarts=0`;
- no `MDE_DRM_PROOF_*` variable in the service process;
- no proof-only power-honor file, so secure `require_login_at_boot:true` is the
  fail-secure default again;
- the deployed binary still hashes to `2a3569b0…6cd3`;
- `browser-vm` remains running at `192.168.122.58`, and RDP port 3389 remains
  reachable.

## Bounded disposition

| Gate | Result |
| --- | --- |
| Exact pushed release build and focused Browser VM suite | Pass |
| Automatic Workloads selection and typed VDI open | Pass |
| Broker resolution, RDP connection, and session-active publication | Pass |
| Native released-shell Chromium framebuffer | Pass, bounded to routing/presentation |
| Five-second centered alert before every seat mutation | Pass |
| Secure boot-lock and proof-environment restoration | Pass |
| Guest hardware video decode | Open; r7 virgl candidate was rolled back |
| Sample-backed playback/capture/reconnect | Open; controller is not in the immutable image |
| Five-tab 1080p and shell-latency acceptance | Open |
| Surface and six-node fleet rollout | Open; those seats are unreachable |

This closes the released-shell auto-selection gap recorded by r6. R1 and
WL-ARCH-008 remain active for the explicitly open rows above.
