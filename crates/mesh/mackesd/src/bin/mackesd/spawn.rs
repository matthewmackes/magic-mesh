//! `mackesd` bin submodule — worker-spawn + Bus-responder helpers.
//!
//! ARCH split: the `spawn_*_workers` and `start_*_bus_responders` helpers
//! were relocated VERBATIM from `bin/mackesd.rs` (the block below `run_serve`)
//! into this sibling submodule to shrink the daemon entry file. Pure
//! relocation, no logic change: identical spawn order + `worker_names`
//! registration. Reached via `#[path = "mackesd/spawn.rs"] mod spawn;` — a
//! bin file is a crate root, so a plain `mod spawn;` would resolve to
//! `src/bin/spawn.rs` and cargo would mis-detect a stray binary; the
//! `#[path]` keeps the submodule in the bin subdir (mirrors the existing
//! `#[path = "../cli/mod.rs"] mod cli;`).

use super::*;

/// WL-ARCH-004 — register one role-tiered worker from the single
/// [`mackesd_core::worker_role::WORKER_REGISTRY`] table.
///
/// The worker's **rank gate** and its **restart policy** both come from its
/// registry entry (keyed by `name`), so the spawn site supplies only the
/// *constructor* — a closure invoked LAZILY, and ONLY when the gate passes, so a
/// gated-out worker is never built (the historic behavior of the inline
/// `if runs(...) { sup.spawn(Spawn::new(ctor, policy)); push }` block this
/// replaces). Keeping the constructor at the call site (rather than in the table)
/// preserves each worker's heterogeneous, order-sensitive construction and the
/// exact spawn order, while the gate + policy + census all flow from the one table.
///
/// Panics if `name` is absent from the registry — an unregistered tiered spawn,
/// a programming error the `worker_spawns_and_the_census_do_not_drift` test catches
/// first.
pub(crate) fn spawn_tiered<W, F>(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    name: &'static str,
    build: F,
) where
    W: mackesd_core::workers::Worker,
    F: FnOnce() -> W,
{
    if !mackesd_core::worker_role::runs(name, role_rank) {
        return;
    }
    let policy = mackesd_core::worker_role::policy_for(name).unwrap_or_else(|| {
        panic!(
            "WL-ARCH-004: worker '{name}' spawned via spawn_tiered but absent from WORKER_REGISTRY"
        )
    });
    sup.spawn(mackesd_core::workers::Spawn::new(build(), policy));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push(name.into());
}

// run_serve round-2 extract: the Nebula-status + Shell control-surface Bus
// responders (action/nebula/* + action/shell/*). Verbatim thread-spawns +
// worker_names registration, original order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_control_surface_bus_responders(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    worker_status: &mackesd_core::workers::WorkerStatusMap,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    node_id: &String,
    host: &String,
    workgroup_root: &PathBuf,
    db_path: &PathBuf,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // E0.3.1 (EPIC-RETIRE-DBUS, 2026-06-03) — Nebula status
    // Bus responder. The three read-projection verbs
    // (`status` / `self-node` / `list-peers`) migrated off the
    // retired `dev.mackes.MDE.Nebula.Status` D-Bus methods onto
    // the mesh Bus at `action/nebula/<verb>`. The responder
    // runs on its own OS thread with a current-thread tokio
    // runtime — the pure builders hold an
    // `Arc<Mutex<rusqlite::Connection>>` guard across `.await`,
    // which is `!Send` and would not compile on the main
    // multi-thread executor (same constraint mde-session's
    // serve_bus solved this way). It opens its own SQLite
    // handle + the per-peer Bus Persist index, loops until the
    // shutdown flag flips. Graceful-degrade: a missing data-dir
    // or a failed SQLite/Persist open logs + skips the thread
    // (the consumers fall back to their empty/diagnostic
    // rendering exactly as they did when the daemon was down).
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => match mackesd_core::store::open(&db_path) {
            Ok(conn) => {
                let resp_store = Arc::new(tokio::sync::Mutex::new(conn));
                let resp_svc = mackesd_core::ipc::nebula::NebulaStatusService::new(
                    Arc::clone(&resp_store),
                    node_id.clone(),
                    host.clone(),
                )
                .with_workgroup_root(workgroup_root.clone());
                let resp_shutdown = Arc::clone(&shutdown);
                std::thread::Builder::new()
                    .name("nebula-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::nebula::serve_bus(&persist, &resp_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_handle| {
                        tracing::info!(
                            "Nebula Bus responder spawned; serving \
                                 action/nebula/{{status,self-node,list-peers}}"
                        );
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            error = %e,
                            "Nebula Bus responder thread spawn failed; \
                             NF-10..NF-18 consumers will see no peer data"
                        );
                    });
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("nebula_bus_responder".into());
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %db_path.display(),
                    "Nebula Bus responder: sqlite open failed; responder skipped"
                );
            }
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Nebula Bus responder: bus persist open failed; responder skipped"
            );
        }
    }
    // E0.3.5 — Shell control surface (version/healthz/workers) on
    // the mesh Bus at action/shell/<verb>, replacing the retired
    // dev.mackes.MDE.Shell D-Bus interface. Own OS thread
    // (Persist/rusqlite isn't Send); no tokio runtime needed since
    // the Shell builders are synchronous. Graceful-degrade: a
    // missing data-dir / failed Persist open logs + skips (the
    // Overview's mackesd-alive probe then reads offline).
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let shell_svc =
                mackesd_core::ipc::shell::ShellService::new(mackesd_core::ipc::shell::ShellState {
                    db_path: db_path.clone(),
                    worker_names: Arc::clone(&worker_names),
                    // EFF-24 — live worker status → healthz readiness.
                    worker_status: Some(Arc::clone(&worker_status)),
                    // OB6-FIX-4 — live mesh size + leadership in healthz.
                    workgroup_root: workgroup_root.clone(),
                    node_id: node_id.clone(),
                });
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("shell-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::shell::serve_bus(&persist, &shell_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Shell Bus responder spawned; serving \
                             action/shell/{{version,healthz,workers}}"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        error = %e,
                        "Shell Bus responder thread spawn failed; \
                         Overview mackesd-alive probe will read offline"
                    );
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("shell_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Shell Bus responder: bus persist open failed; responder skipped"
            );
        }
    }
}

// run_serve round-2 extract: the bus retention GC thread (BULLETPROOF-1 /
// BUS-RETENTION-2). Not a `*_bus_responder` but a shutdown-gated std::thread
// that lived in the responder region and registers `bus_retention_gc`.
pub(crate) fn start_bus_retention_gc(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // BULLETPROOF-1 — run the bus retention GC. The spool lives on `/run`
    // (tmpfs); the GC pass exists in mde-bus but only the standalone
    // `mde-bus` daemon ran it, and mackesd embeds the bus as a library and
    // ships NO `mde-bus.service` — so on every deployed node retention
    // NEVER ran and the `audit/*` (retention=forever) lane grew until it
    // filled `/run` and bricked the node (found live on both lighthouses
    // 2026-06-16). Own OS thread (sync pass); cap is filesystem-relative so
    // a ~190 MB lighthouse tmpfs and a multi-GB workstation tmpfs are both
    // bounded well below ENOSPC; the hard-cap valve sheds oldest-first.
    if let Some(bus_root) = mde_bus::default_data_dir() {
        let policy = bus_retention_policy(&bus_root);
        let resp_shutdown = Arc::clone(&shutdown);
        std::thread::Builder::new()
                .name("bus-retention-gc".into())
                .spawn(move || {
                    // Faster than the 1h library default — a small tmpfs needs
                    // tighter bounding; cheap (a SQLite scan + a dir walk).
                    let interval = std::time::Duration::from_secs(120);
                    // BUS-RETENTION-2 — edge-triggered /run-low alert state, so we
                    // warn once on the transition into low rather than every pass.
                    let mut run_low = false;
                    while !resp_shutdown.load(Ordering::Relaxed) {
                        match mde_bus::retention::run_pass_at(
                            &policy,
                            &bus_root,
                            mde_bus::retention::current_unix_ms(),
                        ) {
                            Ok(r) if r.evicted > 0 => tracing::warn!(
                                removed = r.removed, evicted = r.evicted, bytes_after = r.bytes_after,
                                "bus retention: hard-cap reached — evicted oldest to stay off ENOSPC (BULLETPROOF-1)"
                            ),
                            Ok(r) => tracing::debug!(
                                removed = r.removed, bytes_after = r.bytes_after, "bus retention pass"
                            ),
                            Err(e) => tracing::warn!(error = %e, "bus retention pass failed"),
                        }
                        // BUS-RETENTION-2 — headroom guard. A full /run breaks
                        // dnf + the bus's own WAL (the v10.0.18 roll failure). The
                        // pass above already compacts; here we warn the operator
                        // (Hub) when free space drops below 15%, edge-triggered.
                        if let (Some(avail), Some(total)) = (
                            filesystem_avail_bytes(&bus_root),
                            filesystem_total_bytes(&bus_root),
                        ) {
                            let low = total > 0 && avail * 100 / total < 15;
                            if low && !run_low {
                                match mde_bus::retention::publish_run_low_warning(
                                    &bus_root,
                                    avail / 1024 / 1024,
                                    total / 1024 / 1024,
                                ) {
                                    Ok(()) => tracing::warn!(
                                        avail_mb = avail / 1024 / 1024,
                                        total_mb = total / 1024 / 1024,
                                        "bus retention: /run low (<15% free) — raised mackesd::alert (BUS-RETENTION-2)"
                                    ),
                                    Err(e) => tracing::warn!(error = %e, "failed to publish /run-low alert"),
                                }
                            }
                            run_low = low;
                        }
                        // Sleep in short slices so shutdown is responsive.
                        for _ in 0..interval.as_secs() {
                            if resp_shutdown.load(Ordering::Relaxed) { break; }
                            std::thread::sleep(std::time::Duration::from_secs(1));
                        }
                    }
                })
                .map(|_h| tracing::info!(
                    soft_mb = policy.quota_soft_bytes / 1024 / 1024,
                    hard_mb = policy.quota_hard_bytes / 1024 / 1024,
                    "Bus retention GC spawned (BULLETPROOF-1)"
                ))
                .unwrap_or_else(|e| tracing::warn!(error = %e, "Bus retention GC thread spawn failed"));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("bus_retention_gc".into());
    }
}

// run_serve round-2 extract: the mesh-connectivity Bus responders — Fleet
// (action/fleet/*), Connect (action/connect/*), Route (action/route/trace),
// Clipboard (action/clipboard/*). Verbatim, original order.
pub(crate) fn start_connectivity_bus_responders(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    node_id: &String,
    workgroup_root: &PathBuf,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // NOTIFY-CHAT-6 — the standalone `alert-mirror` worker was RETIRED here.
    // It mirrored this node's alert-lane messages to `<workgroup>/.mesh-alerts/`
    // to feed the retired standalone Notifications panel (the old shared-alert
    // model crate's poll-shared tail). Mesh-wide notifications now flow through
    // the ONE notification interface — the `chat` worker (NOTIFY-CHAT-2) folds
    // every alert lane into per-host `alert:<host>` conversations replicated over
    // the Syncthing chat log — so this parallel mirror + the shared-alert model
    // crate it used are gone (E12-14 decommission discipline).
    // E0.3.3 / FPG-4 — Fleet control surface (push/list/diff/
    // rollback/nudge) on the mesh Bus at action/fleet/<verb>,
    // replacing the retired dev.mackes.MDE.Fleet D-Bus interface.
    // The verbs are REAL (FPG-4): they run against the Syncthing-replicated
    // revision log via magic-fleet; any node serves + mints
    // (leaderless, FPG-3). Own OS thread (Persist/rusqlite isn't
    // Send); no tokio runtime (the responders are sync).
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            // FPG-4 — the verbs run against the Syncthing-replicated
            // revision log; any node serves + mints (leaderless, FPG-3).
            let fleet_svc =
                mackesd_core::ipc::fleet::FleetService::new(&workgroup_root, node_id.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("fleet-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::fleet::serve_bus(&persist, &fleet_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Fleet Bus responder spawned; serving \
                             action/fleet/{{push-revision,list-revisions,diff-revisions,rollback}} \
                             (FPG-4, Syncthing-replicated revision log)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Fleet Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("fleet_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Fleet Bus responder: bus persist open failed; responder skipped"
            );
        }
    }
    // CONNECT-1 — the connectivity/exposure responder: action/connect/*
    // serves the per-service exposure policy (mesh-only vs public-via-ingress)
    // from the shared-substrate TOML. Same dedicated-OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let connect_svc = mackesd_core::ipc::connect::ConnectService::new(
                workgroup_root.clone(),
                node_id.clone(),
            );
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("connect-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::connect::serve_bus(&persist, &connect_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Connect Bus responder spawned; serving \
                             action/connect/{{list-services,set-policy,expose,unexpose,\
                             list-templates,set-template}} (CONNECT-1)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Connect Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("connect_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "Connect Bus responder: bus persist open failed; responder skipped");
        }
    }
    // ROUTE-TRACE-1 — the route-trace responder: action/route/trace assembles
    // the typed PathGraph between two endpoints from the CONNECT exposure +
    // peer directory. Same dedicated-OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let route_svc = mackesd_core::ipc::route::RouteService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("route-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::route::serve_bus(&persist, &route_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Route Bus responder spawned; serving action/route/trace (ROUTE-TRACE-1)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Route Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("route_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "Route Bus responder: bus persist open failed; responder skipped");
        }
    }
    // CLIP-SYNC-1 (action layer) — the clipboard responder:
    // action/clipboard/{list,pin,unpin,delete,clear} edits the mesh-global
    // history the clipboard_sync worker maintains, for the Clipboard Viewer
    // (CLIP-VIEW-1). Same dedicated-OS-thread shape as Connect/Route.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let clip_svc =
                mackesd_core::ipc::clipboard::ClipboardService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("clipboard-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::clipboard::serve_bus(&persist, &clip_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Clipboard Bus responder spawned; serving \
                             action/clipboard/{{list,pin,unpin,delete,clear}} (CLIP-SYNC-1)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Clipboard Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("clipboard_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "Clipboard Bus responder: bus persist open failed; responder skipped");
        }
    }
}

// run_serve round-2 extract: the datacenter Bus responders on action/dc/* —
// Datacenter vm-power, Host-ops host-power, DC-power WoL, Tofu plan.
pub(crate) fn start_datacenter_bus_responders(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    workgroup_root: &PathBuf,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // DATACENTER (action layer) — the VM power-control responder:
    // action/dc/vm-power runs `xe vm-{start,shutdown,reboot}` over the
    // mesh-key SSH against an allowed dom0. Same dedicated-OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let dc_svc =
                mackesd_core::ipc::datacenter::DatacenterService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("dc-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::datacenter::serve_bus(&persist, &dc_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Datacenter Bus responder spawned; serving action/dc/vm-power (DATACENTER)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Datacenter Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("dc_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "Datacenter Bus responder: bus persist open failed; responder skipped");
        }
    }
    // DATACENTER-10 (action layer) — the host power-control responder:
    // action/dc/host-power runs `xe host-{disable,enable,reboot}` over the
    // mesh-key SSH against an allowed dom0 (maintenance on/off + reboot).
    // Same dedicated-OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let host_svc = mackesd_core::ipc::host_ops::HostOpsService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                    .name("host-ops-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::host_ops::serve_bus(&persist, &host_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_handle| {
                        tracing::info!(
                            "Host-ops Bus responder spawned; serving action/dc/host-power (DATACENTER-10)"
                        );
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "Host-ops Bus responder thread spawn failed");
                    });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("host_ops_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "Host-ops Bus responder: bus persist open failed; responder skipped");
        }
    }
    // DATACENTER-16 (action layer) — the Wake-on-LAN responder:
    // action/dc/wol broadcasts the 102-byte magic packet to
    // 255.255.255.255:9 to power on a sleeping/off machine by MAC.
    // Same dedicated-OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let dc_power_svc =
                mackesd_core::ipc::dc_power::DcPowerService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("dc-power-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::dc_power::serve_bus(&persist, &dc_power_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "DC-power Bus responder spawned; serving action/dc/wol (DATACENTER-16)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "DC-power Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("dc_power_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "DC-power Bus responder: bus persist open failed; responder skipped");
        }
    }
    // DC-15 (action layer) — the Tofu-plan responder: action/dc/tofu-plan
    // runs a read-only `tofu plan` of an allow-listed workspace under
    // infra/tofu/<ws> with its env sourced. Same dedicated-OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let tofu_svc = mackesd_core::ipc::tofu::TofuService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("tofu-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::tofu::serve_bus(&persist, &tofu_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Tofu Bus responder spawned; serving action/dc/tofu-plan (DC-15)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Tofu Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("tofu_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "Tofu Bus responder: bus persist open failed; responder skipped");
        }
    }
}

