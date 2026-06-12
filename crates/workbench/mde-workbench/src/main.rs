//! `mde-workbench` binary entry — single-instance handshake
//! + Iced launch.
//!
//! CB-1.13 contract: every invocation either becomes the primary
//! workbench process or hands its `--focus <slug>` argument off
//! to the already-running primary.
//!
//! DBUS-3 (Q96/EPIC-RETIRE-DBUS): the single-instance NAME
//! (`dev.mackes.MDE.Workbench`) is still owned on D-Bus — name
//! ownership is inherently a D-Bus/socket primitive (finding #3
//! documented exception). The `focus` hand-off itself migrated to
//! the Bus action topic `action/shell/workbench-focus` with the
//! 40 ms interactive poll (finding #1).

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use mde_bus::hooks::config::Priority;
use mde_bus::rpc::{request_with_interval, INTERACTIVE_POLL_INTERVAL};
use mde_workbench::{
    acquire_single_instance, serve_focus_bus, App, PendingFocus, PrimaryStatus, ACTION_TOPIC,
};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(
    name = "mde-workbench",
    about = "Mackes Desktop Environment (MDE) Workbench"
)]
struct Cli {
    /// Open the workbench at the named panel
    /// (e.g. `--focus network.mesh_ssh`).
    #[arg(long)]
    focus: Option<String>,
    /// E6.1 — open the workbench at a role's card landing
    /// (e.g. `--page apps`). Role-level alias of `--focus`; the
    /// Start tile + "Manage Workstation" app invoke this. When both
    /// are given `--focus` (the more specific panel target) wins.
    #[arg(long)]
    page: Option<String>,
}

impl Cli {
    /// The effective deep-link slug: the panel-level `--focus` if
    /// given, else the role-level `--page`. Both resolve through the
    /// same `model::view_from_focus_slug` router (a bare role slug
    /// lands on that role's card; `<role>.<panel>` on the panel).
    fn target(&self) -> Option<String> {
        self.focus.clone().or_else(|| self.page.clone())
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let target = cli.target();
    let initial_focus = target.clone().unwrap_or_default();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            error!("failed to build tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };

    // AUD-8 / §2 — single-instance via a pidfile (no private D-Bus name). A
    // live sibling → hand the focus slug off over the Bus + exit; else become
    // primary and keep the lockfile handle alive for the process lifetime.
    let (status, lock) = acquire_single_instance();
    if status == PrimaryStatus::Existing {
        return hand_off_to_running(&runtime, &target);
    }
    if let Some(handle) = lock {
        Box::leak(Box::new(handle));
    } else {
        info!("continuing without single-instance protection (lockfile unavailable)");
    }
    if start_primary_focus_responder().is_err() {
        info!("focus responder unavailable; --focus hand-off may not work");
    }

    // Iced takes over the main thread — keep the tokio runtime alive for the
    // lifetime of the process via a leaked handle.
    let _runtime = Box::leak(Box::new(runtime));

    if !initial_focus.is_empty() {
        PendingFocus::submit(initial_focus);
    }

    match App::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("iced runtime error: {e}");
            ExitCode::from(1)
        }
    }
}

/// Sibling-process branch — publish the `--focus <slug>` request on
/// the Bus action topic the running primary serves, then exit.
/// Uses the 40 ms interactive poll so the round-trip is imperceptible
/// (finding #1). Returns `ExitCode::SUCCESS` when the primary
/// acknowledged, `2` when the Bus call itself failed.
fn hand_off_to_running(runtime: &tokio::runtime::Runtime, focus: &Option<String>) -> ExitCode {
    let slug = focus.clone().unwrap_or_default();
    info!(%slug, "primary workbench already running — handing off focus over the Bus");
    let Some(bus_root) = mde_bus::default_data_dir() else {
        error!("no Bus data dir; cannot hand off focus");
        return ExitCode::from(2);
    };
    let persist = match mde_bus::persist::Persist::open(bus_root) {
        Ok(p) => p,
        Err(e) => {
            error!("opening Bus store for focus hand-off: {e}");
            return ExitCode::from(2);
        }
    };
    let result = runtime.block_on(request_with_interval(
        &persist,
        ACTION_TOPIC,
        Priority::Default,
        None,
        Some(slug.as_str()),
        Duration::from_secs(2),
        INTERACTIVE_POLL_INTERVAL,
    ));
    match result {
        Ok(_reply) => ExitCode::SUCCESS,
        Err(e) => {
            error!("focus hand-off over the Bus failed: {e}");
            ExitCode::from(2)
        }
    }
}

/// Primary-process branch — spawn the Bus focus responder so the
/// [`PendingFocus`] slot fills whenever a sibling invocation publishes to
/// `action/shell/workbench-focus` (the single-instance primitive itself is now
/// the pidfile, AUD-8). The responder runs on its own thread because `Persist`
/// (rusqlite) isn't `Send`.
fn start_primary_focus_responder() -> Result<(), ()> {
    std::thread::Builder::new()
        .name("workbench-focus-bus".into())
        .spawn(|| {
            let Some(bus_root) = mde_bus::default_data_dir() else {
                error!("workbench focus responder: no Bus data dir; --focus hand-off unavailable");
                return;
            };
            match mde_bus::persist::Persist::open(bus_root) {
                Ok(persist) => serve_focus_bus(&persist, || false),
                Err(e) => error!("workbench focus responder: opening Bus store: {e}"),
            }
        })
        .map(|_| {
            info!("primary workbench focus responder started on the Bus");
        })
        .map_err(|e| error!("spawning workbench focus responder thread: {e}"))
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use mde_workbench::model::{view_from_focus_slug, Group, View};

    fn cli(focus: Option<&str>, page: Option<&str>) -> Cli {
        Cli {
            focus: focus.map(str::to_string),
            page: page.map(str::to_string),
        }
    }

    #[test]
    fn focus_wins_over_page_when_both_set() {
        // --focus is the more specific panel target; it takes precedence.
        let c = cli(Some("network.mesh_ssh"), Some("apps"));
        assert_eq!(c.target().as_deref(), Some("network.mesh_ssh"));
    }

    #[test]
    fn page_is_used_when_focus_absent() {
        let c = cli(None, Some("apps"));
        assert_eq!(c.target().as_deref(), Some("apps"));
    }

    #[test]
    fn target_is_none_when_neither_given() {
        assert_eq!(cli(None, None).target(), None);
    }

    #[test]
    fn page_role_slug_routes_to_that_roles_card() {
        // E6.1 acceptance: `--page <role>` deep-links to the role's
        // card landing (a group-root View).
        let target = cli(None, Some("fleet")).target().unwrap();
        assert_eq!(
            view_from_focus_slug(&target),
            Some(View::Group(Group::Fleet))
        );
    }
}
