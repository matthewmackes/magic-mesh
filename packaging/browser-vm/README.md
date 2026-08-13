# Browser VM guest profile and image

`Containerfile`, `build-image.sh`, and `verify-image.sh` now define the actual
Fedora 44 guest-image lane: Chromium, Sway/Wayland, Mesa virtual-GPU support,
PipeWire/WirePlumber, libinput, and the image-owned runtime are installed in a
dedicated `browser-vm-chromium` image. The image verifier is a static contents
gate; it does not claim that a VM has booted or that a VDI endpoint is live.

Build a signed/recorded ext4-rootfs disk artifact on the build farm with exactly
one Fedora-44 `magic-mesh-lighthouse` guest RPM passed through `--rpm`. The
builder refuses the mutable repository-latest lane, hashes the selected regular
non-symlink RPM before the container build, re-attests those exact bytes before
`dnf` installation, and records the digest in both the OCI label and immutable
guest metadata. Publish the resulting disk into the promoted mesh image
catalog, then use its `browser-vm-chromium:VERSION` reference with
`request-browser-vm-workload`. The helper sends exactly one typed
`StartAndAttach` request to `action/workload/operation`; WorkloadCompute owns
domain lifecycle and the authenticated QEMU Display1 lease. A Browser launcher
must never publish the retired `browser-provision` action or connect directly
to a SPICE endpoint.

When `--disk qcow2` or `--disk raw` is requested, the image builder binds the
output virtual size to `BROWSER_VM_DISK_GB` from `profile.env` (currently 64
GiB). Smaller bootc-image-builder defaults are enlarged before publication;
`deploy-image.sh` and the seat preflight reject smaller artifacts.

Every promotable qcow2/raw output now has one canonical sidecar named
`<artifact>.mcnf-manifest.json`. The bounded schema binds the complete artifact
bytes and format, the established `browser-vm-chromium-v1` image version, the
exact profile bytes/resources/source revision, and the fixed runtime source
assets copied or compiled into the guest. `verify-image.sh --artifact IMAGE
MANIFEST` is the Workload-admission entrypoint; it also invokes the profile
contract, so callers do not infer identity from a filename or maintain a second
allowlist. The verifier rejects noncanonical sidecar names, symlinks, malformed
or oversized JSON, duplicate/unknown fields, truncation, stale profile/runtime
digests, image-byte mismatch, and qcow2/raw virtual sizes other than exactly 64
GiB. An OCI container built without `--disk` is an intermediate build input,
not an admissible Browser VM artifact.

`promote-catalog-image.py` is the bounded offline import path for an already
admitted Browser VM pair when the live armed-token image promotion service is
unavailable. It accepts only an absolute, previously nonexistent catalog root,
re-runs the complete artifact/profile verifier and `qemu-img check`, preserves
the original artifact and identity-manifest names, and atomically publishes the
canonical `manifest.toml`, `image.sha256`, `<name>.img`, and `PROMOTED` layout.
The Workload artifact name is a hard link to the admitted qcow2 bytes, not a
conversion. This helper is therefore suitable for an isolated/offline catalog;
it deliberately refuses to update or replace an existing production catalog.

The checked-in `deploy-image.sh` is the bounded operator path for a direct KVM
host. `preflight` verifies the local qcow2 and remote KVM/qemu-img/passwordless
sudo prerequisites, including a resolvable remote `qemu` group, without
changing files. `publish` is dry-run by default; only `publish --apply` uploads
the immutable base, backs up an existing regular image, atomically installs the
new base as `root:qemu` mode `0440`, restores its libvirt SELinux label when
`restorecon` is available, and verifies the remote SHA-256 after those metadata
changes. It accepts no guest password, token, or credential. Publication does
not attach the base directly: the domain must use a separate writable qcow2
overlay whose absolute immediate backing filename is the published base.

Existing Browser VMs are migrated with
`migrate-display1-domain.sh apply --target HOST`. The migration backs up the
inactive libvirt XML, preserves every guest disk/source unchanged, adds QEMU
Display1 and virtio video, and retains SPICE only as a loopback recovery
transport. If the guest is running, it requests a normal shutdown and exits
without changing the definition when that shutdown does not complete; it never
force-destroys a live VM. The converted guest remains stopped, and the next
Browser launch uses the canonical `StartAndAttach` path.