// run_serve round-2 extract: the egress Bus responders — VPN (action/vpn/*),
// DDNS (action/ddns/*), and the DDNS reconcile worker. Verbatim, original order.
pub(crate) fn start_egress_bus_responders(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    node_id: &String,
    workgroup_root: &PathBuf,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // VPN-GW-1 — the VPN responder: action/vpn/* tunnel CRUD + wg-quick/
    // openvpn bring-up over the per-node tunnel config. Same OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let vpn_svc = mackesd_core::ipc::vpn_gw::VpnService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                    .name("vpn-bus-responder".into())
                    .spawn(move || {
                        mackesd_core::ipc::vpn_gw::serve_bus(&persist, &vpn_svc, || {
                            resp_shutdown.load(Ordering::Relaxed)
                        });
                    })
                    .map(|_handle| {
                        tracing::info!(
                            "VPN Bus responder spawned; serving action/vpn/{{list-tunnels,\
                             add-tunnel,remove-tunnel,tunnel-up,tunnel-down,tunnel-status}} (VPN-GW-1)"
                        );
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "VPN Bus responder thread spawn failed");
                    });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("vpn_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "VPN Bus responder: bus persist open failed; responder skipped");
        }
    }
    // DDNS-EGRESS-3 — the DDNS config responder: action/ddns/* over the
    // [ddns] config. Same OS-thread shape.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let ddns_svc = mackesd_core::ipc::ddns::DdnsService::new(workgroup_root.clone());
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("ddns-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::ddns::serve_bus(&persist, &ddns_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "DDNS Bus responder spawned; serving action/ddns/{{get-config,\
                             set-config,add-record,remove-record}} (DDNS-EGRESS-3)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "DDNS Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("ddns_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "DDNS Bus responder: bus persist open failed; responder skipped");
        }
    }
    // DDNS-EGRESS-3 — the DDNS reconcile WORKER (engine half of the responder
    // above): tails event/vpn/signals (VPN-GW exit-IP changes) + a periodic WAN
    // check, resolves each [ddns] record's live SourceState, and reconciles via
    // the pure plan_action predicate → the DigitalOcean A/AAAA-record API
    // (§9-safe fixed-arg curl; token from the mesh secret store). Same
    // dedicated-OS-thread shape as the responders. Additive — one localized
    // spawn block.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let ddns_root = workgroup_root.clone();
            let ddns_node = node_id.clone();
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("ddns-reconcile".into())
                .spawn(move || {
                    mackesd_core::workers::ddns::serve_reconcile(
                        &persist,
                        &ddns_root,
                        &ddns_node,
                        true,
                        || resp_shutdown.load(Ordering::Relaxed),
                    );
                })
                .map(|_handle| {
                    tracing::info!(
                        "DDNS reconcile worker spawned; subscribes event/vpn/signals + WAN \
                             check, reconciles [ddns] records via the DigitalOcean DNS API \
                             (DDNS-EGRESS-3)"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "DDNS reconcile worker thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("ddns_reconcile".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "DDNS reconcile worker: bus persist open failed; worker skipped");
        }
    }
}

// run_serve round-2 extract: the Directory (action/mesh/directory, PD-1) and
// Jobs (action/jobs/*, PLANES-9) Bus responders.
pub(crate) fn start_directory_jobs_bus_responders(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    workgroup_root: &PathBuf,
    db_path: &PathBuf,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // PD-1 — the peer-directory responder: action/mesh/directory
    // answers with the joined per-peer record (presence tier,
    // health, version, overlay ip/role, revision currency). Same
    // dedicated-OS-thread shape as the fleet responder.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let dir_svc = mackesd_core::ipc::directory::DirectoryService::new(
                &workgroup_root,
                Some(db_path.clone()),
            );
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("directory-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::directory::serve_bus(&persist, &dir_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_h| {
                    tracing::info!("Directory Bus responder spawned (action/mesh/directory, PD-1)");
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Directory Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("directory_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Directory Bus responder: bus persist open failed; responder skipped"
            );
        }
    }
    // PLANES-9/10 — the jobs control surface (action/jobs/*):
    // list-templates / launch / runs / run-results. Same
    // dedicated-OS-thread shape; the job_exec worker does the
    // actual local runs.
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let jobs_svc =
                mackesd_core::ipc::jobs::JobsService::new(&workgroup_root, Some(db_path.clone()));
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("jobs-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::jobs::serve_bus(&persist, &jobs_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_h| {
                    tracing::info!("Jobs Bus responder spawned (action/jobs/*, PLANES-9)");
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Jobs Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("jobs_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "Jobs Bus responder: bus persist open failed; skipped");
        }
    }
}

// run_serve round-2 extract: the platform-surface Bus responders — Settings
// (action/settings/*), VOIP gateway (action/voip/*), APPS aggregator
// (action/apps/list). Verbatim, original order.
pub(crate) fn start_platform_bus_responders(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    workgroup_root: &PathBuf,
    db_path: &PathBuf,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // E0.3.4 — Settings store on the mesh Bus at
    // action/settings/<verb> (get/set/list-keys/snapshot/restore;
    // args in the request body), replacing the never-registered
    // dev.mackes.MDE.Settings D-Bus interface. Registering it makes
    // the store genuinely reachable for the first time. Own OS
    // thread (Persist/rusqlite isn't Send); no tokio runtime (the
    // settings free fns are synchronous).
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let settings_svc = mackesd_core::ipc::settings::SettingsService;
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("settings-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::settings::serve_bus(&persist, &settings_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Settings Bus responder spawned; serving \
                             action/settings/{{get,set,list-keys,snapshot,restore}}"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Settings Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("settings_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Settings Bus responder: bus persist open failed; responder skipped"
            );
        }
    }
    // VOIP-GW-1 — the mesh-wide SIP outbound gateway responder
    // (action/voip/{set-gateway,get-gateway,clear-gateway}). The root
    // daemon is the only writer with access to the QNM-Shared mount, so the
    // Workbench panel sets the gateway through here; it lands at
    // <workgroup_root>/voip/gateway.toml in the voice agent's account.toml
    // shape and replicates to every node. Own OS thread (Persist isn't Send).
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let voip_svc = mackesd_core::ipc::voip::VoipService::new(&workgroup_root);
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("voip-bus-responder".into())
                .spawn(move || {
                    mackesd_core::ipc::voip::serve_bus(&persist, &voip_svc, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "VOIP gateway Bus responder spawned; serving \
                             action/voip/{{set-gateway,get-gateway,clear-gateway}}"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "VOIP gateway Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("voip_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "VOIP gateway Bus responder: bus persist open failed; skipped");
        }
    }
    // APPS-1 — the apps_aggregator: serves action/apps/list (the unified
    // launchable-entry list for the Applications Panel launcher). Thin applet
    // (Q24): this root daemon is the single source of truth, aggregating local
    // XDG+flatpak apps, mesh peers' apps (PD-2 directory), workloads (compute
    // inventory), and published services. Own OS thread (Persist isn't Send).
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            let home =
                std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
            let node_id = local_hostname();
            let apps_svc =
                mackesd_core::ipc::apps::AppsService::new(&workgroup_root, &node_id, &home);
            let dir_root = workgroup_root.clone();
            let dir_db = db_path.clone();
            let inv_node = default_node_id();
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("apps-bus-responder".into())
                .spawn(move || {
                    let dir_doc = move || {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis() as u64);
                        mackesd_core::ipc::directory::DirectoryService::new(
                            &dir_root,
                            Some(dir_db.clone()),
                        )
                        .build_directory(now)
                    };
                    let inv_doc = move || mackesd_core::ipc::apps::read_local_inventory(&inv_node);
                    mackesd_core::ipc::apps::serve_bus(
                        &persist,
                        &apps_svc,
                        dir_doc,
                        inv_doc,
                        || resp_shutdown.load(Ordering::Relaxed),
                    );
                })
                .map(|_handle| {
                    tracing::info!(
                        "APPS aggregator Bus responder spawned; serving action/apps/list"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "APPS aggregator Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("apps_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "APPS aggregator Bus responder: bus persist open failed; skipped");
        }
    }
}

// run_serve round-2 extract: the Nebula signal dispatcher (event/nebula/signals).
// Fills the shared sender slot; registers no worker_names row (verbatim).
pub(crate) fn start_nebula_signal_dispatcher(
    nebula_signal_slot: &mackesd_core::ipc::nebula::SignalSenderSlot,
) {
    // E0.3.1.b — the Nebula signal dispatcher drains worker
    // NebulaSignal events onto the Bus event topic
    // (event/nebula/signals) + fills nebula_signal_slot so the
    // health_reconciler + nebula_csr_watcher workers pick up the
    // sender on their next tick. Relocated out of the retired
    // Fleet.Files D-Bus arm — it never depended on that connection.
    let _nebula_sender = mackesd_core::ipc::nebula::spawn_signal_dispatcher(&nebula_signal_slot);
    tracing::info!(
        "Nebula signal dispatcher spawned (Bus event topic {}); \
             health_reconciler + nebula_csr_watcher will emit on next \
             state transition",
        mackesd_core::ipc::nebula::NEBULA_EVENT_TOPIC,
    );
}

