//! Per-panel views — one module per group leaf. CB-1.x ports
//! land these incrementally; each module ships a state struct,
//! a `Message` variant set, an `update` reducer that returns
//! the parent app's `Message`, and a `view` builder over
//! [`Element<'_, crate::Message>`].

pub mod about;
pub mod audit;
pub mod compute;
pub mod config_apply;
pub mod connect;
/// CONNECT-6 — the unified connectivity / exposure matrix panel.
pub mod connectivity;
pub mod dns;
pub mod drift;
pub mod firewall;
pub mod fleet_logs;
pub mod fleet_revisions;
pub mod fleet_rollup;
pub mod fleet_settings;
pub mod hardware;
pub mod health_check;
pub mod help_index;
pub mod home;
pub mod hub;
pub mod images;
pub mod interfaces;
pub mod inventory;
pub mod jobs;
pub mod json_helpers;
pub mod lighthouses;
pub mod logs;
pub mod mesh_bus;
pub mod mesh_control;
pub mod mesh_federation;
pub mod mesh_history;
pub mod mesh_join;
pub mod mesh_logs;
pub mod mesh_pending;
pub mod mesh_services;
pub mod mesh_storage;
pub mod mirrors;
pub mod music;
pub mod network_hosts;
pub mod node_roles;
pub mod node_roster;
pub mod notifications;
/// PD-3 — the Peers directory (the Front Door).
pub mod peers;
/// PD-7 — the live mesh map (the Peers panel's Map view).
pub mod peers_map;
pub mod playbooks;
pub mod policy;
pub mod profiles;
pub mod registration;
pub mod remote_desktop;
pub mod repair;
pub mod resources;
pub mod routing;
pub mod run_history;
pub mod service_publishing;
/// VOIP-GW-1 — the mesh-wide SIP outbound gateway settings panel.
pub mod sip_gateway;
pub mod snapshots;
pub mod sparkline;
pub mod sync_status;
pub mod system_update;
pub mod tags;
pub mod vm_wizard;
pub mod vpn;
pub mod wallpaper;
pub mod wifi;