`profile.env` is the small, reviewable contract at the Construct/Browser VM
boundary. It identifies the guest image and immutable source provenance
(repository, path, and pinned commit), fixes the Arch-008
baseline (4 vCPU, 8 GiB RAM, 64 GiB disk), and declares the implemented RDP
transport plus the retained SPICE compatibility path. The host is explicitly
forbidden from owning a Browser
engine; Chromium, browser chrome, page execution, media decode, and failures
remain inside the guest.

Sunshine/Moonlight is still unavailable: the image contains no Sunshine guest
endpoint and Construct has no admitted decoder path. SPICE remains an explicit
compatibility/recovery transport, not a claim that the requested Sunshine
alternate has shipped. S4 live closure therefore still requires a promoted
manifested image plus guest/host Sunshine implementation and live VDI evidence.

The guest control plane is deliberately the thin `magic-mesh-lighthouse`
package. It supplies `mackesd`, `meshctl`, Nebula join helpers, and guest
systemd units while avoiding the workstation `magic-mesh` RPM's host
multimedia/Samba ABI closure. The image never installs `magic-mesh-browser` or
the full workstation package. The mandatory `--rpm` lane accepts exactly one
package whose RPM name is `magic-mesh-lighthouse` and installs it through
`dnf`, so dependency resolution remains enforced; repository-latest resolution
and `rpm --nodeps` are not supported image paths.

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
Each bootstrap invalidates prior runtime, media, GPU, and PipeWire evidence
before admission, so malformed provenance or runtime input cannot leave an old
`wired` record available as evidence for the failed attempt.
Validate collected records with
`install-helpers/verify-browser-vm-runtime-evidence.py`.

The image also carries a fixed 64x64 VP8/Opus media fixture from the shared
media test corpus. Each guest session runs
`mcnf-browser-vm-media-probe`, which records bounded Chromium media-element
readiness and decoded/dropped-frame counters in `media-evidence.json`. Validate
that record with `install-helpers/verify-browser-vm-media-evidence.py`. This is
guest-local decode evidence only: it does not claim GPU hardware acceleration,
audible playback, VDI presentation, or reconnect recovery.

The immutable image also builds and installs the guest half of the production
audio qualification control plane. The controller runs as the dedicated
`mcnf-browser-probe` account, is enabled but condition-gated until a separately
provisioned controller secret exists, and admits traffic only from loopback or
the fixed hypervisor-side `192.168.122.1/32` address at both the systemd IP ACL
and application-authentication layers. The image contains no controller
secret. Provisioning must create the 64-hex-character secret as
`mcnf-browser-probe:mcnf-browser-probe` mode `0400`, then start the service;
the matching host copy remains root-owned mode `0600`.

Chromium is managed with `AudioCaptureAllowed=false` plus one exception,
`http://127.0.0.1:38443/*`. This leaves microphone capture denied for every
other origin while removing the permission dialog for the authenticated local
probe page. The page still requires its two real trusted RDP clicks, and this
packaging state alone is not playback, capture, reconnect, or audibility proof.

The guest launch boundary is equally fail-closed. Workloads writes only
`profile-id`, `image-id`, `image-digest`, `session-id`, and `transport` under
`/etc/mcnf-browser-vm`; the image runs `validate-runtime-inputs.sh` before
starting Sway or a VDI endpoint. It requires the admitted Browser VM identity,
a 64-byte hexadecimal SHA-256 image reference, a bounded identity token, and
`rdp` or `spice`. Extra files, symlinks, commands, URLs, paths, and shell
syntax are rejected. No launch command, host-browser fallback, or
host-supplied lifecycle state is accepted from the host. The profile explicitly
declares `fail-closed` runtime behavior and `failed,unavailable` guest terminal
states; Workloads/session-broker owns publication of those states, and a guest
that cannot pass admission remains unavailable rather than starting a fallback.
The dedicated provisioning directory and every admitted record must be
root-owned; the directory intentionally remains outside `/etc/mackesd`, whose
mode-0700 daemon boundary protects secrets from unprivileged traversal. The
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

