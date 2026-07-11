#!/usr/bin/env bash
# shellcheck disable=SC2034  # every var here is consumed by the scripts that SOURCE this file
# rpm-features.sh — the SINGLE canonical source of the RPM cut's cargo build
# knobs, SOURCED (never executed) by BOTH release-cut paths so they can never
# drift out of sync:
#   - install-helpers/xcp-build.sh          (native build on a farm XCP VM)
#   - install-helpers/build-rpm-fedora43.sh (fedora:43 container cut)
#
# build-deploy-3 fix (2026-07-11): the mde-shell-egui feature list and the
# `--locked` policy used to be hand-duplicated verbatim in both scripts. A
# feature enabled in one cut path but not the other silently ships a
# differently-configured binary — this class of gap actually bit the fleet
# twice (the 2026-07-03 FakeMpv media regression and the 2026-07-05
# gated-EmptyState Browser regression, both cited in the scripts' own comments).
# Define each knob ONCE here; both scripts source this file and consume the
# variables, so the two paths are equal by construction.
#
# This fragment must stay side-effect-free: only variable assignments, no
# `set -e`, no output, safe to source under `set -u`.

# The feature set the shipped seat binary (mde-shell-egui) is re-linked with for
# the release RPM. Each flag turns a surface from a gated placeholder into the
# real thing:
#   drm         — own the bare KMS/DRM seat, no Wayland compositor        [E12-3]
#   live-helper — Browser surface really spawns the shipped mde-web-preview
#                 (without it it is a permanent gated EmptyState)     [BOOKMARKS-6]
#   live-vdi    — Desktop surface pumps live IronRDP in-shell              [E12-5]
#   media-mpv   — Media surface links the real mpv engine, not FakeMpv [BUG-VIDEO-1]
MDE_RPM_SHELL_FEATURES="drm,live-helper,live-vdi,media-mpv"

# Reproducible-build policy for EVERY cargo build in the RPM cut path: assert the
# committed Cargo.lock is authoritative and is never mutated mid-cut, so the farm
# cut stays byte-faithful to the canonical fedora:43 cut. This RECONCILES the old
# divergence where xcp-build.sh omitted --locked on the workspace build + the
# shell re-link while build-rpm-fedora43.sh used it uniformly; the stricter
# uniform policy wins (a release cut must build against the pinned lockfile).
# Set to "" to opt out everywhere at once.
MDE_RPM_LOCKED="--locked"
