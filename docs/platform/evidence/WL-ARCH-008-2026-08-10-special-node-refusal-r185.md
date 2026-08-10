# WL-ARCH-008 — special-node refusal (r185)

Date: 2026-08-10

The portable Browser profile migration walker now inventories every
non-directory filesystem entry without following links. FIFOs, sockets, and
device nodes in an allowed source location are represented as failed entries
and refuse bundle publication instead of disappearing from an apparently
successful migration.

## Farm proof

Farm: `.90`, slot `arch008-special-node-refusal-r185`.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch008-special-node-refusal-r185 install-helpers/xcp-build.sh sync
printf '%s\n' 'cd ~/magic-mesh-farm-arch008-special-node-refusal-r185' \
  'python3 install-helpers/verify-browser-portable-boundary.py --self-test' 'exit' \
  | MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch008-special-node-refusal-r185 install-helpers/xcp-build.sh shell

migrate-browser-profile: self-test passed
browser portable boundary: PASS
verify-browser-portable-boundary.py: self-test passed
```

The fixture covers a FIFO in the allowlisted `History` path and confirms no
partial output directory is left behind. This is source-level migration proof;
legacy-profile discovery, guest import, and live three-seat Browser VM quality
remain unproven here.
