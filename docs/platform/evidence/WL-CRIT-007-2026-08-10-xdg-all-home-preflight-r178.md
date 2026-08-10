# WL-CRIT-007 XDG recovery all-home preflight — r178

- Scope: recovery validates every desktop home and XDG target before the first mount mutation, refusing a later hostile symlink without partial restoration.
- Farm gate: synced with `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=crit007-xdg-recovery-r178 install-helpers/xcp-build.sh sync`; then ran `bash install-helpers/test-mesh-xdg-bind-recovery.sh` on `.50` with passwordless root fixture support.
- Result: helper refused the hostile later `Music` target and fixture passed `all-home preflight: hostile later target causes zero mount mutations`.