// run_serve round-2 extract: the Files Bus responder — one thread serving
// action/{files-inbox,files-outbox,files-downloads,file-ops,fleet-files}/* +
// the mesh-transfer surface. Verbatim.
pub(crate) fn start_files_bus_responder(
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    host: &String,
    db_path: &PathBuf,
) {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    // E0.3.2 — the five file-transfer surfaces moved off D-Bus onto
    // the mesh Bus: Fleet.Files (the live, store-backed mesh roster)
    // + the four Shell.* stubs (Inbox/Outbox/Downloads/
    // FileOperations — honest empty / transport-not-configured until
    // a future epic fills the transfer engine). One dedicated
    // responder thread serves all five over its own Persist
    // (rusqlite isn't Send); Fleet.Files locks the shared store via
    // blocking_lock on this non-async thread. Replaces
    // register_fleet_files + the session D-Bus connection (Shell +
    // Nebula already moved off it, so no D-Bus interface registers
    // anywhere now).
    match mde_bus::default_data_dir()
        .ok_or_else(|| "no XDG data dir for bus".to_string())
        .and_then(|d| mde_bus::persist::Persist::open(d).map_err(|e| e.to_string()))
    {
        Ok(persist) => {
            use mackesd_core::ipc::files;
            // AUD-1/AUD-7 — the real cross-mesh transport over the
            // Syncthing-replicated QNM-Shared volume. One `FileXfer` per
            // surface (cheap: just a root path + host id) backs inbox /
            // outbox / file-ops with genuine copy/list/rollback.
            // EFF-2 — `FileXfer::new` confines send-to sources to the
            // operator's home dir (the share root), so a Bus writer
            // can't exfil /etc/shadow / keys into a peer's inbox.
            let qnm_root = mackesd_core::default_qnm_shared_root();
            let xfer_inbox = files::FileXfer::new(qnm_root.clone(), host.clone());
            let xfer_outbox = files::FileXfer::new(qnm_root.clone(), host.clone());
            let xfer_ops = files::FileXfer::new(qnm_root.clone(), host.clone());
            let mut surfaces = vec![
                files::Surface {
                    prefix: files::INBOX_PREFIX,
                    verbs: &files::INBOX_VERBS,
                    reply: Box::new(move |verb, body| xfer_inbox.inbox_reply(verb, body)),
                },
                files::Surface {
                    prefix: files::OUTBOX_PREFIX,
                    verbs: &files::OUTBOX_VERBS,
                    reply: Box::new(move |verb, body| xfer_outbox.outbox_reply(verb, body)),
                },
                files::Surface {
                    prefix: files::DOWNLOADS_PREFIX,
                    verbs: &files::DOWNLOADS_VERBS,
                    reply: Box::new(files::downloads_reply),
                },
                files::Surface {
                    prefix: files::FILE_OPS_PREFIX,
                    verbs: &files::FILE_OPS_VERBS,
                    reply: Box::new(move |verb, body| xfer_ops.file_ops_reply(verb, body)),
                },
            ];
            // FILEMGR-7 — the peer-side direct-transfer helper: a cross-node
            // A→B copy rsyncs straight over the overlay (not double-hopped
            // through us). Reuses the FILEMGR-5/6 shared key + `<host>.mesh`
            // DNS + published mount scope; the live ssh/rsync leg is honestly
            // gated (§7) — an unprovisioned key/absent ssh replies `gated` so
            // the Files surface falls back to the sshfs relay.
            {
                use mackesd_core::ipc::mesh_transfer;
                let runtime_base = mackesd_core::workers::mesh_mount::resolve_runtime_base();
                let mesh_bus_dir = mde_bus::default_data_dir();
                let xfer = mesh_transfer::MeshTransfer::new(
                    runtime_base,
                    mackesd_core::ipc::secret_store::repo_root(),
                    mackes_mesh_types::peers::default_workgroup_root(),
                )
                .with_bus_dir(mesh_bus_dir);
                surfaces.push(files::Surface {
                    prefix: mesh_transfer::MESH_TRANSFER_PREFIX,
                    verbs: &mesh_transfer::MESH_TRANSFER_VERBS,
                    reply: Box::new(move |verb, body| xfer.reply(verb, body)),
                });
            }
            // Fleet.Files joins only when sqlite opens; its stub
            // siblings serve regardless.
            match mackesd_core::store::open(&db_path) {
                Ok(_conn) => {
                    // SUBAUDIT-A2 — FleetFilesService now reads the replicated
                    // directory (not the empty sqlite `nodes` table), so it
                    // needs the workgroup root, not the db handle.
                    let svc = files::FleetFilesService::new(
                        mackes_mesh_types::peers::default_workgroup_root(),
                        host.clone(),
                    );
                    surfaces.push(files::Surface {
                        prefix: files::FLEET_FILES_PREFIX,
                        verbs: &files::FLEET_FILES_VERBS,
                        reply: Box::new(move |verb, body| svc.reply(verb, body)),
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        db_path = %db_path.display(),
                        "Fleet.Files: sqlite open failed; mesh-roster surface \
                         omitted (the four stub surfaces still serve)"
                    );
                }
            }
            let resp_shutdown = Arc::clone(&shutdown);
            std::thread::Builder::new()
                .name("files-bus-responder".into())
                .spawn(move || {
                    files::serve_all(&persist, &surfaces, || {
                        resp_shutdown.load(Ordering::Relaxed)
                    });
                })
                .map(|_handle| {
                    tracing::info!(
                        "Files Bus responder spawned; serving action/{{files-inbox,\
                             files-outbox,files-downloads,file-ops,fleet-files}}/*"
                    );
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Files Bus responder thread spawn failed");
                });
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("files_bus_responder".into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Files Bus responder: bus persist open failed; responder skipped"
            );
        }
    }
}

// run_serve extract: rank-0 compute/lifecycle workers (mdns_relay .. health_reconciler).
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_compute_lifecycle_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    node_id: &String,
    workgroup_root: &PathBuf,
    db_path: &PathBuf,
    daemon_cfg: &mackesd_core::config::daemon::MackesdConfig,
    nebula_signal_slot: &mackesd_core::ipc::nebula::SignalSenderSlot,
) {
    use mackesd_core::workers::{heartbeat::HeartbeatWorker, mdns_relay::MdnsRelayWorker};
    // MESH-MDNS-RELAY — native cross-segment mDNS service relay (browses
    // the local LAN, publishes services to the mesh Bus). Rank 0: a relay
    // control-plane worker, runs on every role.
    spawn_tiered(sup, worker_names, role_rank, "mdns_relay", || {
        MdnsRelayWorker::new()
    });
    // RETIRE-PY.4 (2026-06-07) — the GVFS `fs_sync` worker (supervised
    // `python3 -m mackes.mesh_gvfs.daemon`, a retired Python MDE module
    // absent in the monorepo) is removed. Mesh storage is served by
    // Syncthing (E3); per-peer share access is via the Bus file-ops, so
    // the second FUSE substrate is retired rather than rebuilt.
    spawn_tiered(sup, worker_names, role_rank, "heartbeat", || {
        HeartbeatWorker::new(workgroup_root.clone(), node_id.clone())
            .with_interval(daemon_cfg.heartbeat_interval())
    });
    // BOOT-STATUS-1 — the boot_readiness worker: probes the fabric bring-up
    // chain (Nebula → overlay IP → mackesd → bus → QNM mount → directory) and
    // publishes an ordered snapshot to state/boot-readiness for the HOME
    // boot-status dialog. All roles (headless nodes report the same chain).
    spawn_tiered(sup, worker_names, role_rank, "boot_readiness", || {
        mackesd_core::workers::boot_readiness::BootReadinessWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
            db_path.clone(),
        )
    });
    // XCP-6 (B2) — on an XCP-ng dom0, advertise hypervisor capacity
    // (CPU/RAM/SR-free/running-VMs) to `compute/xcp-host/<node>` so any node
    // can target it for a VM spawn. Self-gates on the dom0 marker, so it's a
    // harmless no-op on every non-hypervisor node; spawned on all roles (a
    // joined XCP host pins Server).
    spawn_tiered(sup, worker_names, role_rank, "xcp_host", || {
        mackesd_core::workers::xcp_host::XcpHostWorker::new(node_id.clone())
    });
    // KVM-HEALTH (MV-2) — the Fedora+KVM successor to xcpng_health. Probes
    // the per-node KVM virtualization service catalog
    // (`mackesd_core::kvm::KVM_SERVICES`, `systemctl is-active` each) every
    // 30 s and publishes a whole-host health summary to `event/kvm/services`
    // so the Datacenter panels + the alert lane see the live stack state.
    // The KVM stack is universal — every mesh node runs the same libvirt +
    // Podman set (docs/design/mesh-virt-management.md: "same stack on every
    // machine") — so it gates through the rank-0-default worker resolver,
    // i.e. it runs everywhere.
    spawn_tiered(sup, worker_names, role_rank, "kvm_health", || {
        mackesd_core::workers::kvm_health::KvmHealthWorker::new(node_id.clone())
    });
    // MV-3 — the vm_lifecycle worker: the libvirt/KVM VM-lifecycle actuator
    // the Datacenter UI drives. Drains `action/vm/lifecycle` (create-from-
    // image / start / stop / destroy / list, each addressed to a target
    // node id) via an injectable LibvirtBackend that shells `virsh`/
    // `qemu-img` through the bounded proc path, and publishes this node's VM
    // instance roster to `event/vm/instances`. Universal like kvm_health —
    // every node can host datacenter VMs — so it gates through the
    // rank-0-default worker resolver (runs everywhere). node_id is both the
    // event `host` stamp and the action target this worker matches.
    spawn_tiered(sup, worker_names, role_rank, "vm_lifecycle", || {
        mackesd_core::workers::vm_lifecycle::VmLifecycleWorker::new(node_id.clone())
    });
    // MV-4 — the container worker: the Podman container-lifecycle actuator (the
    // container half of the mesh management layer, companion to MV-3
    // vm_lifecycle). Drains `action/container/lifecycle` (run / stop / rm /
    // list, each addressed to a target node id) via an injectable
    // PodmanBackend that shells `podman` through the bounded proc path, and
    // publishes this node's container roster to `event/podman/containers`.
    // Universal like vm_lifecycle — every node can host datacenter containers —
    // so it gates through the rank-0-default worker resolver (runs everywhere).
    // node_id is both the event `host` stamp and the action target this worker
    // matches.
    spawn_tiered(sup, worker_names, role_rank, "container", || {
        mackesd_core::workers::container::ContainerWorker::new(node_id.clone())
    });
    // E12-20 — the storage worker: the privileged owner of the Workbench
    // Storage plane (GParted for the mesh). Owns a typed StorageOp pending
    // queue over a live UDisks2 zbus topology, validates each op at stage-time
    // (advisory) + apply-time (authoritative), enforces the hard-wall
    // interlocks (root/boot/EFI · mesh-storage backer · in-use VM/container)
    // and the typed-arming echo IN the executor (a UI bug can't bypass), and
    // publishes the `state/storage/<node>` topology mirror + drains
    // `action/storage/<node>` verbs. Universal like vm_lifecycle/container —
    // any node has disks — so it is pinned at rank 0 in the worker_role census
    // (BUG-STORAGE-1: an EXPLICIT rank-0 entry, not the silent unknown-worker
    // default, so a Workstation provably publishes its own mirror and the
    // `role-workers` diagnostic lists it). node_id is the per-node topic
    // namespace + the mirror `host` stamp.
    spawn_tiered(sup, worker_names, role_rank, "storage", || {
        mackesd_core::workers::storage::StorageWorker::new(node_id.clone())
    });
    // EXPLORER-1 — the unit_aggregator worker: the daemon spine of the Hero
    // unit explorer (docs/design/unit-explorer.md). Unions three sources into
    // one typed `Unit` stream and publishes `state/units/<node>`: the mesh
    // mirror (peers + `/mesh/leader` + health it already reads), the union of
    // every node's `state/openstack/<node>` mirror (QC-2, deduped by object id +
    // host-tagged, consumed through the Bus read path — never an openstack
    // file), and the surface-gated active LAN scan (EXPLORER-2's producer seam,
    // a no-op today). Publish-on-change + heartbeat, plus the E9
    // `action/units/get-stream` read verb any mesh client can call. Universal
    // (rank 0) like storage/openstack — every node publishes its own unit view,
    // no center. node_id is the mirror `host` stamp + self unit; workgroup_root
    // seeds the peer-directory reader.
    spawn_tiered(sup, worker_names, role_rank, "unit_aggregator", || {
        mackesd_core::workers::unit_aggregator::UnitAggregatorWorker::new(
            node_id.clone(),
            workgroup_root.clone(),
        )
    });
    // WL-FUNC-008 — the service_aggregator worker: the unified service
    // provenance/health view. Merges the three service sources — the published KDC
    // directory (`kdc-services/<host>.json`), the nmap probe inventory
    // (`probe-inventory.json`), and the Explorer's `service → openable-action`
    // enrichment map — into one deduped `ServiceRecord` set with stale-entry TTL
    // age-out, published on `state/services/<node>` for the shell's Services view.
    // Universal (rank 0) like unit_aggregator — every node folds + publishes its own
    // mesh-wide service view (no center). node_id is the mirror `host` stamp;
    // workgroup_root seeds both the directory + inventory readers.
    spawn_tiered(sup, worker_names, role_rank, "service_aggregator", || {
        mackesd_core::workers::service_aggregator::ServiceAggregatorWorker::new(
            node_id.clone(),
            workgroup_root.clone(),
        )
    });
    // MV-5a — the scheduler worker: the placement slice of the no-center
    // scheduler. Drains `action/schedule/place`, folds each node's latest
    // `event/kvm/services` capacity, chooses the target node (healthy pin →
    // most-active → node_id tie-break), and forwards a host-targeted
    // create/run onto `action/vm/lifecycle` / `action/container/lifecycle`
    // (plus the decision to `event/schedule/placements`). Rank-0-default like
    // vm_lifecycle/container (runs everywhere); an interim lowest-node-id
    // single-actor election keeps N nodes from emitting duplicate placements.
    // Failover re-election + etcd desired-state persistence are MV-5b.
    spawn_tiered(sup, worker_names, role_rank, "scheduler", || {
        mackesd_core::workers::scheduler::SchedulerWorker::new(node_id.clone())
    });
    // E12-5b — the session_broker worker: the mackesd side of the E12-5 VDI
    // remote-desktop milestone. Drains `action/vdi/session`, folds each op
    // into the live VDI-session roster (which peer serves which VM to which
    // client + state) via a pure state machine, and — leader-gated —
    // reconciles that roster into the shared roaming-session plane through the
    // injectable SessionStore seam so any peer sees the active sessions.
    // Rank-0-default like scheduler (runs everywhere); the shared leader lock
    // keeps an N-node mesh from multi-writing. The live etcd/Syncthing
    // cross-peer publish is integration-gated (typed error, §7).
    spawn_tiered(sup, worker_names, role_rank, "session_broker", || {
        mackesd_core::workers::session_broker::SessionBrokerWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        )
    });
    // VDI-VM-1 — the console_broker worker: the serving-side half that actually
    // makes a LOCAL KVM VM's console reachable on the mesh. Every VM binds SPICE
    // to 127.0.0.1 (vm_lifecycle's domain XML), so session_broker can track a
    // local-VM session but there is no reachable endpoint to attach frames.
    // For each VDI `Open` naming a VM this node serves, this worker resolves the
    // live console (`virsh domdisplay`), relays that loopback port onto the
    // Nebula overlay with a scoped socat (the compute_expose forward pattern),
    // and publishes the overlay `host:port` back on the session record
    // (`state/vdi/console`, keyed by session id) for the client shell to
    // resolve. Serving-peer-gated (NOT leader-gated: the relay + loopback
    // console are physically on the serving host); runs everywhere like
    // session_broker. Honest-gates (never a fake endpoint) when the VM is off /
    // has no graphics / socat|virsh|overlay is absent — §7.
    spawn_tiered(sup, worker_names, role_rank, "console_broker", || {
        mackesd_core::workers::console_broker::ConsoleBrokerWorker::new(node_id.clone())
    });
    // E12-8 — the session_roaming worker: the roaming + persistence POLICY over
    // the E12-5b session_broker's sessions. Drains `action/vdi/roaming`, folds
    // arrivals / per-VM disconnect policy / monitor layouts, and — leader-gated —
    // makes a user's desktops follow them to any Workstation (reconcile_roaming)
    // and survive disconnect (on_disconnect default KeepRunning; on_node_loss
    // holds reconnectable). Rank-0-default like session_broker (runs everywhere);
    // the shared leader lock keeps an N-node mesh from multi-writing. REUSES the
    // broker's VdiSession + SessionStore; sessions and monitor layouts persist
    // through the replicated workgroup-root stores (MeshSessionStore +
    // MeshLayoutStore), with future etcd-backed stores hidden behind the same
    // seams.
    spawn_tiered(sup, worker_names, role_rank, "session_roaming", || {
        mackesd_core::workers::session_roaming::SessionRoamingWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        )
    });
    // OW-11 (Bus half) — the service_onboard worker: `onboard service-add`
    // reachable over the Bus. Drains `action/onboard/service-add`, runs the
    // EXISTING onboard::service_add engine (plan + the injectable ServiceApply
    // seam — §6 glue), and — leader-gated like session_broker so an N-node
    // mesh answers each request once — publishes the typed result event on
    // `event/onboard/service-add` for the shell's Services flow. Rank-0-default
    // like session_broker (runs everywhere); real applies run over
    // LiveServiceApply, whose typed IntegrationGated is the honest live answer
    // today (§7).
    spawn_tiered(sup, worker_names, role_rank, "service_onboard", || {
        mackesd_core::workers::service_onboard::ServiceOnboardWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        )
    });
    // OW-7 (Bus half) — the spawn_lighthouse_onboard worker: `onboard
    // spawn-lighthouse` reachable over the Bus. Drains
    // `action/onboard/spawn-lighthouse`, runs the EXISTING
    // onboard::spawn_lighthouse engine (plan_spawn + the injectable Provisioner
    // seam — §6 glue), and — leader-gated like service_onboard so an N-node mesh
    // answers each request once — publishes the typed result event on
    // `event/onboard/spawn-lighthouse` for the shell's Spawn Lighthouse flow.
    // Rank-0-default like service_onboard (runs everywhere); real provisions run
    // over LiveProvisioner, whose typed IntegrationGated is the honest live answer
    // today (the live cloud/SSH provision + CA-migrate stays gated, §7).
    spawn_tiered(
        sup,
        worker_names,
        role_rank,
        "spawn_lighthouse_onboard",
        || {
            mackesd_core::workers::spawn_lighthouse_onboard::SpawnLighthouseOnboardWorker::new(
                workgroup_root.clone(),
                node_id.clone(),
            )
        },
    );
    // OW-15 (target-side, day-2) — the onboard_apply worker: the §9-native
    // receiver for the BusApply remote-push transport. Drains
    // `action/onboard/apply` (a signed JobBundle + the claimed issuer) and
    // applies it ONLY when addressed to this node, from a leadership-authorized
    // issuer (the CA `nodes` registry resolves the issuer to a leader-eligible
    // lighthouse identity key), validly signed/fresh/single-use — reusing the
    // pure onboard::remote_push core (allow-listed Action enum, no raw shell —
    // §9). Rank-0 default (any enrolled peer can be a target; each node applies
    // only bundles addressed to it). Publishes the typed observed-state /
    // rejection on `event/onboard/apply`; the live cross-node round-trip is
    // operator/live-gated behind BusApply (§7).
    spawn_tiered(sup, worker_names, role_rank, "onboard_apply", || {
        mackesd_core::workers::onboard_apply::OnboardApplyWorker::new(
            &workgroup_root,
            node_id.clone(),
        )
    });
    // E12-9 — the clipboard_bridge worker: the first of the E12-9 VDI client↔VM
    // bridges. Drains `action/vdi/clipboard`, applies a per-session policy
    // (allow/deny + one-way + a size cap) via the pure relay decision
    // (Forward/Drop/Truncate), and relays each clip into the connected VM desktop
    // through the injectable ClipboardAccess seam (with an echo guard). Clipboard
    // relay is per-session + node-local — every serving node must apply ITS
    // session's clips — so it is NOT leader-gated (unlike session_broker) but is
    // rank-0-default the same way (runs everywhere). The live OS/guest clipboard
    // channel (SPICE/RDP vdagent / wl-clipboard) is integration-gated (typed
    // error, §7); the pure model + relay pipeline ship green behind the seam.
    spawn_tiered(sup, worker_names, role_rank, "clipboard_bridge", || {
        mackesd_core::workers::clipboard_bridge::ClipboardBridgeWorker::new()
    });
    // OV-7.a (v2.6) — health reconciler. Polls each known
    // peer's QNM-Shared heartbeat.json every 5 s, applies the
    // telemetry::health_state_from_age threshold table, writes
    // back into nodes.health, and fires PeerStateChanged on
    // transitions. Closes the gap between live heartbeats and
    // the SQLite column that NebulaStatusService::build_peer_list
    // projects. Spawn order: after HeartbeatWorker so peers
    // have at least one observable heartbeat by the first
    // reconcile tick.
    spawn_tiered(sup, worker_names, role_rank, "health_reconciler", || {
        mackesd_core::workers::health_reconciler::HealthReconcilerWorker::new(
            workgroup_root.clone(),
            db_path.clone(),
            node_id.clone(),
            std::sync::Arc::clone(&nebula_signal_slot),
        )
    });
}

