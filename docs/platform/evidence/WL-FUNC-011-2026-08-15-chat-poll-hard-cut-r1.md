# WL-FUNC-011 Chat poll hard-cut r1

Date: 2026-08-15

The shipped shell no longer polls the retired `mde-chat` read model to drive
notifications. `Shell::update` now polls the canonical Communications read
model whenever the shell is expanded; the old Chat surface/render path remains
unreachable and is not a live update consumer.

Farm validation:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=collab-hardcut-farm26 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui legacy_node_surfaces_normalize_into_workers_tabs -- --nocapture
```

Result: the shell crate compiled and linked successfully; the targeted route
test passed 1/1. The existing shell suite remains the authoritative
route/render test surface.