Browser presentation is requested through the typed Workload `Open` or
`StartAndAttach` operation. The bounded `state/workloads/<node>` projection
carries an expiring node-local Display1 lease; no raw host, port, ticket,
credential, command, path, or URL is published as an attachment envelope.

The declarative audio boundary can be checked with
`install-helpers/verify-browser-vm-audio.sh --domain <name>` (or an XML file).
It requires exactly one virtio sound device and one Browser-owned PulseAudio
backend on `tcp:127.0.0.1:4713`, with exactly one playback and one capture
endpoint. Capture must use `streamName="MCNF-Browser-VM-Capture"` and playback
must use `streamName="MCNF-Browser-VM"`; neither node may carry a `name`
device selector. This routes both QEMU streams through the seat's default
Pulse source/sink while retaining exact stream identities, and rejects
duplicate, physical-device, or alternate routes. It intentionally does not
claim that live audio is audible, captured, or recovered.

For the Browser-specific OpenTofu domain, both compatibility and accelerated
overlays connect QEMU to `tcp:127.0.0.1:4713`. The base Workstation package
installs `mcnf-qemu-pulse-endpoint.service` into the selected seat user's
systemd manager; its helper admits exactly one PipeWire-Pulse module and one
loopback listener owned by that same seat user's sole PipeWire-Pulse process,
publishes watchdog health, and refuses ambiguous or broad listeners.
`verify-qemu-pulse-endpoint` checks the packaged source/unit
contract, while `mcnf-qemu-pulse-endpoint --health` checks the live graph. A
healthy endpoint is wiring evidence only; physical audibility remains part of
the live acceptance boundary.

Live performance acceptance records are validated with
`install-helpers/verify-browser-vm-performance.py`. A passing record must be
bound to the source commit and image digest and must cover five concurrent
1080p tabs for at least 15 minutes, minimum 30 FPS, no stall over 500 ms,
pointer activity, navigation/session latency, partial uploads, hidden repaint,
and reconnect recovery. The verifier never creates a local or farm-only pass;
Dell/seat evidence is still required.

The final Browser VM promotion boundary is
`install-helpers/verify-browser-vm-live-acceptance.py`. It binds observed VDI
frame/input/reconnect, connected guest runtime with GPU readiness, Chromium
decode, performance, and sample-backed playback/capture audio to one exact
source commit and image digest. It rejects endpoint-only audio, guest-local
media alone, missing reconnect/input observations, stale or symlinked artifacts,
and credential-shaped fields. Its self-test validates the boundary; it does not
claim Dell or seat-15 readiness until a real bundle is collected.

The underlying `install-helpers/verify-vdi-live-proof.py` runner requires both
`--source-commit` and `--image-digest sha256:<64-hex>` on every live probe. A
VDI frame marker without those immutable artifact bindings is rejected and
cannot enter the composite acceptance bundle.

After the base is installed, create one writable qcow2 overlay with an absolute
backing filename, attach that overlay as the domain's sole `vda`, and start the
`browser-vm` domain. Never attach the hashed base directly; guest writes would
invalidate its identity. Produce a deployment receipt with
`deploy-image.sh receipt`. Its schema-v2 live probe uses `qemu:///system` and
requires a running domain, exactly one file-backed `vda`, qcow2 format for both
files, and an exact two-file chain: writable `attached_disk` followed by
immutable `remote_image`. The overlay's immediate backing filename must be the
absolute base path, and the base must have no backing file. Relative,
alternate, duplicate, and deeper chains are rejected. A read-only `vda` or an
overlay that the resolved `qemu` account cannot read and write is also rejected.

The receipt records `remote_image`, `attached_disk`, `backing_image`, both
formats, and `backing_chain_depth=1`. It hashes only the immutable base and
requires that digest to match `--expected-digest`; it deliberately does not
hash the active overlay. The live probe also rejects a stopped domain, a
symlinked or non-regular base, a base not readable by the remote `qemu` group,
and a base writable by group or other. The composite acceptance gate requires
this receipt, so records from a different guest cannot be combined merely
because they use the same image.

The source URL and path are deliberately recorded now so a later standalone
Browser-stack extraction can bind the guest profile to an immutable source
record rather than silently reusing a host image.
