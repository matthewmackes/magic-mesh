# WL-FUNC-018 launch-action admission — r162

- Revision: `abcacee4` (`require launch authority for App VM projections`).
- Scope: installed Flatpak rows without the exact `launch` action are withheld from the App-VM projection.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func018-launch-admission-r162 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::app_catalog::tests::installed_row_without_launch_action_is_not_projected -- --nocapture`
- Result: `1 passed; 0 failed; 4703 filtered out` on seat 90.
