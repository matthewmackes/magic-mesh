# WL-FUNC-026/027 — pin XDG_CONFIG_HOME on the root DRM shell (2026-08-24)

Seat 15 dest hydrate showed the root DRM shell has no login `HOME` /
`XDG_CONFIG_HOME`, so `default_config_file()` is `None` and Files persist
is a no-op. The packaged unit `packaging/bootc/units/mde-shell-egui.service`
now sets `Environment=XDG_CONFIG_HOME=/root/.config` next to the existing
PipeWire `XDG_RUNTIME_DIR=/run/user/1000` pin. `role_provision` asserts
the pin via `include_str` of that unit.

This is source for the next unpublished RPM. It does not close operator
GUI view/sort/pin leftovers. Seat 15 already has a local drop-in dest.
`production_admitted` unchanged.
