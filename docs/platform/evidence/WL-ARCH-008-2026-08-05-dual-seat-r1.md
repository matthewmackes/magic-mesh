# WL-ARCH-008 — dual-seat Browser VM and Android test lane (2026-08-05)

This evidence records the live test-seat state after the operator enabled VM
support on seat 15. It is evidence for the canonical worklist, not a second
tracker. The Browser VM performance gate remains open and rejected.

## Shared test identity

Both seats use the immutable Browser VM base with source commit
`af3348bcfa350c6e2ed0d4f283e3e8d7da4c9ba6` and image digest
`sha256:6c693432dbf23ae6ce931445dbaea9704d84e319b5e31750da85101950ad232a`.
Both run the same hardened localhost PipeWire-Pulse endpoint contract at
`127.0.0.1:4713`. The endpoint unit and helper were verified locally, then
deployed to each seat only after the centered red `AI-GENERATED-ALERT` warning
and its five-second delay.

| Seat | Host and fixed link | VM/KVM state | Guest proof | Audio proof |
| --- | --- | --- | --- | --- |
| 15 | `172.20.0.15`, `eno1` | `/dev/kvm` present; `browser-vm` running; base `root:qemu` mode `0440`; direct writable overlay | QGA responds; guest `192.168.122.139`; RDP `3389` accepts connections | `mcnf-qemu-pulse-endpoint.service` active; health reports `healthy user=mm address=127.0.0.1 port=4713` |
| T480 | `172.20.146.68`, `enp0s31f6` | `/dev/kvm` present; `browser-vm` running | QGA responds; guest `192.168.122.225`; existing RDP/virgl path | Same endpoint unit fixed and active; health reports the exact loopback endpoint |

Seat 15 also passed a schema-v2 deployment receipt probe: the running domain
uses one writable qcow2 overlay backed directly by the immutable base, and the
remote base digest matches the expected digest. No image promotion or fleet
deployment was performed.

## Bounded live Chrome preflight

The current producer and the tested RDP observer ran the same five-tab
Chromium setup on both seats. These are diagnostic preflights, not the
905-second acceptance run; the runner emitted `acceptance_eligible=false`,
`acceptance_status=not-run`, and `acceptance_gate_seconds=905.0` for both.

| Seat | Setup probe | Visible quality-presented FPS | Visible RVFC FPS | Geometry | Background tabs | Result |
| --- | ---: | ---: | ---: | --- | --- | --- |
| 15 | 4.104 s | 9.502 | 6.578 | ready | 4/4 progressed | rejected; visible floor is 27 FPS |
| T480 | 4.458 s | 14.582 | 3.589 | ready | 4/4 progressed | rejected; visible floor is 27 FPS |

The 15 artifact is
`/var/lib/mcnf-browser-vm/seat15-r1-preflight.ndjson`; the T480 artifact is
`/var/lib/mcnf-browser-vm/t480-r16-preflight.ndjson`. Both are private
`diagnostic-failure` records. The sidecars are empty because setup failed
before the sampling phase. No production audio audibility or 905-second
acceptance claim is made.

## Android test lane

15 and T480 are now the named dual-seat presentation lane for Android and
Chrome VM testing: either seat can exercise the VDI display, input, and audio
client path against a capable Android provider host, while both are also
available as local KVM VM test hosts.

The Android provider/Cuttlefish implementation is not yet a live guest proof.
The bounded provider contract and farm tests pass, but there is still no
verified Cuttlefish image boot, nested-KVM lifecycle, ADB inventory, guest
display session, or Android app launch on either physical seat. Seat 15's
current free capacity is sufficient for the staged Browser VM but is not a
claim that a Cuttlefish image can be placed locally. Android guest placement
must wait for the image size/capacity gate; until then, use these two seats as
the presentation/client acceptance pair.

The adapter now exposes a typed starter-app launch handoff. It rejects a
launch while the retained observation is not guest-ready and passes only the
admitted VM target plus closed starter-app request to a configured backend.
The BigBoy focused farm run passed 10/10 Android/Cuttlefish tests; this proves
the dispatch gate, not a live Cuttlefish guest.

The Android placement side now has a separate readiness gate: it validates the
bounded Cuttlefish base-image qcow2, the exact nine-app image manifest, KVM,
libvirt pool/network, and a fresh nested-host `cvd`/`adb` tooling receipt. A
successful result is `ready_for_provisioning` only; the report always retains
`live_android_guest_proof=unavailable` until the guest actually boots and
publishes package/display/session evidence. Its farm self-tests and the
existing manifest verifier both pass.

The read-only `verify-workloads-live-proof.py` run on both hosts returned the
required `libvirt/KVM` check as `ok`, with `/dev/kvm` a character device,
libvirt's `default` network and `mde-vms` pool active, and `browser-vm` listed.
The T480 report also retained unrelated non-required warnings for its cloud
mirror and inactive Podman socket; those do not change the VM seat result.

## Related boot work

The full `mackesd` Rust gate completed on BigBoy `.130` with `4398 passed,
0 failed, 1 ignored`, plus the binary and integration test groups. The
current cloud-credential packaging gate also passed `11/11` on BigBoy `.130`
in slot `wl-dual-seat-cloud-gate-r2`. The boot-latency slices are now
source-verified: SELinux policy loading is
fingerprinted and idempotent, and cloud credential materialization is
post-start/asynchronous so it no longer gates `mackesd`. Live reboot timing
must still be remeasured after the next package rollout; the old Dell/T480
measurements remain historical evidence.
