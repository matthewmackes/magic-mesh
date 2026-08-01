# App VM guest image

`Containerfile` defines the immutable `wayland-standard` guest profile used by
`app-vm-wayland-standard.qcow2`. It is separate from the Construct host image:
the guest owns Sway, xdg-desktop-portal, Flatpak, PipeWire, input, and the VDI
application surface.

The image does not add a public Flatpak remote. Deployment must provision a
signed remote named `curated`; the App VM first-boot runtime refuses to install
anything if that remote is missing. Catalog data supplies only a validated app
identity, never a URL, command, mount, environment, or socket.

The image also carries `validate-runtime-inputs.sh`. Cloud-init invokes this
validator before admission; it bounds and allowlists the app, catalog revision,
guest profile, session, VM hostname, and capabilities metadata. Missing or
malformed validator/image inputs fail closed. `verify-image.sh` inspects a
built image before any disk artifact is emitted, and fails if the immutable
guest contract or required runtime packages are missing, or if a public
Flatpak remote has been pre-admitted. It also carries the immutable
`image-contract.json` profile marker; cloud-init refuses to admit an app when
the image does not identify the `wayland-standard`/Sway/`curated` contract.
Run `verify-contract.sh` for the focused static and fixture checks. The
verifier includes a bounded terminal-runtime evidence fixture: `connected` and
`reconnecting` are admitted, `failed` is rejected for readiness, and
`unavailable` is retained as an admitted-but-not-ready observation. The
fixture advances generations monotonically and rejects a replay; it also
rejects command, path, mount, environment, socket, and host-fallback fields.
Every connected, reconnecting, unavailable, or terminal observation is also
bound to the admitted session, VM, and application identities; an identity
drift during reconnect is rejected rather than being allowed to supersede the
guest session.
This is a contract check, not a claim that a live guest or VDI transport is
available.

The guest launcher also owns shutdown: TERM, INT, and HUP are trapped, the
Flatpak child is terminated and waited for, and a terminal `failed` observation
is published before the launcher exits. This prevents a stopped or reconnecting
session from leaving an orphaned application painting a stale VDI surface.

Build from the repository root with `build-image.sh`. The driver resolves the
base image before invoking Podman, bounds a registry probe with
`MCNF_PULL_TIMEOUT`, and exits with `3` when the farm cannot reach the registry;
an image is never claimed or passed to disk conversion in that state. The
successful path runs `verify-image.sh` before any disk output. The repo lane
requires the configured signed package channel; the local lane accepts a
staged `magic-mesh-*.rpm` and is useful for an air-gapped farm build. The
Containerfile uses `context.containerignore` so only the packaging inputs enter
the build context.
