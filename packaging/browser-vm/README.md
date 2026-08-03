# Browser VM guest profile and image

`Containerfile`, `build-image.sh`, and `verify-image.sh` now define the actual
Fedora 44 guest-image lane: Chromium, Sway/Wayland, Mesa virtual-GPU support,
PipeWire/WirePlumber, libinput, and the image-owned runtime are installed in a
dedicated `browser-vm-chromium` image. The image verifier is a static contents
gate; it does not claim that a VM has booted or that a VDI endpoint is live.

Build a signed/recorded ext4-rootfs disk artifact on the build farm with the Fedora-44
`magic-mesh-lighthouse` guest RPM, then set `browser_base_image_source` to the resulting qcow2 and
pass the resulting image digest in the typed `browser-provision` request. A
missing or malformed digest is refused before desired state is written.

The checked-in `deploy-image.sh` is the bounded operator path for a direct KVM
host. `preflight` verifies the local qcow2 and remote KVM/qemu-img/passwordless
sudo prerequisites without changing files. `publish` is dry-run by default;
only `publish --apply` uploads the image, backs up an existing regular image,
atomically installs the new root-owned image, and verifies the remote SHA-256.
It accepts no guest password, token, or credential.

`profile.env` is the small, reviewable contract at the Construct/Browser VM
boundary. It identifies the guest image and immutable source provenance
(repository, path, and pinned commit), fixes the Arch-008
baseline (4 vCPU, 8 GiB RAM, 64 GiB disk), and declares the implemented RDP
transport plus the retained SPICE compatibility path. The host is explicitly
forbidden from owning a Browser
engine; Chromium, browser chrome, page execution, media decode, and failures
remain inside the guest.

The guest control plane is deliberately the thin `magic-mesh-lighthouse`
package. It supplies `mackesd`, `meshctl`, Nebula join helpers, and guest
systemd units while avoiding the workstation `magic-mesh` RPM's host
multimedia/Samba ABI closure. The image never installs `magic-mesh-browser` or
the full workstation package. The local `--rpm` lane accepts only a
`magic-mesh-lighthouse-*.rpm` and installs it through `dnf`, so dependency
resolution remains enforced; `rpm --nodeps` is not a supported image path.

The profile verifier admits only a root-owned regular file: symlinked,
group/other-writable, and executable profile inputs are rejected before the
profile is parsed. This keeps a mutable or redirected profile from changing
the guest identity or source provenance after the caller selects it.

This file is a profile contract, not proof that a VM is running. The Fedora 44
image and qcow2 build lane is farm-verified, but image publication still needs
signed artifact evidence and live VDI acceptance. Run `verify-profile.sh` before handing the profile to an image or
Workloads adapter. The verifier parses the file as data, never sources it as
shell, and rejects missing/duplicate/unknown fields, weak resource values,
unsupported transport claims, host-engine names, missing provenance, or the
all-zero Git revision placeholder.

The default verifier mode is the deployed-input admission check and requires a
root-owned profile. Farm/source-tree contract gates use
`verify-profile.sh --source profile.env`, which keeps the same schema and mode
checks while allowing the non-root owner produced by a normal checkout; source
mode is not a deployment admission decision.

The guest runtime writes a guest-owned, mode-0600 bounded
`runtime-evidence.json` record with transport health, VA-API status, and
PipeWire endpoint counts. `audio_status=wired` is endpoint-wiring evidence
only; it does not prove audible Chromium playback, capture, or recovery.
Validate collected records with
`install-helpers/verify-browser-vm-runtime-evidence.py`.

The image also carries a fixed 64x64 VP8/Opus media fixture from the shared
media test corpus. Each guest session runs
`mcnf-browser-vm-media-probe`, which records bounded Chromium media-element
readiness and decoded/dropped-frame counters in `media-evidence.json`. Validate
that record with `install-helpers/verify-browser-vm-media-evidence.py`. This is
guest-local decode evidence only: it does not claim GPU hardware acceleration,
audible playback, VDI presentation, or reconnect recovery.

The guest launch boundary is equally fail-closed. Workloads writes only
`profile-id`, `image-id`, `image-digest`, `session-id`, and `transport` under
`/etc/mackesd/browser-vm`; the image runs `validate-runtime-inputs.sh` before
starting Sway or a VDI endpoint. It requires the admitted Browser VM identity,
a 64-byte hexadecimal SHA-256 image reference, a bounded identity token, and
`rdp` or `spice`. Extra files, symlinks, commands, URLs, paths, and shell
syntax are rejected. No launch command, host-browser fallback, or
host-supplied lifecycle state is accepted from the host. The profile explicitly
declares `fail-closed` runtime behavior and `failed,unavailable` guest terminal
states; Workloads/session-broker owns publication of those states, and a guest
that cannot pass admission remains unavailable rather than starting a fallback.
The provisioning directory and every admitted record must be root-owned; the
directory may be traversable, but records may not be writable by group/other or
executable. This prevents a second process from replacing an identity after
the admission check while keeping the record format non-executable.

Run `verify-contract.sh` for the focused contract tests. These tests exercise
both the static profile and the guest launch admission boundary; they do not
claim that an image has been built or that a live guest is ready. The
Workloads/session-broker plane remains the single authority for placement,
capabilities, and lifecycle; the guest validator is only an admission check and
does not create a second control plane.

The contract also runs `verify-activation-contract.sh`. It binds
`Surface::Browser` to the typed `browser-vm` route, its implemented RDP/SPICE
VDI transports, the guest visual boundary, and the Workloads `DesktopVm`
delivery type. Sunshine/Moonlight remains unavailable until both a guest
endpoint and a host decoder exist; the shell presents that state instead of
substituting a different protocol. This is a source-seam guard, not live VDI
proof.

`browser-vm-transport-attach.schema.json` defines the minimal attach envelope
that the existing `state/vdi/console` shell mirror can consume without a
Browser helper crate. The envelope is an RDP brokered endpoint bound to the
Browser VM workload, Browser surface, session generation, and mesh-safe
`host:port`; it carries no ticket, credential, command, path, or URL. The
example and `verify-transport-attach.sh` keep the RDP/SPICE wire shape
fail-closed.

The declarative audio boundary can be checked with
`install-helpers/verify-browser-vm-audio.sh --domain <name>` (or an XML file).
It requires a virtio sound device, a PipeWire/PulseAudio backend, and both
playback and capture endpoints; it intentionally does not claim that live
audio is audible, captured, or recovered.

The source URL and path are deliberately recorded now so a later standalone
Browser-stack extraction can bind the guest profile to an immutable source
record rather than silently reusing a host image.
