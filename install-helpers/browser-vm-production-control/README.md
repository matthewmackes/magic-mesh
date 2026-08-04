# Browser VM production control hooks

This standalone crate supplies the two trusted executables required by
`collect-browser-vm-live-audio.sh` and the guest controller they depend on:

- `browser-vm-guest-audio-probe-hook` drives Chromium exclusively through the
  public `mde-vdi-rdp` connection, framebuffer, and input APIs. It performs two
  real RDP clicks per measured operation: one to arm WebAudio/getUserMedia and
  one, after the collector's start signal, to start the measured operation. A
  live collection keeps one authenticated RDP transport for the phase's
  playback and capture jobs, replaces the current tab through Chromium's
  proven omnibox shortcut for each job, and pumps the bounded controller
  handoff rather than racing a second xrdp attachment.
- `browser-vm-reconnect-hook` gracefully closes a real RDP transport, creates a
  new TLS/CredSSP transport to the same endpoint, and requires both at least
  700 per mille unique inbound repaint coverage and 850 per mille visual
  identity before writing the collector receipt.
- `browser-vm-guest-audio-probe-controller` serves a one-shot page on the guest
  loopback origin. Only that Chromium page can upload capture PCM. The service
  has no oscillator, sample generator, receipt writer, process launcher, or
  guest-session impersonation path. Chromium transport re-fetches are admitted
  only while the job remains `registered`; the first `page_loaded` event closes
  the page endpoint so speculative fetches cannot weaken the job state machine.

The host/controller API is restricted to one configured hypervisor IP and uses
HMAC-SHA256 over method, path, timestamp, nonce, and body. Nonces are replay
protected. Browser endpoints additionally require a loopback peer, exact
loopback Host/Origin or Referrer, same-origin Fetch Metadata, a Chromium user
agent, and the one-time 256-bit job id.

## Build

Build and test this nested crate on the build farm:

```text
cargo test --locked --manifest-path install-helpers/browser-vm-production-control/Cargo.toml
cargo build --release --locked --manifest-path install-helpers/browser-vm-production-control/Cargo.toml
```

No repo-root workspace edit is required. The nested `Cargo.lock` is its release
resolution.

## Host deployment

Install the two host binaries as root-owned, non-symlink executables (mode
`0755`), for example under `/usr/libexec/mackesd/`. Install
`host-config.example.json` as
`/etc/mcnf/browser-vm-production-control.json`, replacing addresses and paths.
The config must be root-owned and not group/world writable. Its password and
controller-secret files must be root-owned mode `0600`; the secret is exactly
32 random bytes encoded as 64 hexadecimal characters. The RDP and controller
addresses must name the same guest.

Pass the installed binaries directly to the collector:

```text
--guest-probe-hook /usr/libexec/mackesd/browser-vm-guest-audio-probe-hook
--reconnect-hook /usr/libexec/mackesd/browser-vm-reconnect-hook
```

The hook deliberately suppresses the optional RDPSND client probe and requires
the typed `NoHostPlaybackSink` result. This keeps measured playback/capture on
the exact QEMU virtio-audio streams that the collector binds by QEMU PID.

## Guest deployment

Install the controller binary at
`/usr/libexec/mcnf/browser-vm-guest-audio-probe-controller`, install
`controller-config.example.json` at
`/etc/mcnf/browser-vm-guest-audio-probe-controller.json`, and install/enable the
provided service unit. Use a dedicated unprivileged `mcnf-browser-probe`
account. Its config and 64-hex-character secret file must be owned by that
account and not group/world writable. The secret must match the host copy.

Permit TCP port `38443` only from the configured hypervisor-side address. The
service independently rejects every other non-loopback peer.

Chromium must be managed to pre-authorize microphone access only for
`http://127.0.0.1:38443/*`. This removes permission-dialog automation; it does
not remove the page's two mandatory trusted user activations. The guest must
provide a live stereo 48 kHz capture endpoint. A mono endpoint fails closed.

## Explicit boundaries

- The production control channel implemented here is RDP. It rejects a
  `sunshine` transport rather than mislabeling it `rdp-webaudio`.
- QGA remains outside browser control. The collector may still use QGA for its
  existing ping and immutable provenance reads.
- The hook validates digital PCM only. It does not claim physical speaker
  audibility; the collector validator retains the operator-confirmation gate.
- The immutable Browser image builds the guest-only package, installs the
  service and loopback-only Chromium policy, and deliberately leaves the
  matching controller secret to runtime provisioning.
