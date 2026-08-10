# Surface Pro 5/6 deployment and access preflight

This is a read-only operations procedure, not a second worklist. It does not
install packages, copy files, accept a new SSH host key, change services,
enroll a MOK, query for firmware updates, collect physical evidence, or reboot.

The canonical Surface Pro 6 seat is `Surface`, at LAN `172.20.146.79` and
overlay `10.42.0.7`. These endpoints are fixed inside the helper and are always
redacted from JSON output. Run:

```bash
install-helpers/preflight-surface-pro56-deployment.py > /tmp/surface-preflight.json
```

Exit `0` means at least one canonical endpoint admitted the existing root SSH
identity, exact DMI identified a Surface Pro 6, its baked `mackesd` revision
matched a clean local HEAD, the bounded SHA-256 of its executable acceptance
collector exactly matched the locally approved collector, and the complete
Fedora 44 Surface artifact set passed the governed signature verifier. A dirty
tracked checkout or failed Git status probe blocks deployment rather than
claiming a revision it cannot represent. Exit `3` is a valid but blocked
preflight. Exit `2` means the request or local inputs were invalid. The current
blocked artifact manifest is expected to produce exit `3`; its bounded blocker
text is carried in the JSON.

A Surface Pro 5 is never guessed from the Pro 6 addresses. Supply its private
IPv4 address explicitly after placing the optional seat on the network:

```bash
install-helpers/preflight-surface-pro56-deployment.py \
  --generation 5 --pro5-address 192.168.1.25
```

The remote probe ignores user and system SSH configuration, uses `BatchMode`
with public-key-only authentication, disables proxy/jump commands, connection
multiplexing, local commands, agent/X11 forwarding, and all other forwarding,
and pins the root and global `known_hosts` paths with
`StrictHostKeyChecking=yes` and `UpdateHostKeys=no`. An unknown host key is a
blocker rather than an invitation to mutate `known_hosts`. Every local and
remote command capture has a fixed byte ceiling and timeout; overflow or
timeout kills and reaps the command's process group and blocks the probe.
Output contains no address, command stderr, usernames beyond the fixed root
admission contract, host keys, environment, credential paths, or arbitrary
remote text.

Passing this preflight does not claim physical acceptance. After deployment,
run the separately documented bounded collector and then complete its declared
touch, pen, Type Cover, buttons/storage, rotation, camera, audio, suspend, and
boot/recovery checks with an operator. The preflight only proves that this
collector/manual-proof phase is available and structurally complete.

Parser and hostile-target regression:

```bash
install-helpers/preflight-surface-pro56-deployment.py --self-test
```
