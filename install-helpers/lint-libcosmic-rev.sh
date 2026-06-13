#!/usr/bin/env bash
# CUT-5 — libcosmic single-rev guard (EFF-35 identical-rev policy).
#
# Our four GUI consumers pin libcosmic at REV below. But
# cosmic-panel-config (a libcosmic dep via pop-os/cosmic-panel)
# references the fork by BARE URL — floating master — and cargo
# can't `[patch]` two rev-specs of the same repo into one source.
# The Cargo.lock is the only thing pinning that bare source; a
# `cargo update` would silently drift it past the rev our shims and
# fork-API fixes were written against, splitting the tree across two
# libcosmic commits.
#
# This lint fails when ANY pop-os/libcosmic package stanza in
# Cargo.lock resolves to a commit other than the pinned rev. On a
# deliberate rev bump: update REV here and in the four Cargo.tomls
# (mde-files, mde-music, mde-cosmic-applet, mde-role-chooser) in the
# same commit.
set -euo pipefail
cd "$(dirname "$0")/.."

REV="cca48bc29ef7a9f22160c0ab6ba117ab22d1ae87"

bad=$(grep -E '^source = "git\+https://github\.com/pop-os/libcosmic' Cargo.lock \
  | grep -v "$REV" || true)

if [[ -n "$bad" ]]; then
  echo "lint-libcosmic-rev.sh: FAIL — libcosmic source(s) drifted off the pinned rev ($REV):" >&2
  echo "$bad" >&2
  exit 1
fi

echo "lint-libcosmic-rev.sh: clean — every libcosmic source in Cargo.lock is at ${REV:0:8} (EFF-35)"