// run_serve extract: mesh gossip/reconcile plumbing workers (sshd_overlay_bind .. connect_firewall).
pub(crate) fn spawn_mesh_plumbing_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    node_id: &String,
    workgroup_root: &PathBuf,
    db_path: &PathBuf,
) {
    use mackesd_core::workers::{
        device_control, firewall_preset::FirewallPresetWorker, fleet_reconcile, job_exec,
        lifecycle_exec, mesh_dns, netstate_apply, presence_watch, router_action, ssh_pubkey_gossip,
        sshd_overlay_bind::SshdOverlayBindWorker, validation_suite, RestartPolicy, Spawn,
    };
    // NF-21.1 — sshd overlay-bind worker. Polls
    // /var/lib/mackesd/nebula/overlay-ip every 5 s; on change,
    // writes the /etc/ssh/sshd_config.d/mackes-mesh.conf drop-in
    // + reloads sshd so the daemon binds to the new overlay
    // address. Quiet no-op on pre-enrollment peers (missing
    // publish file). Replaces mesh_nebula.py::write_sshd_overlay_bind
    // so the Python module can fully retire (DEAD-2.14 plan).
    spawn_tiered(sup, worker_names, role_rank, "sshd_overlay_bind", || {
        SshdOverlayBindWorker::new()
    });
    // SVC-2 (Q60) — SSH pubkey gossip: publish this box's user
    // ed25519 pubkey into <root>/ssh-keys/ and merge every peer's
    // published key into ~/.ssh/authorized_keys (managed block,
    // write-on-change). Syncthing replication is the transport.
    // PD-11 — the lifecycle executor: descriptor-gated container/VM
    // start/stop requests from peers, via replicated request files.
    spawn_tiered(sup, worker_names, role_rank, "lifecycle_exec", || {
        lifecycle_exec::LifecycleExecWorker::new(
            workgroup_root.clone(),
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
        )
    });
    // DEVMGR-8 — the device-control executor: drains this box's replicated
    // fleet/device-control/<self>/ for typed privileged-op requests the
    // Device-Manager surface dispatches (enable/disable, reload module,
    // rescan bus), gates each against this node's own published inventory
    // (L9 rail), executes the FIXED sysfs/ip/modprobe seam, hash-chain audits
    // it, and notifies on failure. Universal (rank 0) like lifecycle_exec.
    spawn_tiered(sup, worker_names, role_rank, "device_control", || {
        device_control::DeviceControlExecWorker::new(
            workgroup_root.clone(),
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
            node_id.clone(),
        )
    });
    // WL-RUN-006 — router-action executor: drains this node's replicated
    // action/router/<self> firewall-edit requests, gates each on a typed-confirm
    // token, applies inside a Vyatta commit-confirm window (auto-revert), and
    // hash-chain audits every edit. Universal (rank 0) like device_control; the
    // live mutation is operator-gated (MDE_ROUTER_ACTION_LIVE=1).
    spawn_tiered(sup, worker_names, role_rank, "router_action", || {
        router_action::RouterActionWorker::new(
            workgroup_root.clone(),
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
            node_id.clone(),
        )
    });
    // PD-13 — presence-transition alerts: offline/online crossings
    // become desktop notifications via the alert_relay pipeline.
    spawn_tiered(sup, worker_names, role_rank, "presence_watch", || {
        let alerts = mackesd_core::workers::alert_relay::default_alerts_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/mde-alerts"));
        presence_watch::PresenceWatchWorker::new(
            workgroup_root.clone(),
            alerts,
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
        )
    });
    // SUBSTRATE-10 — etcd WATCH worker: opens watch streams on /mesh/peers/
    // (a Delete = a keepalive lease expired = a peer dropped) + /mesh/leader
    // (a Put with a new node_id = a leadership handover) and PUSHES instant
    // alerts onto the same alert_relay lane presence_watch uses — no poll,
    // no 5 s reconcile lag. Degrades cleanly off the coordination plane
    // (empty endpoints / etcd unreachable → idle + back off, never panic).
    spawn_tiered(sup, worker_names, role_rank, "etcd_watch", || {
        let alerts = mackesd_core::workers::alert_relay::default_alerts_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/mde-alerts"));
        mackesd_core::workers::etcd_watch::EtcdWatchWorker::new(
            alerts,
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
        )
    });
    // PD-9 / FPG — the reconcile driver: magic-fleet reconcile on a
    // 15-min cadence + immediately on this host's nudge file.
    spawn_tiered(sup, worker_names, role_rank, "fleet_reconcile", || {
        fleet_reconcile::FleetReconcileWorker::new(
            workgroup_root.clone(),
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
        )
    });
    // PLANES-18 — mesh DNS: feed <host>.mesh into resolved +
    // /etc/hosts on every node (rank 0 plumbing).
    spawn_tiered(sup, worker_names, role_rank, "mesh_dns", || {
        mesh_dns::MeshDnsWorker::new(Some(db_path.clone()))
    });
    // PLANES-15 — netstate engine mount: converge the baseline's
    // network desired-state under a rollback checkpoint + overlay
    // self-test (W77/W78), on every node.
    spawn_tiered(sup, worker_names, role_rank, "netstate_apply", || {
        netstate_apply::NetstateApplyWorker::new(
            workgroup_root.clone(),
            Some(db_path.clone()),
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
        )
    });
    // PLANES-19 — overlay-reachability validation suite: every node
    // participates; the leader mints nightly/run-now + writes verdicts.
    spawn_tiered(sup, worker_names, role_rank, "validation_suite", || {
        validation_suite::ValidationSuiteWorker::new(
            workgroup_root.clone(),
            Some(db_path.clone()),
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
            std::path::PathBuf::from(
                mackesd_core::workers::netdata_aggregator::DEFAULT_ROLE_HOST_MARKER,
            ),
        )
    });
    // PLANES-9 — the local job executor (execution-tag gated, W84).
    spawn_tiered(sup, worker_names, role_rank, "job_exec", || {
        job_exec::JobExecWorker::new(
            workgroup_root.clone(),
            node_id
                .strip_prefix("peer:")
                .unwrap_or(&node_id)
                .to_string(),
        )
    });
    spawn_tiered(sup, worker_names, role_rank, "ssh_pubkey_gossip", || {
        ssh_pubkey_gossip::SshPubkeyGossipWorker::new(workgroup_root.clone(), node_id.clone())
    });
    // NF-21.3 — firewall_preset worker. Applies the Nebula
    // firewalld preset (UDP/4242 inbound on all peers; TCP/443
    // inbound additionally on lighthouses) on first tick + on
    // every role-flip via the /var/lib/mackesd/nebula/role.host
    // marker. Idempotent — firewall-cmd's ALREADY_ENABLED is
    // treated as success. Replaces mesh_nebula.py::apply_nebula_firewall_preset
    // so the Python helper can retire (DEAD-2.14 plan).
    spawn_tiered(sup, worker_names, role_rank, "firewall_preset", || {
        FirewallPresetWorker::new()
    });
    // CONNECT-3 — exposure-driven firewall enforcement (additive): opens the
    // policy's ingress ports on the public zone for services bound to this
    // node, so `expose` actually accepts public traffic. Never removes a rule
    // (can't lock out SSH/Nebula). Same supervised shape as the preset worker.
    sup.spawn(Spawn::new(
        mackesd_core::workers::connect_firewall::ConnectFirewallWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::OnFailure,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("connect_firewall".into());
}

// run_serve extract: leader-gated datacenter/scheduler/frontdoor workers (alert_relay .. action).
pub(crate) fn spawn_datacenter_scheduler_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    node_id: &String,
    workgroup_root: &PathBuf,
) {
    use mackesd_core::workers::{RestartPolicy, Spawn};
    // MON-4 (v2.6) — alert relay worker. Polls
    // ~/.local/share/mde/alerts/*.json for events
    // written by mde-alert-emit (MON-3) via Netdata's
    // health_alarm_notify.conf custom-sender hook + fires
    // an FDO desktop notification via notify-send per
    // new event. Deduplicates by deterministic ULID.
    // RestartPolicy::Always since the tick is passive +
    // operator outage detection is the failure-tolerance
    // goal.
    //
    // v6.0 Portal-1 — attach a PortalClient so CRITICAL
    // alerts also navigate Portal-full to the Control
    // (mesh-health) layer. Graceful-degrade: if the session
    // bus or mde-portal aren't running at daemon startup
    // the relay skips the portal call and surfaces the
    // FDO notification alone.
    // DBUS-2: the portal shell IPC is the Bus now. PortalClient is
    // stateless (it appends to action/shell/<verb> per call), so the
    // relay always attaches it — a CRITICAL alert's goto(control) is
    // durable even if mde-portal is down at the time.
    // E4.20 — the portal-era "navigate to Control on CRITICAL" publish was
    // dropped: alerts already surface via `notify-send` → notifyd → the Win10
    // Action Center, so the `action/shell/goto` Bus publish (whose only
    // consumer was the retired portal) is redundant.
    let alert_relay = mackesd_core::workers::alert_relay::AlertRelayWorker::new();
    tracing::info!(
        "alert_relay: PortalClient attached \
             (CRITICAL alerts publish action/shell/goto control)"
    );
    sup.spawn(Spawn::new(alert_relay, RestartPolicy::Always));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("alert_relay".into());

    // INST-11 + INST-12 + INST-13 (v2.7) — fleet upgrade-barrier
    // worker. Runs on every peer; silently no-ops until a
    // `mde-update --coordinate <ver>` writes an intent file into
    // `<mesh-home>/upgrade-intent/`. Then it runs `dnf upgrade
    // mde-core` on its own schedule, marks itself ready, fires
    // `mde-install --yes` once quorum + grace are met, and — when
    // it holds the leader lease — cleans up fully-complete intent
    // files after the +24h grace. No SQLite handle needed: the
    // barrier state lives in the GFS-replicated intent files and
    // the peer roster in the PEERVER peers dir.
    sup.spawn(Spawn::new(
        mackesd_core::workers::upgrade_intent_watcher::UpgradeIntentWatcher::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("upgrade_intent_watcher".into());

    // FARM-AUTO-1 — build-farm orchestrator. Leader-gated; bridges the farm's
    // etcd job lifecycle (FARM-AUTO-3 queue/results) onto the Bus as
    // `event/farm/<jobid>` events so farm activity is visible mesh-wide.
    sup.spawn(Spawn::new(
        mackesd_core::workers::farm_orchestrator::FarmOrchestratorWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("farm_orchestrator".into());

    // DATACENTER-5 — datacenter orchestrator. Leader-gated; samples the DC
    // substrate (DigitalOcean now via doctl; Xen/XAPI + gateway as Phase-0
    // deps land) and publishes `event/dc/<kind>/<id>` so the Workbench
    // Datacenter plane sees hosts/VMs/droplets as first-class mesh state.
    sup.spawn(Spawn::new(
        mackesd_core::workers::datacenter_orchestrator::DatacenterOrchestratorWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("datacenter_orchestrator".into());

    // DATACENTER-7 (audit half) — passive datacenter audit subscriber.
    // Leader-gated; watches the `action/dc/*` Bus lanes and emits one
    // append-only `event/dc/audit/<ulid>` record per request (deduped on
    // ulid), without touching the action handlers — a pure side-observer.
    sup.spawn(Spawn::new(
        mackesd_core::workers::dc_auditor::DcAuditorWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("dc_auditor".into());

    // DATACENTER-6 — passive async job-status tracker. Leader-gated; watches
    // the `action/dc/*` Bus lanes + their `reply/<ulid>` replies and emits one
    // `event/dc/job/<ulid>` event per status transition (pending→ok/error),
    // without touching the action handlers — a pure side-observer.
    sup.spawn(Spawn::new(
        mackesd_core::workers::dc_jobs::DcJobsWorker::new(workgroup_root.clone(), node_id.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("dc_jobs".into());

    // DATACENTER-24 — passive care-and-feeding health checker. Leader-gated;
    // on a 30 s tick probes each configured Xen dom0's SSH reachability, the
    // SUBSTRATE-V2 etcd `/health`, the mesh secret-store helper, the Nebula CA
    // cert expiry, each dom0's VMs for crashes + its pool for degraded hosts,
    // and emits one `event/dc/health/<check>` per check (deduped on status). It
    // also folds each dom0's recent journal tail into the fleet_logs sink for
    // the Datacenter Logs view — all without touching the substrate it
    // watches (a pure side-observer).
    sup.spawn(Spawn::new(
        mackesd_core::workers::dc_health::DcHealthWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("dc_health".into());

    // DATACENTER-23 — scheduled DR backups. Leader-gated; on a coarse (~5 min)
    // tick decides via the pure `due` helper whether at least
    // `MCNF_DR_INTERVAL_SECS` (default daily) have elapsed since the last run,
    // and if so runs `automation/dr/dr-backup.sh` and publishes the outcome to
    // `event/dc/dr/last` ({"status":"ok","path":…} | {"status":"fail",…}). The
    // leader runs exactly one backup per interval mesh-wide.
    sup.spawn(Spawn::new(
        mackesd_core::workers::dr_scheduler::DrSchedulerWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("dr_scheduler".into());

    // DATACENTER-12 (scheduled-snapshot executor) — the missing consumer of
    // the Storage tab's "Save schedule". Leader-gated; reads each SR's latest
    // `event/dc/snap-schedule/<sr>` config off the Bus, and on a coarse
    // (~5 min) tick decides via the pure `due` helper whether each SR is due
    // per its cadence. When due it takes the snapshot by REUSING the existing
    // storage `xe vdi-snapshot` path over the mesh-key SSH (the same
    // `xen_ssh_key`/`xen_dom0s` injection-guarded, dom0-allow-listed contract
    // `ipc::storage_ops` uses), then enforces retention by destroying its OWN
    // (prefix-tagged) oldest snapshots beyond the configured count — never an
    // operator's hand-made snapshot. Emits a run result to
    // `event/dc/snap-schedule-run/<sr>` and alerts on failure via the
    // alert_relay lane. Without this worker the Storage tab's schedule was a
    // config-only stub (config persisted, nothing ever executed it).
    let snap_alerts = mackesd_core::workers::alert_relay::default_alerts_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/mde-alerts"));
    sup.spawn(Spawn::new(
        mackesd_core::workers::dc_snap_scheduler::DcSnapSchedulerWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
            snap_alerts,
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("dc_snap_scheduler".into());

    // DATACENTER-20 — passive promotion tracker. Leader-gated; publishes the
    // version running at each promotion stage (Build→Eagle→DO) to
    // `event/dc/promote/<stage>` so the Workbench Datacenter plane can render
    // the promotion matrix. Build version = newest release RPM (else
    // `git describe`); Eagle/DO are honest `"unknown"` placeholders until
    // those hosts are reachable.
    sup.spawn(Spawn::new(
        mackesd_core::workers::dc_promote::DcPromoteWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("dc_promote".into());

    // ONBOARD-6 — continuous leader election. Renews the
    // <QNM-Shared>/.mackesd-leader.lock lease every 20s so exactly one
    // node always holds leadership (previously only the upgrade watcher
    // touched the lock, and only while an upgrade was in flight, so a
    // steady-state mesh had NO LEADER and every leader-gated surface was
    // dark). Runs on every node; the shared QNM-Shared mount makes them
    // contend for the same lock.
    sup.spawn(Spawn::new(
        mackesd_core::workers::leader_election::LeaderElection::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("leader_election".into());

    // FRONTDOOR-9 — the Copilot codex backend. Spawned on every node so
    // failover is seamless, but LEADER-gated (Q73): only the elected node
    // (the one renewed by `leader_election` above on the shared QNM-Shared
    // lock) drains `action/copilot/ask`, reads the sealed codex API key from
    // the mesh secret-store, runs `codex exec` per ask (external dependency,
    // pulled at runtime — Q100), and replies on `reply/<ulid>`. ASK/SUGGEST
    // ONLY (§9): it spawns the AI subprocess itself but never executes OS
    // actions on the operator's behalf — typed/audited actions are the
    // separate FRONTDOOR-11 worker. Degrades gracefully (logs + an "AI
    // unavailable" reply, never a panic) when codex/key/network is down, so
    // the rest of the Front Door keeps working (Q33).
    //
    // FRONTDOOR-10 (this worker, additional cadences) — the same worker also
    // PROACTIVELY publishes (a) a compact Copilot STATUS to
    // `state/copilot/status` on a cheap cadence (so the Front Door's Copilot
    // tile — left a plain launcher by FD-4 because no topic existed — renders
    // ready/thinking/offline), and (b) on a MODERATE leader-only timer, a
    // ranked set of HIGH-IMPACT/HIGH-CONFIDENCE suggestions to
    // `action/copilot/suggestions` for the GUI to render inline (Q7/Q61).
    // Suggestions are PROPOSALS (FD-12 typed `ActionProposal`s) — never
    // executed here, never published to FD-11's `action/exec/request` (§9).
    sup.spawn(Spawn::new(
        mackesd_core::workers::copilot::CopilotWorker::new(workgroup_root.clone(), node_id.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("copilot".into());

    // FRONTDOOR-11 — the typed action worker (the execution half of the
    // confirm gate, Q17 + Q26). Spawned on every node so failover is seamless,
    // but LEADER-gated (Q73): only the elected node drains
    // `action/exec/request` and acts, so a multi-node mesh executes + audits
    // each action exactly once. It accepts a TYPED ActionRequest enum (an
    // allowlisted KIND + typed params — NEVER a command string; §9 forbids a
    // raw-shell channel) and maps each allowlisted KIND onto an EXISTING verb:
    // the first cut allowlists `service_lifecycle`, dispatched via the PD-11
    // `lifecycle` verb (a typed request the target's own `lifecycle_exec`
    // validates against its live probe and runs locally — no push, no shell).
    // Every action is hash-chain audited via the existing events plane (§8),
    // and an unknown/disallowed action degrades to a typed rejection, never a
    // panic (Q33).
    sup.spawn(Spawn::new(
        mackesd_core::workers::action::ActionWorker::new(workgroup_root.clone(), node_id.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("action".into());
}

// run_serve extract: Workstation broker/terminal workers (mesh_mount, pty_broker).
pub(crate) fn spawn_broker_terminal_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    workgroup_root: &PathBuf,
) {
    // FILEMGR-5 — the mesh-mount worker owns the sshfs mount lifecycle over
    // the Nebula overlay for the Files surface (design `file-manager-full.md`
    // locks 11/13/15/17): it drains `action/mesh-mount/<host>` (typed verb —
    // mount home / escalate to `/` / unmount), holds the node-sealed shared
    // mesh SSH key (FILEMGR-6), and publishes `state/mesh-mount/*` with
    // idle-unmount + reconnect-backoff + frozen-mount recovery. The live
    // sshfs/fusermount impl is integration-gated behind the injectable
    // `MountBackend` seam (§9 — no raw shell in the action layer; §7 — it
    // returns an honest typed error headless, never a faked mount). A desktop
    // feature (Workstation tier); idles gracefully with no mount requests.
    spawn_tiered(sup, worker_names, role_rank, "mesh_mount", || {
        let runtime_base = mackesd_core::workers::mesh_mount::resolve_runtime_base();
        let repo_dir = mackesd_core::ipc::secret_store::repo_root();
        mackesd_core::workers::mesh_mount::MeshMountWorker::new(
            runtime_base,
            repo_dir,
            workgroup_root.clone(),
        )
    });

    // TERM-7 — the mesh PTY-broker worker owns the remote-shell lifecycle
    // over the Nebula overlay for the mde-term-egui terminal surface (design
    // `mesh-terminal.md`): it drains `action/pty/<peer>` (typed verb —
    // open/write/resize/close, each carrying the client-minted session id),
    // opens a real remote shell via `ssh -tt` on the node-sealed shared mesh
    // SSH key (FILEMGR-6, reused from mesh_mount), and publishes an append
    // log on `state/pty/<id>` (base64 output chunks + the terminal exit) with
    // idle-reap + dead-session reap. The live ssh impl is integration-gated
    // behind the injectable `PtyBackend` seam (§9 — a typed argv, no
    // shell-string injection; §7 — it returns an honest typed Gated/
    // Unreachable state headless, never a faked session). A desktop feature
    // (Workstation tier); idles gracefully with no pty requests on a headless
    // box.
    spawn_tiered(sup, worker_names, role_rank, "pty_broker", || {
        let runtime_base = mackesd_core::workers::pty_broker::resolve_runtime_base();
        let repo_dir = mackesd_core::ipc::secret_store::repo_root();
        mackesd_core::workers::pty_broker::PtyBrokerWorker::new(
            runtime_base,
            repo_dir,
            workgroup_root.clone(),
        )
    });
}

// run_serve round-3: the browser-worker spawn group (bookmarks + adfilter +
// browser_policy + the BROWSER-DD-* CEF workers, now that arch-7 moved those
// workers into mde-browser-workers, re-exported via workers/mod.rs). Extracted
// VERBATIM — identical spawn order + `worker_names.push(...)` registrations +
// role gates, so the WORKER_REGISTRY census + the ARCH-5 drift guard
// (`worker_spawns_and_the_census_do_not_drift`) stay byte-identical.
pub(crate) fn spawn_browser_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    node_id: &String,
    workgroup_root: &PathBuf,
) {
    // BOOKMARKS-2 — the mesh-synced bookmarks worker (design
    // `mesh-bookmarks.md` locks Q17-Q24/Q90/Q91): it drains
    // `action/bookmarks/*` (add/edit/move/delete/add-folder/rename — minting
    // real mde-bookmarks CRDT ops), writes this node's append-only op segment
    // into the encrypted Syncthing share (`workgroup_root`, the same
    // /mnt/mesh-storage substrate ssh-gossip/chat use), replay-merges every
    // peer's segment into one converged collection, snapshot+prunes for
    // bounded growth, and publishes `state/bookmarks/*`. Offline-first: edits
    // apply to a node-local durable store immediately and auto-resume when the
    // share reappears. No external transport to fake (§7) — the honest gate is
    // `shared_root_writable`, published as an offline SyncStatus, never a faked
    // converge. A desktop feature (Workstation tier); idles gracefully with no
    // requests on a headless box.
    spawn_tiered(sup, worker_names, role_rank, "bookmarks", || {
        let local_root = mackesd_core::workers::bookmarks::resolve_local_root();
        let user = mackesd_core::workers::bookmarks::resolve_user();
        mackesd_core::workers::bookmarks::BookmarksWorker::new(
            node_id.clone(),
            user,
            local_root,
            workgroup_root.clone(),
        )
    });

    // BOOKMARKS-7 — the mesh-wide ad-blocker worker (the Syncthing replication +
    // leader compile behind the pure mde-adblock engine). Every node writes its
    // own serialized filter-store blob into the encrypted Syncthing share
    // (`workgroup_root`, the same /mnt/mesh-storage substrate bookmarks/ssh-gossip
    // use) and LWW-merges every peer's into one converged store; the elected
    // leader compiles that store into the shared engine blob the mde-web-preview
    // browser reads + refreshes the enabled lists from an airgap-safe local mirror
    // (honest Staleness fallback, never fabricated — §7). Drains
    // action/adfilter/{allow,block} into the mesh-synced per-site allowlist
    // (block-on-by-default) + publishes state/adfilter/<node>. Offline-first: the
    // node-local store survives a down share, and nothing is written into a bare
    // unprovisioned mount (`shared_root_writable`). A desktop feature (Workstation
    // tier); idles gracefully on a headless box with no browser + no requests.
    spawn_tiered(sup, worker_names, role_rank, "adfilter", || {
        let local_root = mackesd_core::workers::adfilter::resolve_local_root();
        mackesd_core::workers::adfilter::AdfilterWorker::new(
            node_id.clone(),
            local_root,
            workgroup_root.clone(),
        )
    });

    // BOOKMARKS-8 — the mesh-wide browser/ad-blocker POLICY worker (fleet
    // governance ENFORCED mesh-side, not just in the UI). Every node writes its
    // own operator-authored policy doc into the encrypted Syncthing share
    // (`workgroup_root`, the same substrate the adfilter/bookmarks workers use)
    // and converges on the newest-authored doc mesh-wide; it folds that doc for
    // THIS node's deployment role and enforces at the browser launch/spawn seam
    // — draining action/browser/{launch,navigate,set-adblock} to refuse a
    // launch on a disallowed role, inject the forced ad-blocker + URL allowlist
    // + custom lists on a granted launch, and reject out-of-policy navigate /
    // adblock-off actions. Draining action/browser-policy/set authors the fleet
    // policy. Disable stops the browser-data sync + hides the surface but
    // retains the node-local data (no destructive wipe). Publishes
    // state/browser-policy/<node> for the Workbench fleet view. Offline-first:
    // the node-local doc + data survive a down share, and nothing is written
    // into a bare unprovisioned mount (`shared_root_writable`). A desktop-
    // governance feature (Workstation tier); idles gracefully on a headless box.
    spawn_tiered(sup, worker_names, role_rank, "browser_policy", || {
        let local_root = mackesd_core::workers::browser_policy::resolve_local_root();
        let role = mackesd_core::worker_role::role_name(role_rank).to_string();
        mackesd_core::workers::browser_policy::BrowserPolicyWorker::new(
            node_id.clone(),
            role,
            local_root,
            workgroup_root.clone(),
        )
    });

    // BROWSER-DD-6 — Browser passkey/WebAuthn ceremony owner. Browser
    // publishes strict ceremony metadata to `action/browser/passkey`; this
    // worker validates RP/origin/challenge shape, persists pending
    // challenges locally, mirrors them into the Syncthing-backed workgroup
    // root, and publishes honest pending/error state without minting fake
    // credentials. A Workstation-tier browser security feature; it idles on
    // headless boxes with no Browser publishes.
    spawn_tiered(sup, worker_names, role_rank, "browser_passkeys", || {
        let local_root = mackesd_core::workers::browser_passkeys::resolve_local_root();
        mackesd_core::workers::browser_passkeys::BrowserPasskeysWorker::new(
            node_id.clone(),
            local_root,
            workgroup_root.clone(),
        )
    });

    // BROWSER-DD-7 — the browser session-sync owner. The shell publishes
    // deduped `action/browser/session-sync` snapshots for tabs/settings/
    // downloads/speed-dial; this worker validates those restore-compatible
    // JSON bodies, persists the latest local copy, and mirrors it into the
    // Syncthing-backed workgroup root at
    // browser-session-sync/<host>/latest.json. The file body stays the exact
    // Browser snapshot shape so startup restore consumes it directly. A
    // Workstation-tier browser feature; it idles on headless boxes with no
    // Browser publishes and never writes into a missing canonical share.
    spawn_tiered(sup, worker_names, role_rank, "browser_session_sync", || {
        let local_root = mackesd_core::workers::browser_session_sync::resolve_local_root();
        mackesd_core::workers::browser_session_sync::BrowserSessionSyncWorker::new(
            node_id.clone(),
            local_root,
            workgroup_root.clone(),
        )
    });

    // BROWSER-DD-11 — Browser read-aloud/TTS owner. The shell publishes
    // bounded `action/browser/read-aloud` page-text requests; this worker
    // validates them, invokes the configured offline TTS command when present
    // (`MDE_BROWSER_TTS_COMMAND` / `MDE_TTS_COMMAND`), and publishes honest
    // spoken/unavailable/error state. A Workstation-tier browser feature; it
    // idles on headless boxes with no Browser publishes.
    spawn_tiered(sup, worker_names, role_rank, "browser_read_aloud", || {
        mackesd_core::workers::browser_read_aloud::BrowserReadAloudWorker::new(node_id.clone())
    });

    // BROWSER-DD-11 — Browser voice-command/dictation STT owner. The shell
    // publishes active-tab context to `action/browser/voice-command`; this
    // worker validates it, invokes the configured offline STT/capture command
    // when present (`MDE_BROWSER_STT_COMMAND` / `MDE_STT_COMMAND`), emits a
    // bounded transcript event, and publishes honest unavailable/error state.
    spawn_tiered(
        sup,
        worker_names,
        role_rank,
        "browser_voice_command",
        || {
            mackesd_core::workers::browser_voice_command::BrowserVoiceCommandWorker::new(
                node_id.clone(),
            )
        },
    );

    // BROWSER-DD-12 — Browser external-protocol owner. The shell refuses to
    // navigate `mailto:`/`magnet:` URLs and publishes
    // `action/browser/protocol`; this worker validates those handoffs and
    // emits retained route status/events for Email/Transfers owners.
    spawn_tiered(sup, worker_names, role_rank, "browser_protocol", || {
        mackesd_core::workers::browser_protocol::BrowserProtocolWorker::new(node_id.clone())
    });

    // BROWSER-DD-12 — Browser platform-share owner. The shell publishes
    // `action/browser/share` for Peer/Email/QR platform targets; this worker
    // validates those handoffs and emits retained route status/events
    // without faking downstream delivery.
    spawn_tiered(sup, worker_names, role_rank, "browser_share", || {
        mackesd_core::workers::browser_share::BrowserShareWorker::new(node_id.clone())
    });

    // BROWSER-DD-12 — Browser private offline/mesh translation owner. The
    // shell publishes bounded page text to `action/browser/translate`; this
    // worker validates the private-only request, invokes the configured
    // local/mesh translation command when present
    // (`MDE_BROWSER_TRANSLATE_COMMAND` / `MDE_TRANSLATE_COMMAND`), emits a
    // bounded result event, and publishes honest unavailable/error state.
    spawn_tiered(sup, worker_names, role_rank, "browser_translate", || {
        mackesd_core::workers::browser_translate::BrowserTranslateWorker::new(node_id.clone())
    });

    // BROWSER-DD-12 — Browser offline/mesh cache owner. The shell publishes
    // explicit private page snapshots to `action/browser/offline-cache`; this
    // worker validates them, writes a local durable cache record, and mirrors
    // it into the Syncthing-backed workgroup root. The browser helper remains
    // no-store; the cache is daemon-owned and private to the mesh.
    spawn_tiered(
        sup,
        worker_names,
        role_rank,
        "browser_offline_cache",
        || {
            let local_root = mackesd_core::workers::browser_offline_cache::resolve_local_root();
            mackesd_core::workers::browser_offline_cache::BrowserOfflineCacheWorker::new(
                node_id.clone(),
                local_root,
                workgroup_root.clone(),
            )
        },
    );

    // BROWSER-DD-12 — Browser CEF security-update status owner. It watches
    // the packaged fast-update manifest plus the active CEF runtime and
    // publishes an honest current/missing/mismatch posture for the
    // independent browser-engine update path.
    spawn_tiered(
        sup,
        worker_names,
        role_rank,
        "browser_security_update",
        || {
            mackesd_core::workers::browser_security_update::BrowserSecurityUpdateWorker::new(
                node_id.clone(),
            )
        },
    );

    // BROWSER-DD-12 — Browser idle-tab suspend owner. The shell already
    // stops inactive helpers and publishes `action/browser/tab-suspend`;
    // this worker validates those handoffs and publishes retained
    // suspend status/events for diagnostics and future orchestration.
    spawn_tiered(sup, worker_names, role_rank, "browser_tab_suspend", || {
        mackesd_core::workers::browser_tab_suspend::BrowserTabSuspendWorker::new(node_id.clone())
    });
}

// run_serve extract: desktop/media discovery + seat input workers (seat_remote_input, desktop_sources, media_sources).
pub(crate) fn spawn_desktop_discovery_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    node_id: &String,
    workgroup_root: &PathBuf,
) {
    // KDC-MESH-6 — seat-side phone remote-input consumer. `kdc_host`
    // publishes sanitized `action/seat/remote-input` rows after the paired
    // phone/protocol checks; this worker owns the local injection seam and
    // honest unavailable/error state when no uinput helper is configured.
    spawn_tiered(sup, worker_names, role_rank, "seat_remote_input", || {
        mackesd_core::workers::seat_remote_input::SeatRemoteInputWorker::new(node_id.clone())
    });

    // CHOOSER-1 — the desktop-source discovery aggregator (design
    // `desktop-chooser.md` §Architecture, locks 5/14): collects every
    // desktop source — mesh-peer advertised (the replicated peers plane's
    // RemoteAccess/vms rows), mDNS RDP/VNC/Spice on the local LAN (the
    // mdns_relay machinery + its anti-loop TXT guard), local KVM guest
    // consoles (the MV-3 LibvirtBackend seam), and manually-added
    // endpoints — merges them into ONE deduped roster and publishes
    // `state/desktops/sources` for the Chooser surface (CHOOSER-2).
    // Drains typed `action/desktops/{add-source,remove-source,refresh}`
    // verbs (§9). Live KVM enumeration is honestly gated (a typed Gated
    // lane status when virsh is absent, §7 — never a faked source);
    // reachability derives from roster presence / VM power state, never
    // a blocking probe. A desktop feature (Workstation tier); idles
    // gracefully on a headless box.
    spawn_tiered(sup, worker_names, role_rank, "desktop_sources", || {
        let store_root = mackesd_core::workers::desktop_sources::resolve_store_root();
        mackesd_core::workers::desktop_sources::DesktopSourcesWorker::new(
            node_id.clone(),
            workgroup_root.clone(),
            store_root,
        )
    });

    // MEDIA-14 — the mesh media-source discovery aggregator (design
    // `mesh-media-player.md`, row 26 "Mesh discovery"): folds two lanes into
    // ONE deduped roster and publishes `state/media/sources` for the
    // mde-media Sources panel (MEDIA-8). Lane 1 (mesh-registry) reads the
    // replicated peers plane's `descriptors.media` Jellyfin/DLNA rows + each
    // peer's `descriptors.mesh_fs` file share — the SAME plane desktop_sources
    // reads, no new advertisement channel (§6 glue). Lane 2 (mDNS) browses
    // `_jellyfin._tcp` on the local LAN via the mdns_relay machinery + its
    // anti-loop TXT guard. Reachability derives from roster presence / peer
    // health, never a blocking probe; music-only services (navidrome/mpd) are
    // honestly excluded (mde-music's domain), and SSDP-only DLNA is surfaced
    // as a `gated:` mDNS-lane note rather than faked (§7). A desktop feature
    // (Workstation tier); idles gracefully on a headless box.
    spawn_tiered(sup, worker_names, role_rank, "media_sources", || {
        mackesd_core::workers::media_sources::MediaSourcesWorker::new(
            node_id.clone(),
            workgroup_root.clone(),
        )
    });
}

// run_serve extract: fleet compute/virt/network-assessment workers (voice_provision .. voip_rtt); owns fw_host.
pub(crate) fn spawn_fleet_compute_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    deploy_class: mackesd_core::worker_role::DeployClass,
    node_id: &String,
    workgroup_root: &PathBuf,
    db_path: &PathBuf,
) {
    use mackesd_core::workers::{RestartPolicy, Spawn};
    // VOIP-GW-3 — the leader-gated voice_provision worker. Spawned on every
    // node so failover is seamless, but LEADER-gated internally (lock 7):
    // only the elected node provisions per-node Vitelity sub-accounts, seals
    // each node's SIP creds to its per-node key in the mesh secret store,
    // reconciles Vitelity ⇄ roster idempotently + rate-limited, and holds
    // the master API key (never distributed). Each node's reg-state is
    // published to `state/voice/<node>` for the Voice panel fleet board
    // (VOIP-GW-5). The live Vitelity transport is integration-gated (a typed
    // error), never faked — a fresh mesh with no sealed master key simply
    // shows every node `Provisioning` rather than a fake online (§7).
    sup.spawn(Spawn::new(
        mackesd_core::workers::voice_provision::VoiceProvisionWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("voice_provision".into());

    // PRINT-2..PRINT-6 + PRINT-8 (v5.0.0) — auto CUPS print
    // sharing + sync. Spawned on headless + full; SKIPPED on
    // lighthouse (routing-only, no printers — Q8 lock). The
    // profile is read from the installed-profile marker
    // `mde-install` writes; missing marker → assume a printing
    // profile (full/headless) and spawn. The worker itself is a
    // silent no-op without cups/lpadmin, so an over-spawn on a
    // box that happens to lack cups is harmless.
    let print_profile = std::fs::read_to_string("/var/lib/mde/installed-profile")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if print_profile != "lighthouse" {
        sup.spawn(Spawn::new(
            mackesd_core::workers::cups_sync::CupsSyncWorker::new(),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("cups_sync".into());
    } else {
        tracing::info!("cups_sync: skipped (lighthouse profile)");
    }

    // FWMON-2..4 (v5.0.0) — firewall-denied event monitor.
    // Reads kernel journal entries logged by firewalld's
    // LogDenied=all setting (enabled by birthright's
    // apply_firewall_log_denied step), filters overlay +
    // established traffic, appends denials to
    // <mesh-storage>/firewall/<host>.jsonl, and fires a Bus
    // alert when one source crosses the threshold.
    let fw_host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| node_id.clone());
    sup.spawn(Spawn::new(
        mackesd_core::workers::firewall_monitor::FirewallMonitorWorker::new(fw_host.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("firewall_monitor".into());

    // NOTIFY-SRC — SELinux AVC denials → the security alert lane. Without
    // this the Alert Center never showed SELinux alerts (no source published
    // them). auditd captures AVCs to audit.log, so the worker scrapes them
    // via `ausearch --checkpoint` and publishes distinct denials to
    // fleet/sec/selinux/<host>; the NOTIFY-DIST-2 mirror federates them.
    sup.spawn(Spawn::new(
        mackesd_core::workers::selinux_monitor::SelinuxMonitorWorker::new(fw_host.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("selinux_monitor".into());

    // VIRT-1 (v5.0.0) — unified KVM + Podman compute inventory.
    // Polls virsh + podman every 10 s; the per-peer inventory bus
    // publish (`compute/inventory/<peer-nebula-addr>`) is on-change +
    // a 60 s heartbeat per BUS-RUN-FULL-1 (docs/DECISIONS.md ADR-0005)
    // — the cross-node fleet view reads the replicated
    // compute-inventory.json file, the bus topic's only consumer is
    // this node's own Workloads source. Silent no-op on peers without
    // virsh/podman (lighthouse, container-stripped). The nebula
    // address is auto-detected from the local nebula1 interface at
    // tick time (empty hint = runtime detect).
    sup.spawn(Spawn::new(
        mackesd_core::workers::compute_registry::ComputeRegistryWorker::new(
            fw_host.clone(),
            String::new(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("compute_registry".into());

    // ROUTER-3/4 — per-node, always-on router-registry: discover the node's
    // primary router/firewall (lowest-metric default route + gateway MAC),
    // cred-match `router/<mac>` + Vyatta `show version` fingerprint, and
    // publish a RouterEntry to mesh/devices/router/<mac> + the QNM-Shared
    // <host>/router-registry.json. Unconditional (any node may sit behind a
    // router); a node with no default route is a safe no-op.
    sup.spawn(Spawn::new(
        mackesd_core::workers::router_registry::RouterRegistryWorker::new(
            node_id.clone(),
            fw_host.clone(),
        )
        .with_mount(workgroup_root.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("router_registry".into());

    // MEDIA-7 — register the navidrome/media service into the mesh service
    // registry. Capability-gated via runs_in("navidrome", deploy_class): it
    // runs ONLY on a Lighthouse_Media node (MEDIA-1's Capability::Media) and
    // is absent everywhere else. Publishes its registration (with a
    // per-instance health field) to the per-peer Bus topic
    // mesh/services/media/<peer> + the replicated QNM-Shared plane
    // <host>/media-registry.json — the same registry plane the other
    // published services use. The .with_mount honors --workgroup-root so the
    // worker writes where the registry readers look.
    if mackesd_core::worker_role::runs_in("navidrome", deploy_class) {
        sup.spawn(Spawn::new(
            mackesd_core::workers::media_registry::MediaRegistryWorker::new(
                node_id.clone(),
                fw_host.clone(),
            )
            .with_mount(workgroup_root.clone()),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("media_registry".into());

        // MEDIA-pkg-2 — self-heal the Navidrome systemd unit (restart-if-down,
        // re-provision-if-missing via the RPM-shipped setup-media-navidrome).
        sup.spawn(Spawn::new(
            mackesd_core::workers::navidrome_supervisor::NavidromeSupervisor::new(),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("navidrome_supervisor".into());
    }

    // MEDIA-15 — the mesh media server + DLNA/UPnP + aggregation (design
    // `mesh-media-player.md`, rows 27 "Mesh library" + 30 "Server role"):
    // the PRODUCER half MEDIA-14 discovers + MEDIA-8 renders. Scans this
    // node's chosen shared folders into a `media-library.json` share
    // manifest written to the replicated QNM-Shared plane
    // (<host>/media-library.json — the SAME plane media-registry.json rides,
    // no new channel), binds the mesh HTTP media server on MESH_MEDIA_PORT
    // (9600) so the localhost descriptor probe folds `mde-media` into this
    // peer's descriptors.media and peers' MEDIA-14 find it, and serves a
    // DLNA/UPnP MediaServer (device description + DIDL-Lite; the SSDP
    // multicast announce is the honestly-gated live leg — §7). Reads every
    // peer's manifest off the plane + folds them into ONE deduped, per-node-
    // attributed mesh library on `state/media/library` for the MEDIA-8
    // Library panel. A desktop feature (Workstation tier); keyed by the
    // hostname (fw_host) like media_registry so its manifest lands on the
    // same replicated <host>/ dir the aggregators read. Idles gracefully on
    // a headless box (empty share, empty library).
    spawn_tiered(sup, worker_names, role_rank, "media_server", || {
        mackesd_core::workers::media_server::MediaServerWorker::new(
            node_id.clone(),
            fw_host.clone(),
            workgroup_root.clone(),
        )
    });

    // APPS-LIVE-1 — apps_running: mirror this node's set of currently-
    // running launchable apps to <QNM-Shared>/<host>/running-apps.json
    // every 10 s so every node's Applications-menu launcher can badge each
    // entry with a live "running on <host>" indicator (same replicated
    // plane as compute-inventory.json; the bus is per-node). Detects via
    // process ↔ .desktop match — root reads every /proc/<pid>/cmdline, so
    // no per-seat compositor probe is needed. The `.desktop` scan root
    // mirrors the apps aggregator's home.
    let apps_running_home =
        std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    sup.spawn(Spawn::new(
        mackesd_core::workers::apps_running::AppsRunningWorker::new(
            fw_host.clone(),
            apps_running_home,
        )
        // Write to the SAME resolved root the apps responder reads from
        // (honors a `--workgroup-root` override) — otherwise the worker would
        // publish under the default root while the reader looked elsewhere and
        // no app ever got badged.
        .with_mount(workgroup_root.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("apps_running".into());

    // APPLAUNCH-5 — apps_installed: mirror this node's INSTALLED .desktop
    // set to <QNM-Shared>/<host>/apps-installed.json every 60 s so the
    // Front Door's Mesh filter can answer a focused peer's app set on
    // demand (action/apps/peer-list) by reading the replicated file
    // locally — a slow/dead peer never blocks the UI (lazy-mesh). Same
    // replicated plane + scan root as apps_running; writes to the resolved
    // workgroup_root so the responder reads what the worker publishes.
    let apps_installed_home =
        std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    sup.spawn(Spawn::new(
        mackesd_core::workers::apps_installed::AppsInstalledWorker::new(
            fw_host.clone(),
            apps_installed_home,
        )
        .with_mount(workgroup_root.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("apps_installed".into());

    // VIRT-5 (v5.0.0) — VM Nebula cert signing via Bus. Every peer
    // spawns the worker; only the CA peer (presence of
    // ~/.config/mde/nebula/ca.key) actually signs + replies, the
    // others advance the cursor silently. compute_provision
    // (VIRT-6) publishes to `action/compute/cert-sign-request`
    // and awaits the reply via rpc::await_reply with the 30 s
    // rpc::DEFAULT_RPC_TIMEOUT, retrying once before marking VM
    // creation failed (per VIRT-5 acceptance bullet 4).
    sup.spawn(Spawn::new(
        mackesd_core::workers::cert_authority::CertAuthorityWorker::new(),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("cert_authority".into());

    // VIRT-7 (v5.0.0) — per-network firewalld port forwarding.
    // Each peer subscribes to its own `compute/expose/<addr>` +
    // `compute/unexpose/<addr>` topics and applies firewall-cmd
    // rich rules per selected network. WAN zone is auto-detected
    // at startup via nmcli + firewall-cmd. Publishes the active
    // rule set to `compute/exposed/<addr>` for the Workbench.
    // Silent no-op on lighthouse / container-stripped peers
    // without firewall-cmd.
    sup.spawn(Spawn::new(
        mackesd_core::workers::compute_expose::ComputeExposeWorker::new(),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("compute_expose".into());

    // VIRT-8.a (v5.0.0) — cold VM migration source-side. Each
    // peer drains `action/compute/migrate`; when own nebula IP
    // == request.source_peer, runs the shutdown→rsync→publish
    // migrate-ready→undefine flow over the Nebula overlay.
    // Target-side handler (VIRT-8.b) ships with VIRT-6
    // compute_provision and subscribes to
    // `event/compute/migrate-ready`.
    sup.spawn(Spawn::new(
        mackesd_core::workers::compute_migrate::ComputeMigrateWorker::new(),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("compute_migrate".into());

    // VIRT-21 (v5.0.0) — compute_event_toast. Subscribes to every
    // compute/event/<peer> topic and raises an FDO desktop toast on
    // VM start/stop/crash so fleet lifecycle changes surface without
    // keeping mde-virtual open.
    sup.spawn(Spawn::new(
        mackesd_core::workers::compute_event_toast::ComputeEventToastWorker::new(),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("compute_event_toast".into());

    // VIRT-6 (v5.0.0) — compute_provision. Drains this peer's
    // `compute/create/<addr>` topic: ensures the mde-vms pool,
    // allocates a per-peer /24 VM IP, runs requester-side
    // nebula-cert keygen + the cert-sign RPC, builds the NoCloud
    // seed, virt-installs the VM (with virtiofs MeshFS share when
    // requested + mounted), acks on compute/create-ack/<ulid>, and
    // fires an immediate inventory publish. workgroup_root + node_id
    // locate this peer's nebula-bundle.json for the guest
    // lighthouse roster.
    sup.spawn(Spawn::new(
        mackesd_core::workers::compute_provision::ComputeProvisionWorker::new(
            fw_host.clone(),
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("compute_provision".into());

    // XCP-3 — the A-plane provision flow. Drains
    // `action/provision/spawn` and, for each request, drives the
    // mackes-xcp Hypervisor layer over xe-over-SSH:
    // `clone MDE-VM-golden → set_identity_seed (the fresh cloud-init
    // seed: MDE-VM-<name> hostname, op key, regen host keys +
    // machine-id) → start → resolve IP`, acking on
    // `action/provision/spawn-ack/<ulid>`. This is the runtime caller
    // that makes set_identity_seed reachable — a provisioned VM
    // actually gets its identity seed. Idles cleanly on a node with no
    // dom0 configured (MCNF_XEN_DOM0S empty → a clean error-ack); the
    // dom0 allow-list is single-sourced from the datacenter env config.
    sup.spawn(Spawn::new(
        mackesd_core::workers::xcp_provision::XcpProvisionWorker::new(),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("xcp_provision".into());

    // MESH-A-1 (v5.0.0) — per-peer network assessment. Collects
    // the 9 items (docs/design/v6.0-mde-portal.md §7.1) hourly +
    // writes ~/.local/share/mde/netassess/<host>/<iso>-<hash>.json
    // with a 30-day rolling trim. Shell-outs degrade to None when
    // a tool is absent (headless / air-gapped peers).
    if let Some(data_dir) = dirs::data_dir() {
        let netassess_base = data_dir.join("mde").join("netassess");
        sup.spawn(Spawn::new(
            mackesd_core::workers::netassess::NetAssessWorker::new(fw_host.clone(), netassess_base)
                .with_mesh_context(workgroup_root.clone(), node_id.clone(), db_path.clone()),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("netassess".into());

        // MESH-A-4.c.2 (v5.0.0) — surrounding-host discovery worker.
        // Sweeps the LAN (mDNS + ARP-MAC + OUI) every 10 min and
        // writes a per-peer snapshot under
        // ~/.local/share/mde/surrounding/<host>/ (mesh-synced;
        // every peer reads the union per R8-Q13).
        let surrounding_base = data_dir.join("mde").join("surrounding");
        sup.spawn(Spawn::new(
            mackesd_core::workers::surrounding_worker::SurroundingWorker::new(
                fw_host,
                surrounding_base,
            ),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("surrounding_hosts".into());

        // MESH-A-5.2 (v5.0.0) — mesh-coordinated firewall DROP:
        // reconciles firewalld source-DROP rules against the
        // mesh-synced Blocked-host consensus every minute.
        sup.spawn(Spawn::new(
            mackesd_core::workers::mesh_firewall::MeshFirewallWorker::new(
                data_dir.join("mde").join("surrounding"),
            ),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("mesh_firewall".into());

        // VOIP-4.b (v5.0.0) — broadcast this peer's Vitelity-link RTT to
        // voip/link-rtt/<peer> every 60s for the dialer route override.
        sup.spawn(Spawn::new(
            mackesd_core::workers::voip_rtt_worker::VoipRttWorker::new(),
            RestartPolicy::Always,
        ));
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push("voip_rtt".into());
    } else {
        tracing::warn!("netassess: no XDG data dir; skipping network assessment worker");
    }
}

// run_serve extract: probe/observability/relay workers (probe .. lighthouse_probe).
pub(crate) fn spawn_probe_observability_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    node_id: &String,
    workgroup_root: &PathBuf,
    db_path: &PathBuf,
    daemon_cfg: &mackesd_core::config::daemon::MackesdConfig,
) {
    use mackesd_core::workers::{RestartPolicy, Spawn};
    use std::sync::Arc;
    // EPIC-MESH-PROBE (MESH-PROBE-4) — scheduled two-tier nmap
    // probe worker. Resolves mesh-peer overlay IPs, scans them
    // (fast 60s / deep 10min), writes this peer's
    // probe-inventory.json into mesh-home, and announces
    // probe/changed on the Bus when the inventory changes. The
    // `mackesd probe scan/refresh` CLI shares the same engine.
    sup.spawn(Spawn::new(
        mackesd_core::workers::probe::ProbeWorker::new(workgroup_root.clone(), node_id.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("probe".into());

    // SUBAUDIT-D2 — hardware-probe producer. Gathers this node's
    // PeerProbe (PCI/USB/kernel/power) + writes it to the replicated
    // directory so every peer's Workbench Hardware panel renders the
    // fleet. Was never built — the panel was permanently empty.
    spawn_tiered(sup, worker_names, role_rank, "hardware_probe", || {
        mackesd_core::workers::hardware_probe::HardwareProbeWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        )
    });

    // E12-19 (Construct host controls) — host_state. Mirrors this node's seat
    // snapshot (published by the shell) to state/host/<node>/seat for the
    // Workbench + remote peers, and authorizes remote typed verbs on
    // action/host/<node>/verb behind the allowlist + safety interlocks
    // (never-black-the-last-console, leader-aware power, two-phase confirm),
    // forwarding an approved verb to the shell's local apply lane. Runs on
    // every node.
    sup.spawn(Spawn::new(
        mackesd_core::workers::host_state::HostStateWorker::new(
            workgroup_root.clone(),
            node_id.clone(),
        ),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("host_state".into());

    // SURFACE-3 — the per-node surface_enable worker. On a recognised
    // Microsoft Surface it drains action/hardware/surface/<node>/enable
    // (the Install tab's activate + MOK request), activates iptsd +
    // applies the per-model config, walks the guided MOK enrollment
    // (typed-armed reboot, honest firmware copy), and publishes the typed
    // EnableResult to state/hardware/surface/<node>/enable. On a
    // non-Surface node it idles (never touches the Bus). Live actions are
    // integration-gated (honest typed errors, never faked).
    sup.spawn(Spawn::new(
        mackesd_core::workers::SurfaceEnableWorker::new(node_id.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("surface_enable".into());

    // SURFACE-4 — the per-node surface_verify worker. On a recognised
    // Microsoft Surface it probes each profile-claimed subsystem into a
    // tri-state board (Ok/Failed/Degraded + NeedsGesture, each with a real
    // reason) published to state/hardware/surface/<node>/probes (the Test
    // tab), and publishes the compact enablement summary (model, %, red
    // count) to state/hardware/surface/<node> for the fleet rollup. On a
    // non-Surface node it idles. Live probes are integration-gated (honest
    // typed states headless, never faked green).
    sup.spawn(Spawn::new(
        mackesd_core::workers::SurfaceVerifyWorker::new(node_id.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("surface_verify".into());

    // SURFACE-5 — the per-node surface_firmware worker. On a recognised
    // Microsoft Surface it publishes the fwupd/LVFS inventory (current +
    // available versions per device) to state/hardware/surface/<node>/firmware
    // (the Install tab's firmware panel), drains typed-armed apply requests
    // on action/hardware/surface/<node>/fw-apply, and on a successful apply
    // re-runs SURFACE-4's verify. An un-armed apply is refused; live fwupd
    // calls are integration-gated (honest typed errors, never a faked
    // update). On a non-Surface node it idles.
    sup.spawn(Spawn::new(
        mackesd_core::workers::SurfaceFirmwareWorker::new(node_id.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("surface_firmware".into());

    // MON-1.b (v2.6) — Netdata aggregator-IP publisher.
    // Pairs with `apply_netdata_monitor`'s baseline
    // /etc/netdata/netdata.conf: when this peer wins
    // leader-election it publishes its overlay IP to
    // QNM-Shared so every other peer picks the same
    // aggregator; on demote it stops publishing and the
    // freshest pointer wins. Every tick re-reads the
    // freshest pointer + rewrites the local netdata.conf
    // `[stream]` block + reloads netdata when the
    // aggregator IP changes. Fail-soft per the v2.6
    // design lock: missing aggregator strips the
    // `[stream]` block so netdata stays local-only with
    // the 7-day dbengine retention. API key defaults to
    // `mesh-${MDE_MESH_ID}-netdata` so every peer in the
    // same mesh shares the value automatically without
    // an extra wizard step (operators can override via
    // MDE_NETDATA_API_KEY if they want a custom value).
    match mackesd_core::store::open(&db_path) {
        Ok(conn) => {
            let netdata_store = Arc::new(tokio::sync::Mutex::new(conn));
            let mesh_id_for_netdata =
                std::env::var("MDE_MESH_ID").unwrap_or_else(|_| format!("mesh-{node_id}"));
            let api_key = std::env::var("MDE_NETDATA_API_KEY")
                .unwrap_or_else(|_| format!("{mesh_id_for_netdata}-netdata"));
            sup.spawn(Spawn::new(
                mackesd_core::workers::netdata_aggregator::NetdataAggregator::new(
                    netdata_store,
                    node_id.clone(),
                    workgroup_root.clone(),
                    api_key,
                ),
                RestartPolicy::Always,
            ));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("netdata_aggregator".into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                db_path = %db_path.display(),
                "netdata_aggregator: sqlite open failed; worker skipped"
            );
        }
    }

    // PLANES-24 W63 — scheduled one-puller mirror sync. Every node writes
    // its dnf .repo to self-serve from the local file:// mount (W62); the
    // leader additionally pulls upstream + indexes, Syncthing replicating
    // the result. No DB handle needed — it works off the replicated root.
    sup.spawn(Spawn::new(
        mackesd_core::workers::mirror_syncd::MirrorSyncd::new(workgroup_root.clone()),
        RestartPolicy::Always,
    ));
    worker_names
        .lock()
        .expect("worker_names mutex")
        .push("mirror_syncd".into());

    // NF-1.5 (v2.5) — TCP/443 covert listener. Binds the
    // TLS 1.3 listener on :443 (default; env-overrideable),
    // spawns the per-stream demux pump per accepted peer
    // tunnel. Cert + key paths default to
    // /etc/nebula/lighthouse.{crt,key}; overridable via
    // MDE_HTTPS_TUNNEL_{CERT,KEY} env vars so operators
    // running Let's-Encrypt-issued certs can point to the
    // existing PEM chain. On peer-role boxes (no cert
    // files), the worker fails its bind + the supervisor's
    // OnFailure backoff effectively quarantines it.
    match mackesd_core::workers::nebula_https_listener::NebulaHttpsListener::new() {
        Ok(mut w) => {
            if let Ok(p) = std::env::var("MDE_HTTPS_TUNNEL_CERT") {
                w = w.with_cert(PathBuf::from(p));
            }
            if let Ok(p) = std::env::var("MDE_HTTPS_TUNNEL_KEY") {
                w = w.with_key(PathBuf::from(p));
            }
            if let Ok(addr) = std::env::var("MDE_HTTPS_TUNNEL_BIND") {
                if let Ok(parsed) = addr.parse() {
                    w = w.with_bind_addr(parsed);
                } else {
                    tracing::warn!(
                        value = %addr,
                        "nebula-https-listener: MDE_HTTPS_TUNNEL_BIND parse failed; using default",
                    );
                }
            }
            // Bug 6 (2026-06-06) — only run the relay :443 listener when a
            // relay cert is actually present. A box with no lighthouse /
            // Let's-Encrypt cert is not a relay; spawning anyway only fails
            // the bind (and a per-user daemon can never bind a privileged
            // port at all), which the OnFailure policy then respins ~4x/s.
            //
            // SUBAUDIT-D1 (2026-06-16) — the relay never ran *anywhere*
            // because no node ever had /etc/nebula/lighthouse.crt. A
            // public/lighthouse node now SELF-BOOTSTRAPS a self-signed relay
            // cert so the :443 listener actually binds by default. Gated on
            // relay-eligibility — the lighthouse role.host marker OR a pinned
            // Lighthouse role — so a NAT'd workstation (e.g. .13) never
            // generates a cert or binds :443.
            let https_cert = std::env::var("MDE_HTTPS_TUNNEL_CERT").unwrap_or_else(|_| {
                mackesd_core::workers::nebula_https_listener::DEFAULT_CERT_PATH.to_string()
            });
            let https_key = std::env::var("MDE_HTTPS_TUNNEL_KEY").unwrap_or_else(|_| {
                mackesd_core::workers::nebula_https_listener::DEFAULT_KEY_PATH.to_string()
            });
            let relay_eligible =
                std::path::Path::new(mackesd_core::ipc::nebula::DEFAULT_ROLE_HOST_MARKER).exists()
                    || matches!(mde_role::load(), Ok(mde_role::Role::Lighthouse));
            if relay_eligible && !std::path::Path::new(&https_cert).exists() {
                let sans = vec![
                    detect_primary_ipv4().unwrap_or_else(|_| "127.0.0.1".to_string()),
                    "lighthouse.mesh.local".to_string(),
                ];
                match mackesd_core::nebula_enroll_endpoint::ensure_self_signed_cert(
                    std::path::Path::new(&https_cert),
                    std::path::Path::new(&https_key),
                    &sans,
                ) {
                    Ok(_) => tracing::info!(
                        cert = %https_cert,
                        "nebula-https-listener: self-bootstrapped a relay cert (SUBAUDIT-D1)",
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "nebula-https-listener: relay cert bootstrap failed; relay stays down",
                    ),
                }
            }
            if std::path::Path::new(&https_cert).exists() {
                sup.spawn(Spawn::new(w, RestartPolicy::OnFailure));
                worker_names
                    .lock()
                    .expect("worker_names mutex")
                    .push("nebula_https_listener".into());
            } else if relay_eligible {
                tracing::warn!(
                    cert = %https_cert,
                    "nebula-https-listener: relay-eligible but no cert after bootstrap — relay down",
                );
            } else {
                tracing::info!(
                    cert = %https_cert,
                    "nebula-https-listener: not a relay node (no role.host marker / not Lighthouse) — skipped",
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "nebula-https-listener: construction failed; skipped",
            );
        }
    }

    // v4.0.1 AF-NET-2 (2026-05-23) — mesh-latency sniffer.
    // Pings every enrolled non-local peer every 30 s and
    // writes the result to ~/.cache/mde/mesh-latency.json.
    // The WB-2.k.a Cairo topology canvas + panel Mesh-status
    // tray badge both consume the file. Best-choice
    // deviation from the TransportRegistry-routed approach
    // — see worker doc-comment.
    match mackesd_core::store::open(&db_path) {
        Ok(conn) => {
            let lat_store = Arc::new(tokio::sync::Mutex::new(conn));
            let cache = mackesd_core::workers::mesh_latency::default_cache_path();
            spawn_tiered(sup, worker_names, role_rank, "mesh_latency", || {
                mackesd_core::workers::mesh_latency::MeshLatencyWorker::new(
                    lat_store,
                    node_id.clone(),
                    cache,
                )
                .with_interval(daemon_cfg.mesh_latency_sweep())
            });
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                db_path = %db_path.display(),
                "mesh-latency: sqlite open failed; worker skipped"
            );
        }
    }

    // MESHMAP-6 (2026-06-27) — real per-link byte counters. Maintains an
    // nftables accounting table on the Nebula interface (one passive
    // counter per peer overlay IP per direction), reads byte deltas every
    // 5 s, and writes ~/.cache/mde/link-traffic.json. The mesh wallpaper /
    // Peers-Map flow particles consume it as the REAL per-edge source,
    // falling back to the per-node `sample_flows` proxy (MESHMAP-3) when
    // the cache is absent (no nft / non-root / pre-delta). Rank-0 control-
    // plane observer (runs everywhere, like mesh_latency); honest no-op on
    // a box without nft (idles on the token, consumer keeps the proxy).
    if mackesd_core::worker_role::runs("link-traffic", role_rank) {
        match mackesd_core::store::open(&db_path) {
            Ok(conn) => {
                let lt_store = Arc::new(tokio::sync::Mutex::new(conn));
                let lt_cache = mackesd_core::workers::link_traffic::default_cache_path();
                spawn_tiered(sup, worker_names, role_rank, "link-traffic", || {
                    mackesd_core::workers::link_traffic::LinkTrafficWorker::new(
                        lt_store,
                        node_id.clone(),
                        lt_cache,
                    )
                });
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    db_path = %db_path.display(),
                    "link-traffic: sqlite open failed; worker skipped"
                );
            }
        }
    }

    // TUNE-16.d (2026-05-30) — Q22 8-peer cap counter. Reads the
    // enrolled peer count every 30 s, writes ~/.cache/mde/peer-cap.json,
    // and publishes to mesh/peer-cap/updated. Phones count (enrolled
    // as role='peer'); federated external-mesh peers don't appear in
    // the local store and are naturally excluded.
    match mackesd_core::store::open(&db_path) {
        Ok(conn) => {
            let cap_store = Arc::new(tokio::sync::Mutex::new(conn));
            let cap_cache = dirs::cache_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join("mde")
                .join("peer-cap.json");
            sup.spawn(Spawn::new(
                mackesd_core::workers::peer_cap::PeerCapWorker::new(cap_store, cap_cache),
                RestartPolicy::OnFailure,
            ));
            worker_names
                .lock()
                .expect("worker_names mutex")
                .push("peer-cap".into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                db_path = %db_path.display(),
                "peer-cap: sqlite open failed; worker skipped"
            );
        }
    }

    // LIGHTHOUSE-8 — per-lighthouse deep-probe lane. Every ~15 s probes each
    // lighthouse for Nebula handshake / public IP / overlay peer count /
    // uptime / CA cert-expiry and publishes a LighthouseProbe to
    // `compute/lighthouse-probe/<name>`; the Workbench Lighthouses tab
    // renders it. The spawn is owned by the worker module
    // (`Supervisor::spawn_lighthouse_probe`, sibling `Spawn::new` pattern +
    // the rank-0 role gate); it self-resolves its workgroup root from
    // `MDE_WORKGROUP_ROOT`, so no DB/handle plumbing is needed here.
    if let Some(name) = sup.spawn_lighthouse_probe() {
        worker_names
            .lock()
            .expect("worker_names mutex")
            .push(name.into());
    }
}

// run_serve extract: kdc/messaging/fleet-sync tail workers (kdc_host .. music_autoconfig).
pub(crate) fn spawn_messaging_sync_workers(
    sup: &mut mackesd_core::workers::Supervisor,
    worker_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    role_rank: u8,
    node_id: &String,
    workgroup_root: &PathBuf,
    worker_status: &mackesd_core::workers::WorkerStatusMap,
) {
    use std::sync::Arc;
    // v4.0.1 KDC2-3.3 wire-up (2026-05-23) — spawn the KDC host
    // worker. Owns the pairing store at $XDG_CONFIG_HOME/mde/
    // connect (default ~/.config/mde/connect), the shared
    // DiscoveryRegistry, the outbound packet queue, and the
    // dev.mackes.MDE.Connect D-Bus surface. Graceful-degrade
    // on D-Bus failure — the worker keeps the host alive so
    // the mesh-router can still dispatch through KDC, even if
    // the operator-facing UI methods aren't reachable.
    let kdc_config_dir = {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from);
        let home_default = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|h| h.join(".config"));
        xdg.or(home_default)
            .map(|p| p.join("mde").join("connect"))
            .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/mde/connect"))
    };
    spawn_tiered(sup, worker_names, role_rank, "kdc_host", || {
        mackesd_core::workers::kdc_host::KdcHostWorker::new(kdc_config_dir)
    });

    // BUS-1.1 (v6.x Mackes Bus) — supervise the `mde-bus` daemon
    // subprocess. Gracefully degrades when the binary is absent
    // (dev box, RPM not yet installed) — the worker loops on a
    // 30s tick waiting for the binary to appear. Once the BUS-1
    // sub-epic ships, every mackesd peer carries the bus.
    spawn_tiered(sup, worker_names, role_rank, "bus_supervisor", || {
        mackesd_core::workers::bus_supervisor::BusSupervisor::new()
    });

    // CLIP-SYNC-1 — mesh clipboard sync. Watches the local Wayland clipboard
    // (`wl-paste --watch`, the Cosmic clipboard-manager hook), broadcasts every
    // text clip on the bus + appends to the mesh-global `clipboard/history.json`
    // (last 50 unpinned + unlimited pinned). As the root system daemon it has
    // no inherited $WAYLAND_DISPLAY, so it DISCOVERS the active seat0 graphical
    // session (CLIP-SYNC-2) and spawns the capture as that user; a genuinely
    // headless peer (Lighthouse/Server) finds no session and idles quietly, so
    // it's cheap there. (This replaces the never-built `mde-clipd` daemon +
    // `clipd_supervisor`: that binary never existed in the workspace; this
    // worker is the sole, real clipboard capturer.)
    spawn_tiered(sup, worker_names, role_rank, "clipboard_sync", || {
        mackesd_core::workers::clipboard_sync::build(workgroup_root.clone())
    });

    // NOTIFY-CHAT-2 — the `chat` worker: live Bus send/recv (signs +
    // relays on event/chat/message, persists to this node's Syncthing
    // ring-log for offline backfill), folds every alert/event lane into a
    // message from the originating host (lock 11), derives presence from
    // the mesh-status snapshot + manual gossip, and republishes the
    // state/chat/roster + state/chat/conversation/<key> read-model the
    // Surface::Chat UI renders. Runs on EVERY node incl. headless (emit +
    // relay, no UI) so alerts flow fleet-wide; unknown-worker rank-0
    // default runs it everywhere. self_host is the bare hostname (the
    // roster/DM identity), signed with the persisted node identity key.
    // NOTE (E12-20 storage worker adds its own spawn line to this block —
    // keep-both merge expected).
    if mackesd_core::worker_role::runs("chat", role_rank) {
        match mackesd_core::node_key::load_or_create(std::path::Path::new(
            mackesd_core::node_key::DEFAULT_KEY_PATH,
        )) {
            Ok(signing_key) => {
                let self_host = node_id
                    .strip_prefix("peer:")
                    .unwrap_or(&node_id)
                    .to_string();
                spawn_tiered(sup, worker_names, role_rank, "chat", || {
                    mackesd_core::workers::chat::ChatWorker::new(
                        workgroup_root.clone(),
                        self_host,
                        signing_key,
                    )
                });
            }
            Err(e) => tracing::warn!(
                target: "mackesd::chat",
                error = %e,
                "chat worker: node signing key unavailable; not spawning",
            ),
        }
    }

    // WL-FUNC-011 Phase 2 — the `collab` worker: the live spine that makes the
    // headless mde-collab-core CollabEngine real on the mesh. Drains
    // action/collab/* commands (validate + Ed25519-sign via this node's identity
    // → signed events), appends each to the per-space Syncthing-replicable actor
    // log + projects it into the SQLite read models, publishes the live signed
    // event on collab/event/<space>/<actor> + republishes the affected
    // state/collab/* read models, and converges by merging foreign
    // collab/event/* (bus fast-path) + replicated actor logs (Syncthing durable-
    // path). UNIVERSAL (rank 0) like the chat worker it will EVENTUALLY replace
    // (Phase 4; it runs ALONGSIDE chat for now) — every node, headless included,
    // participates. Same persisted node identity + bare-hostname actor as chat.
    if mackesd_core::worker_role::runs("collab", role_rank) {
        match mackesd_core::node_key::load_or_create(std::path::Path::new(
            mackesd_core::node_key::DEFAULT_KEY_PATH,
        )) {
            Ok(signing_key) => {
                let self_host = node_id
                    .strip_prefix("peer:")
                    .unwrap_or(&node_id)
                    .to_string();
                spawn_tiered(sup, worker_names, role_rank, "collab", || {
                    mackesd_core::workers::collab::CollabWorker::new(
                        workgroup_root.clone(),
                        self_host,
                        signing_key,
                    )
                });
            }
            Err(e) => tracing::warn!(
                target: "mackesd::collab",
                error = %e,
                "collab worker: node signing key unavailable; not spawning",
            ),
        }
    }

    // CHAT-FIX-2 — the `notify` worker: the local-notification PRODUCER that
    // makes Chat non-empty absent peer chatter. It watches this node's OWN
    // event sources (mesh peer join/leave via the replicated directory,
    // dnf/platform updates, systemctl --failed, df/SMART thresholds, journal
    // WARN+) on bounded/edge-triggered polls and publishes typed notifications
    // on `event/notify/<source>` — a lane the chat worker above folds
    // (ALERT_LANE_PREFIXES) into this node's `alert:<self>` conversation the
    // Surface::Chat UI renders as a timestamped feed + tray badge. Runs on
    // EVERY node (rank 0), same self_host identity as the chat worker; every
    // external source degrades honestly when its binary is absent.
    spawn_tiered(sup, worker_names, role_rank, "notify", || {
        let self_host = node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string();
        mackesd_core::workers::notify::NotifyWorker::new(workgroup_root.clone(), self_host)
    });

    // WL-SEC-002 — the federation runtime-enforcement worker. UNIVERSAL (rank 0): it
    // reads the accepted cross-mesh grants (`federation.yaml`) and ENFORCES them —
    // draining the cross-mesh ingress spool through the DEFAULT-DENY decision gate
    // (only granted, non-excluded topics from accepted/non-revoked foreign meshes
    // cross onto the local bus), draining the shell Federation panel's accept/revoke
    // actions (accept installs the cross-mesh Nebula trust cert; revoke deletes it),
    // and publishing the `state/federation/<node>` mirror the shell renders. Keyed by
    // the full node id (the retained-status mirror key `state/federation/<node>`).
    spawn_tiered(sup, worker_names, role_rank, "federation_enforcer", || {
        mackesd_core::workers::federation_enforcer::FederationEnforcerWorker::new(node_id.clone())
    });

    // NODE-GRADE-1 — the `node_grade` worker: every node computes + publishes
    // its OWN A–F capability grade (docs/design/node-grade.md). It scores five
    // factors from telemetry the platform already gathers (§6, no new probes):
    // CPU headroom (/proc/loadavg vs cores), RAM + disk free (/proc/meminfo,
    // df /), role/worker health (the supervisor's live worker-status map +
    // systemctl --failed), and mesh reachability (the replicated peer
    // directory) — resource-heaviest weighted average → a smoothed 0–100 score
    // + trend → an A–F band, published to
    // `<workgroup_root>/node-grade/<hostname>.json` (the SEC-5 mesh-shunt
    // own-row idiom) so every peer reads every node's grade. A debounced drop
    // into D/F fires an `event/notify/node-grade` alert the chat worker folds
    // into the Chat feed (CHAT-FIX-2). Universal (rank 0) like notify — every
    // node grades itself; role_rank marks a lighthouse for the mesh factor and
    // worker_status feeds the role factor.
    spawn_tiered(sup, worker_names, role_rank, "node_grade", || {
        let self_host = node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string();
        mackesd_core::workers::node_grade::NodeGradeWorker::new(
            self_host,
            workgroup_root.clone(),
            role_rank,
            Some(Arc::clone(&worker_status)),
        )
    });

    // TRANSFERS-1 — the `transfers` worker: the daemon-owned queue/ledger/verb/
    // state-machine spine of the Transfers surface (docs/design/transfers-
    // surface.md). Owns a typed TransferJob envelope, a persistent node-local
    // ledger (survives restart, Q11), the submit/cancel/pause/resume/list verbs
    // (Q14, driven by the `mackesd transfer …` CLI for §9 parity), and the
    // parallel cap (Q12). Execution is delegated to an injectable LaneRunner seam
    // the per-protocol lanes (TRANSFERS-2..6) implement — honestly gated for now
    // (§7: a submitted job fails naming the un-wired lane, never a fake success).
    // Workstation-tier (rank 1) — a desktop feature fronted by the File Browser,
    // sibling of pty_broker/mesh_mount; it idles gracefully on a headless box /
    // Lighthouse (an empty inbox + empty ledger). Store is node-local, so it
    // needs neither the workgroup root nor the node id.
    spawn_tiered(sup, worker_names, role_rank, "transfers", || {
        mackesd_core::workers::transfers::TransfersWorker::new(
            mackesd_core::workers::transfers::default_store_root(),
        )
    });

    // TUNE-3.b (2026-05-26) — wire the v1.3.0 Fleet ansible-pull
    // worker. `crates/mackesd/src/workers/ansible_pull.rs::build`
    // has shipped since v2.0.0 Phase B.6 but stayed dead;
    // [[project_v1_3_0_fleet]] keeps the feature in scope so
    // wiring is the right cleanup. The worker invokes
    // `ansible-pull -U <MDE_ANSIBLE_PULL_URL> -i localhost,` on
    // a 15-min cadence (matches the retired
    // `mackes-ansible-pull.timer`). With MDE_ANSIBLE_PULL_URL
    // unset the ansible-pull binary fails fast + the supervisor
    // logs the error — the worker stays cheap to host.
    // Bug 6 (2026-06-06) — without MDE_ANSIBLE_PULL_URL the worker only spawns
    // `ansible-pull` to fail; a box with no fleet config-pull URL has nothing
    // to do, so skip rather than respawn-on-failure into a periodic WARN.
    let ansible_configured = std::env::var("MDE_ANSIBLE_PULL_URL")
        .map(|u| !u.is_empty())
        .unwrap_or(false);
    if mackesd_core::worker_role::runs("ansible-pull", role_rank) {
        if ansible_configured {
            spawn_tiered(sup, worker_names, role_rank, "ansible-pull", || {
                mackesd_core::workers::ansible_pull::build()
            });
        } else {
            tracing::info!(
                "ansible-pull: MDE_ANSIBLE_PULL_URL unset; fleet config-pull worker skipped"
            );
        }
    }

    // EPIC-SYNC-APP-CONFIG (Q26, 2026-05-28) — app-config sync is
    // now a native-Rust worker (`workers::app_sync`); it discovers
    // mesh media servers + writes Sublime Music / Delfin configs +
    // the `~/Mackes Media/` launcher view directly, retiring the
    // `python3 -m mackes.media_sync_daemon` subprocess (advances
    // §11 #6). `OnFailure` keeps the 60 s tick alive across a
    // transient write/probe error.
    spawn_tiered(sup, worker_names, role_rank, "app-sync", || {
        mackesd_core::workers::app_sync::build()
    });
    // WL-UX-005 — the peer_app_launch executor: the missing consumer of the
    // shell Front Door's `action/apps/launch` publishes. It launches a
    // peer-requested app on THIS node, but only one this node itself advertises
    // in its own app catalog (`ipc::apps::scan_local_apps`) — never an arbitrary
    // command off the wire — and logs every launch + refusal. Workstation-tier
    // (you launch apps onto a seat); node_id is the sole launch target it acts on.
    spawn_tiered(sup, worker_names, role_rank, "peer_app_launch", || {
        mackesd_core::workers::peer_app_launch::PeerAppLaunchWorker::new(node_id.clone())
    });
    // remmina-sync is a native Rust tick worker (RETIRE-PY.2): every 60 s
    // it reads the mesh peer registry, TCP-probes SSH/RDP/VNC, and
    // reconciles Remmina's "Mesh Peers" group. No `python3` is spawned.
    spawn_tiered(sup, worker_names, role_rank, "remmina-sync", || {
        mackesd_core::workers::remmina_sync::build()
    });

    // MEDIA-8 — Workstation music auto-config (desktop-tier, like
    // remmina-sync). Every 60 s it reads the published shared account off
    // the replicated registry plane (<workgroup-root>/<host>/media-
    // registry.json, written by a Lighthouse_Media node's media_registry
    // worker) and idempotently writes the uid-1000 desktop user's
    // airsonic-creds.json, so a fresh node's mde-music auto-browses the mesh
    // library with no manual connect. NO mesh age key on Workstations — the
    // shared account flows through the SERVICE REGISTRY, not the secret
    // store. The .with_workgroup_root honors --workgroup-root so it reads
    // where the registry writers write. Never clobbers a user-set creds file.
    spawn_tiered(sup, worker_names, role_rank, "music_autoconfig", || {
        mackesd_core::workers::music_autoconfig::MusicAutoconfigWorker::new()
            .with_workgroup_root(workgroup_root.clone())
    });
}
