# WL-FUNC-023 — Health nags for dest-cut overlay identity leftover (2026-08-25)

Source heal so Construct Health offers an honest Fix for dest-cut seats whose
overlay-ip is empty and host cert is absent. `production_admitted` unchanged.
No REL freeze. No invented dest, token, or mesh-id. Foreign dirty `mackesd`
files were not folded. `Restart mackesd` remains unconfirmed on live seats.

## Why the dest-cut click could not close leftover-3

`PublishOverlayIp` requires `live_overlay_ip && !overlay_ip_published`. Seat 15
and Dell have no host cert, so nebula1 is not live and that nag is silent.
mesh-status still reports monolithic `mackesd` down while grouped
`mackesd-control` is active, so Health led with confirmation-required
`Restart mackesd`. Collaboration identity files exist but `source_revision`
is the previous cut (`7e3474eeb`) vs installed `4071ed295`; file-exists was
treated as admitted.

## Source change (`crates/mesh/mackesd/src/workers/node_grade.rs`)

1. Workstation with missing host cert **or** empty overlay-ip without live
   nebula1 nags `overlay-identity-missing` with typed `OpenOnboarding`.
   `PublishOverlayIp` stays only for a live nebula1 address.
2. Grouped `mackesd-control` plane skips required-service nags for
   `mackesd` / `dns` / `kdc` so Health does not offer `Restart mackesd.service`.
3. `collab_identity_admitted` requires admission JSON `source_revision` to
   match `mde_theme::brand::build::info().git_hash`. Fail-closed `node_key.rs`
   is unchanged.
4. `/mnt/mesh-storage` is present only when `mountpoint -q` succeeds, not
   merely when the path is a directory. The nag offers `OpenOnboarding`
   because `setup-syncthing` now fail-closes on a non-mount and cannot invent
   that dest.

## Farm

```
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=2
install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::node_grade
```

Result: `ok. 39 passed; 0 failed` including
`missing_host_cert_or_empty_overlay_offers_onboarding_not_publish`,
`grouped_plane_does_not_nag_monolithic_mackesd`, and
`collab_identity_file_must_match_installed_revision`.

## Close condition (still open)

Live Construct Fix proof on dest-cut Seat 15 and Dell still requires a dest-cut
of this source. Installed `13.0.0-35` at `4071ed295` does not carry these nags.
Do not invent an enroll dest to click around the leftover.
