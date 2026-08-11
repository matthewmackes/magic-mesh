# Stable exact-gate integration wave — 2026-08-11

- Scope: one fixed coordinator snapshot covering all implementation slices harvested after the prior r443-r472 evidence waves.
- Farm: `.90` slot 1 (mackesd), BigBoy `.130` slot 1 (GUI), `.196` slot 1 (scripts, cap respected), and `.50` slot 1 (Bus/contracts).
- Method: each queue used `set -e`; every Rust command was `cargo test -p <package> <test> -- --exact --nocapture`.
- Result: **PASS**, 43 focused hostile regressions passed; no broad workspace suite ran.

## mackesd — 9/9

- `workers::proc::tests::hostile_helper_descendant_cannot_outlive_the_bounded_process_invocation`
- `workers::mesh_mount::tests::restarted_worker_key_aliases_cannot_redirect_the_authenticated_mesh_identity`
- `network_status::tests::hostile_network_provider_cannot_pin_the_observation_worker`
- `ipc::directory::tests::restarted_directory_cannot_count_stale_healthy_peer_as_current_and_unreachable`
- `mesh_media::tests::restarted_music_client_cannot_adopt_equivocated_healthy_shared_account`
- `workers::device_control::tests::hostile_provider_driver_cannot_escape_or_reinterpret_the_control_seam`
- `workers::lighthouse_probe::tests::returned_peer_cannot_regain_lighthouse_authority_from_stale_fs_during_etcd_outage`
- `workers::upgrade_intent_watcher::tests::hostile_installer_cannot_pin_the_upgrade_worker_past_its_process_budget`
- `workers::weather_forecast::tests::hard_linked_restart_cache_cannot_retain_weather_authority`

## GUI and native workspaces — 17/17

- `navigation_ui::tests::retained_navigation_consumer_cannot_rebind_projection_or_action_to_foreign_host`
- `buffer::tests::replaced_editor_save_path_cannot_redirect_native_document_bytes`
- `lsp_nav::tests::replaced_closed_workspace_path_cannot_redirect_lsp_rename`
- `widgets::tests::hostile_caller_copy_cannot_erase_canonical_workspace_state_language`
- `search_omnibox::tests::restarted_search_cannot_activate_equivocated_target_authority`
- `sheet::tests::hostile_detent_authority_cannot_poison_shared_modal_geometry_or_motion`
- `workload_api::tests::foreign_node_projection_cannot_authorize_local_workload_presentation`
- `chooser::chooser_prefs::tests::restarted_taskbar_pins_cannot_adopt_foreign_identity_or_misbound_seat_record`
- `communications::tests::restarted_shell_cannot_adopt_retained_generic_clock_command_authority`
- `chrome::tests::hostile_health_replacement_cannot_take_kiron_lower_third_authority`
- `chooser::pinned_rail_sources_tests::stale_discovery_card_cannot_authorize_power_without_workload_projection`
- `springboard::tests::foreign_focus_cannot_replay_retained_home_search_authority`
- `status::tests::hostile_history_append_cannot_reactivate_resolved_status`
- `app::tests::restarted_daemon_target_cannot_authorize_retained_handoff_identity`
- `tests::hostile_setid_or_root_launch_cannot_inherit_music_ui_authority`
- `transfers::tests::hard_linked_ledger_record_cannot_inject_transfer_authority`
- `preview::tests::hard_linked_resource_preview_cannot_read_ambiguous_file_authority`

## Bus, contracts, and collaboration — 8/8

- `dnd::tests::restarted_dnd_reader_cannot_adopt_replaced_clock_suppression_authority`
- `federation::tests::hostile_grant_replica_cannot_remove_clipboard_federation_boundary`
- `rpc::tests::generic_rpc_cannot_replay_retained_clock_command_to_foreign_peer`
- `broker::tests::hostile_overlay_publish_cannot_redirect_rich_clipboard_broker_authority`
- `workloads::tests::terminal_projection_cannot_retain_attachment_authority_after_restart`
- `app_catalog::tests::hostile_catalog_row_without_launch_action_cannot_authorize_app_vm_launch`
- `resources::tests::restarted_browser_cannot_admit_future_catalog_under_current_attestation`
- `domain::tests::hostile_member_deletion_cannot_revoke_space_authority_after_restart`

## Packaging and release scripts — 9/9

- `install-helpers/write-candidate-manifest.py --repo /home/mm/magic-mesh-farm-1 --self-test`
- `packaging/browser-vm/promote-catalog-image.py --self-test`
- `packaging/android/verify-manifest.sh --self-test`
- `packaging/browser-vm/prepare-ephemeral-nocloud.sh --self-test`
- `install-helpers/verify-github-release-binding.sh --self-test`
- `packaging/app-vm/validate-runtime-inputs.sh --self-test`
- `packaging/android/verify-contract.sh --self-test`
- `install-helpers/verify-corrected-forward-recovery.py self-test`
- `packaging/browser-vm/deploy-image.sh --self-test`

## Integration corrections

- `search_omnibox` now declares its authority-index type explicitly.
- the workspace-state hostile fixture installs the shared font map before rendering.
- App-VM descriptor inspection uses `/proc/self/fd/3`, not the parent shell PID retained by command substitution.
- Passed queue prefixes were not rerun; only failed commands and their remaining suffixes resumed.

- Remaining boundary: live hardware, three-seat, and six-node/lighthouse evidence named by the owning epics.
