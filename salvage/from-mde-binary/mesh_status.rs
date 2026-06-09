//! PANEL-POLISH (E5.5) — mesh-status indicator for the panel tray.
//!
//! Re-homes the retired `mde-applet-mesh-status`: a background thread polls
//! `action/nebula/status` over the mesh Bus and exposes peer-count + online
//! state as shared memory the panel reads each tick — non-blocking, mirroring
//! how `panel.rs` reads the StatusNotifier tray. (Polling the Bus directly in
//! the GUI tick would risk an 800 ms stall when mackesd is down; the background
//! thread keeps the bar responsive.)

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How often the background thread re-queries mesh status (it changes slowly).
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Per-query Bus timeout — short so a down mackesd doesn't wedge the poller.
const QUERY_TIMEOUT: Duration = Duration::from_millis(800);

/// Mesh status reduced to what the tray chip renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshStatus {
    /// Paired peers (excluding self) the local node knows about.
    pub peer_count: usize,
    /// Whether the active transport is anything other than "offline".
    pub online: bool,
}

/// Shared handle the panel reads (`None` until the first successful poll).
pub type Handle = Arc<Mutex<Option<MeshStatus>>>;

/// Start the background poller; returns the shared-state handle the panel reads.
#[must_use]
pub fn start() -> Handle {
    let handle: Handle = Arc::new(Mutex::new(None));
    let writer = handle.clone();
    let _ = thread::Builder::new()
        .name("mde-mesh-status".into())
        .spawn(move || loop {
            let status = poll_once();
            if let Ok(mut g) = writer.lock() {
                *g = status;
            }
            thread::sleep(POLL_INTERVAL);
        });
    handle
}

/// One Bus poll of `action/nebula/status` → [`MeshStatus`] (`None` on any
/// failure: no Bus dir, no mackesd, timeout, or malformed reply).
fn poll_once() -> Option<MeshStatus> {
    let bus_dir = mde_bus::default_data_dir()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(bus_dir).ok()?;
        mde_bus::rpc::request(
            &persist,
            "action/nebula/status",
            mde_bus::hooks::config::Priority::Default,
            None,
            None,
            QUERY_TIMEOUT,
        )
        .await
        .ok()
    })?;
    parse_status(&reply.body?)
}

/// Parse the `StatusSnapshot` reply body → [`MeshStatus`].
fn parse_status(json: &str) -> Option<MeshStatus> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("error").is_some() {
        return None;
    }
    let peer_count =
        usize::try_from(v.get("peer_count").and_then(serde_json::Value::as_u64)?).ok()?;
    let transport = v
        .get("active_transport")
        .and_then(|x| x.as_str())
        .unwrap_or("offline");
    Some(MeshStatus {
        peer_count,
        online: transport != "offline",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lifts_peer_count_and_online_transport() {
        let s = parse_status(r#"{"peer_count":3,"active_transport":"nebula_direct"}"#).unwrap();
        assert_eq!(s.peer_count, 3);
        assert!(s.online);
    }

    #[test]
    fn parse_offline_transport_is_not_online() {
        let s = parse_status(r#"{"peer_count":0,"active_transport":"offline"}"#).unwrap();
        assert_eq!(s.peer_count, 0);
        assert!(!s.online);
    }

    #[test]
    fn parse_rejects_error_envelope_and_garbage() {
        assert!(parse_status(r#"{"error":"no mackesd"}"#).is_none());
        assert!(parse_status("not json").is_none());
        assert!(parse_status("{}").is_none()); // no peer_count
    }
}
