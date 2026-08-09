# WL-ARCH-008 live profile inventory and atomic import — 2026-08-09

## Result

A read-only, aggregate-only inventory found no legacy Chromium/Chrome profile
candidate in the accessible user scope on seat 15 or Eagle. Seat 15 also had
zero candidates across all home directories; its two governed Browser
deployment roots are runtime/image fixtures, not a portable legacy profile.
Dell was offline, and T480 and Surface rejected the available key, so no real
profile was opened or migrated. No filename or content from a user profile was
printed, read, exported, or changed.

The portable migration helper now refuses partial publication and detects a
profile changing between inventory and copy. It opens allowlisted sources
without following final-component symlinks, binds the copy to the inventoried
device/inode/size/timestamps/SHA-256, and publishes only after every imported
file remains exact. Credential-bearing stores are still classified before any
content read and remain in place.

## Verification

- BigBoy `.130`, slot `arch008-profile-r4`: Python compilation and
  `verify-browser-portable-boundary.py --self-test` passed.
- Representative redacted filesystem fixtures passed byte-identical repeated
  migration. Cookies, password stores, passkeys, and sealed credentials kept
  their exact source bytes and modes and never entered the bundle.
- Hostile live-mutation and failed-entry fixtures both refused publication and
  left no partial output. Local scoped `git diff --check` passed.
- Source SHA-256: migration helper
  `fd50bd7761d110d0696d118dd2a09fd3f55bc3e87a541cc0d481c43f0b0cfac5`;
  boundary verifier
  `6fcf7e7b6825a94d4ee20b47443fddb07bbe31146e844a13c546104394d924ed`.

## Remaining blocker

S2 still needs a governed credential path to T480/Surface or an online Dell
with an actual legacy profile, followed by two unchanged imports and guest-side
restore proof. The correction does not claim live profile or guest migration.
