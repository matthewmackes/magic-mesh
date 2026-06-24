//! CLIP-SYNC-2 — active graphical-session discovery for the system daemon.
//!
//! `mackesd` runs as a **root system service** (`mackesd.service`,
//! `WantedBy=multi-user.target`) — it has **no `$WAYLAND_DISPLAY`** in its
//! environment, because it is not part of any user's graphical session. The
//! clipboard-sync worker's capture command (`wl-paste --watch`) needs a live
//! Wayland session to attach to, so before this module the worker idled
//! forever on the system daemon (`$WAYLAND_DISPLAY unset; idling`) and the
//! mesh-global `history.json` was never written → the Notification-Center
//! Clipboard Viewer showed "Clipboard history is empty." on every Workstation.
//! (Found live on Eagle, 2026-06-24.)
//!
//! This module discovers the active graphical session the daemon should drive:
//!
//!   1. `loginctl list-sessions` → the session **ids**, then every property
//!      (`Type`/`State`/`Seat`/uid/`RuntimePath`) is read from the reliable
//!      `loginctl show-session <id>` `KEY=VALUE` dump. We pick the graphical
//!      session (`Type=wayland`, `State=active`/`online`) owned by a **regular
//!      login uid** (a `gdm` greeter, uid 42, never out-scores the operator),
//!      preferring `seat0` + `active`. Its `RuntimePath` (`XDG_RUNTIME_DIR`) is
//!      read, not assumed.
//!   2. The `wayland-*` socket inside that runtime dir becomes `$WAYLAND_DISPLAY`.
//!   3. The owning uid's **primary gid + `$HOME`** are resolved from
//!      `/etc/passwd` so the capture child is a COMPLETE credential drop (uid
//!      alone would leave it in root's group) with a correct `$HOME`.
//!   4. The worker spawns `wl-paste` as that uid/gid with `HOME` +
//!      `XDG_RUNTIME_DIR` + `WAYLAND_DISPLAY`, so the capture attaches to the
//!      operator's real desktop session, not root's.
//!
//! **Why ids-from-the-table, props-from-`show-session`.** `loginctl
//! list-sessions -o json` is NOT reliable: on Eagle (systemd 259, Fedora 44)
//! the very same `--no-legend --no-pager -o json` invocation emits the legacy
//! whitespace TABLE, not JSON, so a JSON parse of `list-sessions` returns
//! nothing and the worker idled silently (the live CLIP bug, found 2026-06-24).
//! `loginctl show-session <id>` DOES reliably emit `KEY=VALUE` lines on the
//! same host, so we take only the session **id** (the first whitespace token of
//! each `list-sessions` row — a JSON array is also tolerated as a fallback) and
//! resolve every gating property from `show-session`.
//!
//! On a genuinely headless node (a Lighthouse/Server, or a Workstation before
//! the desktop comes up) there is no graphical session — discovery returns a
//! [`MissKind::Steady`] miss and the worker idles, logging the verdict ONCE on
//! the edge into the idle state (not every poll, so a headless node doesn't
//! spam — graceful degrade). A half-resolved session that can't be driven
//! ([`MissKind::Anomalous`]) is logged every recurrence, since it's unexpected.
//!
//! **Testability.** The `loginctl`/passwd parses + the socket pick are pure
//! functions (`parse_session_ids`, `parse_graphical_session`,
//! `parse_passwd_for_uid`, `pick_wayland_socket`) unit-tested without any
//! `loginctl`/filesystem — including against the **verbatim Eagle table** the
//! real `loginctl` emits; the thin I/O wrapper `discover` shells `loginctl` +
//! reads the runtime dir and `/etc/passwd` only at runtime.

use std::path::{Path, PathBuf};

/// Conventional floor for a regular (human) login uid on Fedora/systemd —
/// `UID_MIN`. System service accounts (e.g. `gdm` = 42) sit below it, so a
/// graphical greeter session never out-scores the operator's own session.
const REGULAR_UID_MIN: u32 = 1000;

/// The active graphical session the daemon will spawn the capture child into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicalSession {
    /// Owning user's numeric uid (the capture child is spawned as this uid).
    pub uid: u32,
    /// Owning user's primary gid (the capture child is spawned with this gid so
    /// the privilege drop is complete — uid alone leaves root's gid/groups).
    pub gid: u32,
    /// The user's `$HOME` (so the child's XDG/config lookups resolve under the
    /// operator, not root's `/root`).
    pub home: PathBuf,
    /// `XDG_RUNTIME_DIR` for the session (logind's `RuntimePath`, normally
    /// `/run/user/<uid>`).
    pub runtime_dir: PathBuf,
    /// The `wayland-*` socket name inside `runtime_dir` (the `$WAYLAND_DISPLAY`
    /// value, e.g. `wayland-0` / `wayland-1`).
    pub wayland_display: String,
}

/// Extract the session **ids** from `loginctl list-sessions` output.
///
/// We do NOT trust `list-sessions -o json` to actually emit JSON — on Eagle
/// (systemd 259, Fedora 44) the same `--no-legend --no-pager -o json`
/// invocation prints the legacy whitespace TABLE instead, e.g.:
///
/// ```text
/// 10 1000 mm -     39237 user    - no -
///  2 1000 mm seat0 2666  user    - no -
///  3 1000 mm -     2937  manager - no -
/// ```
///
/// where the **first whitespace token of each line is the session id**. That
/// table path is the one that MUST work, so it is the primary parse; a JSON
/// array (the documented `-o json` shape some hosts honour) is also accepted as
/// a fallback. We only take the id here — every gating property (uid, seat,
/// type, state, runtime path) is then read from the reliable
/// `loginctl show-session <id>` (`KEY=VALUE`) dump. Pure — no I/O.
fn parse_session_ids(output: &str) -> Vec<String> {
    let trimmed = output.trim_start();
    // JSON-array fallback: only when the output actually looks like one. A real
    // table never starts with `[`, so this never shadows the table path.
    if trimmed.starts_with('[') {
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(trimmed) {
            return arr
                .iter()
                .filter_map(|obj| Some(obj.get("session")?.as_str()?.to_string()))
                .collect();
        }
    }
    // Table path (the reliable one): the first whitespace token of each
    // non-empty line is the session id.
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// One session's relevant `loginctl show-session` properties, already parsed
/// from the `KEY=VALUE` lines `loginctl show-session <id>` prints. Every field
/// we gate on comes from here (not from `list-sessions`) because `show-session`
/// is the reliable machine output on every host we've seen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SessionProps {
    /// `Type=` — `wayland` is the one we can drive `wl-paste` against.
    session_type: String,
    /// `State=` — `active` (foreground) or `online` (backgrounded but live).
    state: String,
    /// `Seat=` — `seat0` is the physical desktop; seatless sessions
    /// (nested/headless compositors) score lower. Empty when not seated.
    seat: String,
    /// `User=` — the owning numeric uid (logind prints the uid here, the name
    /// in `Name=`). `None` when absent/unparseable.
    uid: Option<u32>,
    /// `RuntimePath=` — the session's real `XDG_RUNTIME_DIR` (normally
    /// `/run/user/<uid>`, but read it rather than assume so a custom runtime
    /// dir still resolves). Empty when logind doesn't report it.
    runtime_path: String,
}

/// Parse the `KEY=VALUE` lines of `loginctl show-session <id>` for the
/// properties we gate on. Pure — unknown keys ignored. Whitespace-tolerant.
fn parse_session_props(show_output: &str) -> SessionProps {
    let mut props = SessionProps::default();
    for line in show_output.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Type=") {
            props.session_type = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("State=") {
            props.state = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Seat=") {
            props.seat = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("User=") {
            // `User=1000` — the numeric uid. `Name=` carries the username; we
            // want the uid for the credential drop + the regular-uid score.
            props.uid = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("RuntimePath=") {
            props.runtime_path = v.trim().to_string();
        }
    }
    props
}

/// Is this session one we can attach a Wayland clipboard watcher to?
/// `Type=wayland` and a live state (`active` or `online`). Pure.
fn is_drivable_graphical(props: &SessionProps) -> bool {
    props.session_type.eq_ignore_ascii_case("wayland")
        && (props.state.eq_ignore_ascii_case("active")
            || props.state.eq_ignore_ascii_case("online"))
}

/// The chosen graphical session's identity from the `loginctl` probe: the
/// owning uid and its `XDG_RUNTIME_DIR` (logind's `RuntimePath`, falling back
/// to `/run/user/<uid>` when logind doesn't report one).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChosenSession {
    uid: u32,
    runtime_dir: PathBuf,
}

/// Pure core of discovery: given the `list-sessions` output and a resolver that
/// yields each session's `show-session` output, pick the active graphical
/// session. Both the session ids (from `list-sessions`) and every gating
/// property (uid/seat/type/state/runtime from `show-session`) flow through
/// here, so the live `loginctl`-table-not-JSON behaviour is exercised by tests.
///
/// Scoring (highest wins): a **regular login uid** (>= [`REGULAR_UID_MIN`])
/// outranks a system account so a `gdm` greeter never beats the operator; a
/// `seat0` session (the physical desktop) outranks a seatless one; an `active`
/// session outranks a merely `online` one. On an exact tie the first session
/// in `loginctl` order is kept (a single-user desktop has exactly one such
/// session, so ties don't arise in practice). Returns `None` when no session
/// is a drivable Wayland session (headless / pre-desktop).
fn parse_graphical_session<F>(list_output: &str, mut show_session: F) -> Option<ChosenSession>
where
    F: FnMut(&str) -> Option<String>,
{
    let ids = parse_session_ids(list_output);
    let mut best: Option<(u8, ChosenSession)> = None;
    for id in ids {
        let Some(show) = show_session(&id) else {
            continue;
        };
        let props = parse_session_props(&show);
        if !is_drivable_graphical(&props) {
            continue;
        }
        // uid comes from `show-session`'s `User=` (reliable); a session whose
        // uid we can't read is skipped rather than spawned blind.
        let Some(uid) = props.uid else {
            continue;
        };
        // Score: regular-uid (weight 4) > seat0 (weight 2) > active (weight 1).
        // The regular-uid bit dominates so a system greeter (gdm=42) can never
        // out-score the operator's own session even if it is the active seat0 one.
        let uid_score = u8::from(uid >= REGULAR_UID_MIN);
        let seat_score = u8::from(props.seat == "seat0");
        let state_score = u8::from(props.state.eq_ignore_ascii_case("active"));
        let score = uid_score * 4 + seat_score * 2 + state_score;
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            let runtime_dir = if props.runtime_path.is_empty() {
                PathBuf::from(format!("/run/user/{uid}"))
            } else {
                PathBuf::from(&props.runtime_path)
            };
            best = Some((score, ChosenSession { uid, runtime_dir }));
        }
    }
    best.map(|(_, chosen)| chosen)
}

/// Pick the Wayland display socket name from a runtime dir's entries.
///
/// `$WAYLAND_DISPLAY` is the basename of the compositor's socket in
/// `XDG_RUNTIME_DIR` — `wayland-0`, `wayland-1`, … We ignore the `.lock`
/// sidecar files Wayland writes beside each socket. Prefer the
/// lowest-numbered display (`wayland-0` is the primary on a single-GPU
/// desktop). Pure — takes the already-listed names.
fn pick_wayland_socket(entry_names: &[String]) -> Option<String> {
    // The display number for `wayland-<N>` (the socket), or `None` for anything
    // else — including the `.lock` sidecar (`wayland-0.lock` has a non-numeric
    // suffix, so it's filtered out) and unrelated entries (`bus`, `pipewire-0`).
    // Prefer the lowest-numbered display (`wayland-0` is the primary on a
    // single-GPU desktop); the min over the parsed number gives a stable pick.
    entry_names
        .iter()
        .filter_map(|n| {
            n.strip_prefix("wayland-")
                .and_then(|t| t.parse::<u32>().ok())
                .map(|num| (num, n))
        })
        .min_by_key(|(num, _)| *num)
        .map(|(_, n)| n.clone())
}

/// List the `wayland-*` socket names present in `runtime_dir` (I/O wrapper over
/// [`pick_wayland_socket`]). Returns an empty vec when the dir is unreadable.
fn list_runtime_dir(runtime_dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(runtime_dir) else {
        return Vec::new();
    };
    read.filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Run `loginctl show-session <id>` and return its stdout (the `KEY=VALUE`
/// property dump). `None` on spawn/exec failure (loginctl absent / no logind).
fn loginctl_show_session(session_id: &str) -> Option<String> {
    let out = std::process::Command::new("loginctl")
        .args(["show-session", session_id])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Run `loginctl list-sessions` and return its stdout (the session id table).
/// `None` on spawn/exec failure (no logind, e.g. a minimal container) — the
/// caller then degrades to "no graphical session", i.e. the worker idles.
///
/// We ask for `-o json` (some hosts honour it and [`parse_session_ids`] accepts
/// a JSON array), but we do NOT depend on it: on Eagle (systemd 259) this same
/// command prints the legacy whitespace table, and the id parse handles that as
/// the primary path. We only ever read the session id from this output.
fn loginctl_list_sessions() -> Option<String> {
    let out = std::process::Command::new("loginctl")
        .args(["list-sessions", "--no-legend", "--no-pager", "-o", "json"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// The `(gid, home)` of a passwd entry — what the capture child needs beyond
/// its uid for a complete credential drop + a correct `$HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PasswdIdentity {
    gid: u32,
    home: PathBuf,
}

/// Parse `/etc/passwd` content for `uid`'s primary gid + home dir. Pure — the
/// `name:passwd:uid:gid:gecos:home:shell` colon format, skipping malformed
/// lines. `None` when the uid isn't present.
fn parse_passwd_for_uid(passwd: &str, uid: u32) -> Option<PasswdIdentity> {
    for line in passwd.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        // name:passwd:uid:gid:gecos:home:shell → 7 fields.
        if fields.len() < 7 {
            continue;
        }
        let Ok(row_uid) = fields[2].parse::<u32>() else {
            continue;
        };
        if row_uid != uid {
            continue;
        }
        let Ok(gid) = fields[3].parse::<u32>() else {
            continue;
        };
        return Some(PasswdIdentity {
            gid,
            home: PathBuf::from(fields[5]),
        });
    }
    None
}

/// Resolve `uid`'s primary gid + home from `/etc/passwd`. I/O wrapper over
/// [`parse_passwd_for_uid`]; `None` when `/etc/passwd` is unreadable or the
/// uid is absent (the caller then declines to spawn — a half-dropped child is
/// worse than idling).
fn passwd_identity(uid: u32) -> Option<PasswdIdentity> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    parse_passwd_for_uid(&passwd, uid)
}

/// Why [`discover`] found no session to drive — carried back to the worker so
/// it can log the reason *meaningfully* without spamming.
///
/// The CLIP bug was a worker that captured nothing **with no trace at all**, so
/// every miss must be observable; but a genuinely headless node (a
/// Lighthouse/Server) re-probes forever, and logging the same "no desktop here"
/// verdict every poll would be its own noise. So a miss is tagged
/// [`MissKind::Steady`] (the expected steady state on a headless/pre-desktop
/// node — the worker logs it **once per state-transition**, edge-triggered) vs.
/// [`MissKind::Anomalous`] (a session resolved but couldn't be driven — a real
/// problem worth a line every time it recurs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverMiss {
    /// Steady-state (headless / pre-desktop) vs. an anomalous half-resolved
    /// session — the worker uses this to decide log cadence.
    pub kind: MissKind,
    /// Human-readable reason, ready to log (no per-call formatting at the call
    /// site).
    pub reason: &'static str,
}

/// Whether a [`DiscoverMiss`] is the expected steady state (log once per edge)
/// or an anomaly (log every recurrence). See [`DiscoverMiss`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissKind {
    /// Expected on a headless / pre-desktop node — `loginctl` reported no
    /// logind, or no session is a drivable Wayland session. The worker idles
    /// and only logs this on the transition INTO it, not every poll.
    Steady,
    /// A session WAS resolved but can't be driven yet (its `wayland-*` socket
    /// isn't up, or its uid is absent from `/etc/passwd`). Unexpected enough to
    /// log every time it recurs (WARN at the worker).
    Anomalous,
}

/// Discover the active graphical session the daemon should spawn into.
///
/// Shells `loginctl`, reads the session's `XDG_RUNTIME_DIR`, and resolves the
/// owning uid's gid + home from `/etc/passwd`.
///
/// **Blocking** — forks `loginctl` and reads files; the async caller wraps it
/// in `spawn_blocking` so it never parks a tokio worker thread.
///
/// # Errors
///
/// Returns a [`DiscoverMiss`] (worker idles) when:
///   * `loginctl` is unavailable / there is no logind (minimal container) —
///     [`MissKind::Steady`], or
///   * no session is a drivable Wayland session (genuinely headless node) —
///     [`MissKind::Steady`], or
///   * the winning session has no `wayland-*` socket yet (desktop still coming
///     up — the worker retries on its respawn tick) — [`MissKind::Anomalous`],
///     or
///   * the owning uid has no `/etc/passwd` entry (we decline rather than spawn
///     a half-credentialed child) — [`MissKind::Anomalous`].
///
/// **Observability.** Discovery never fails silently — `Ok` carries the chosen
/// session (the worker logs the uid + `WAYLAND_DISPLAY` at INFO when it spawns)
/// and `Err` carries the reason the worker logs (this is what the CLIP bug
/// lacked — a `list-sessions` JSON parse that fell to an empty vec on Eagle with
/// no trace at all). The worker logs a [`MissKind::Steady`] miss only on the
/// edge into it (a headless node doesn't re-log every 20 s poll) and an
/// [`MissKind::Anomalous`] miss every recurrence.
pub fn discover() -> Result<GraphicalSession, DiscoverMiss> {
    let Some(list) = loginctl_list_sessions() else {
        return Err(DiscoverMiss {
            kind: MissKind::Steady,
            reason: "no logind (loginctl unavailable / failed) — no graphical session; idling",
        });
    };
    let Some(chosen) = parse_graphical_session(&list, loginctl_show_session) else {
        return Err(DiscoverMiss {
            kind: MissKind::Steady,
            reason: "no drivable wayland session (headless / pre-desktop) — idling",
        });
    };
    let Some(wayland_display) = pick_wayland_socket(&list_runtime_dir(&chosen.runtime_dir)) else {
        return Err(DiscoverMiss {
            kind: MissKind::Anomalous,
            reason: "graphical session has no wayland-* socket yet (display coming up) — idling",
        });
    };
    let Some(id) = passwd_identity(chosen.uid) else {
        return Err(DiscoverMiss {
            kind: MissKind::Anomalous,
            reason: "session uid not in /etc/passwd — declining a half-credentialed capture child",
        });
    };
    Ok(GraphicalSession {
        uid: chosen.uid,
        gid: id.gid,
        home: id.home,
        runtime_dir: chosen.runtime_dir,
        wayland_display,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The VERBATIM `loginctl list-sessions --no-legend --no-pager -o json`
    /// output captured live on Eagle (systemd 259, Fedora 44) — the host where
    /// the JSON parse silently returned empty. This is the legacy whitespace
    /// table, NOT JSON; the session id is the first token of each row.
    const EAGLE_LIST_TABLE: &str = "10 1000 mm -     39237 user    - no -\n\
         2 1000 mm seat0 2666  user    - no -\n\
         3 1000 mm -     2937  manager - no -\n";

    #[test]
    fn parse_session_ids_from_real_eagle_table() {
        // The whole point of the fix: the live table path MUST yield ids. Eagle
        // emits sessions 10, 2, 3 — first whitespace token of each row.
        let ids = parse_session_ids(EAGLE_LIST_TABLE);
        assert_eq!(ids, vec!["10", "2", "3"]);
    }

    #[test]
    fn parse_session_ids_tolerates_json_array_fallback() {
        // A host that genuinely honours `-o json` still parses (the `session`
        // field is the id). Kept because we still ask loginctl for json.
        let json = r#"[
            {"session":"2","uid":1000,"user":"mm","seat":"seat0"},
            {"session":"c1","uid":42,"user":"gdm","seat":"seat0"}
        ]"#;
        assert_eq!(parse_session_ids(json), vec!["2", "c1"]);
    }

    #[test]
    fn parse_session_ids_empty_on_no_sessions() {
        assert!(parse_session_ids("").is_empty());
        assert!(parse_session_ids("\n  \n").is_empty());
        assert!(parse_session_ids("[]").is_empty());
    }

    #[test]
    fn parse_props_reads_type_state_seat_uid_and_runtime_path() {
        // The reliable `show-session` dump on Eagle — uid is `User=`, the seat
        // is `Seat=`; both now flow from here (not from list-sessions).
        let show = "Id=2\nUser=1000\nName=mm\nType=wayland\nState=active\n\
                    RuntimePath=/run/user/1000\nSeat=seat0\nActive=yes\nLeader=2666\n";
        let p = parse_session_props(show);
        assert_eq!(p.session_type, "wayland");
        assert_eq!(p.state, "active");
        assert_eq!(p.seat, "seat0");
        assert_eq!(p.uid, Some(1000));
        assert_eq!(p.runtime_path, "/run/user/1000");
    }

    #[test]
    fn drivable_requires_wayland_and_live_state() {
        let props = |ty: &str, state: &str| SessionProps {
            session_type: ty.into(),
            state: state.into(),
            ..Default::default()
        };
        assert!(is_drivable_graphical(&props("wayland", "active")));
        assert!(
            is_drivable_graphical(&props("wayland", "online")),
            "online is still live"
        );
        assert!(
            !is_drivable_graphical(&props("x11", "active")),
            "x11 is not wl-paste-drivable"
        );
        assert!(!is_drivable_graphical(&props("wayland", "closing")));
    }

    #[test]
    fn pick_socket_prefers_lowest_numbered_and_ignores_locks() {
        let names = vec![
            "wayland-1".to_string(),
            "wayland-1.lock".to_string(),
            "wayland-0".to_string(),
            "wayland-0.lock".to_string(),
            "bus".to_string(),
            "pipewire-0".to_string(),
        ];
        assert_eq!(pick_wayland_socket(&names), Some("wayland-0".to_string()));
    }

    #[test]
    fn pick_socket_none_when_no_wayland_socket() {
        let names = vec!["bus".to_string(), "pulse".to_string()];
        assert_eq!(pick_wayland_socket(&names), None);
        // A bare lock file with no live socket → still None (display not up yet).
        let only_lock = vec!["wayland-0.lock".to_string()];
        assert_eq!(pick_wayland_socket(&only_lock), None);
    }

    #[test]
    fn graphical_session_discovered_from_verbatim_eagle_table() {
        // THE REGRESSION TEST. Drive the whole pure core off the VERBATIM Eagle
        // `list-sessions` table (not mocked JSON — that is precisely how the bug
        // shipped) plus the reliable `show-session` dumps Eagle prints. Session
        // "2" (uid 1000, seat0, active wayland) must be discovered; the seatless
        // "10" + the (non-graphical) "manager" "3" lose.
        let show = |id: &str| {
            Some(match id {
                // session 10: a seatless wayland session (the operator's too,
                // but seatless) — loses the seat0 tiebreak.
                "10" => "Type=wayland\nState=online\nSeat=\nUser=1000\n\
                         Name=mm\nRuntimePath=/run/user/1000\n"
                    .to_string(),
                // session 2: the physical seat0 desktop — the winner.
                "2" => "Seat=seat0\nLeader=2666\nType=wayland\nClass=user\n\
                        Active=yes\nState=active\nUser=1000\nName=mm\n\
                        RuntimePath=/run/user/1000\n"
                    .to_string(),
                // session 3: the user@.service manager scope — not a wayland
                // session, so not drivable.
                "3" => "Type=unspecified\nState=active\nUser=1000\nName=mm\n".to_string(),
                _ => String::new(),
            })
        };
        let chosen = parse_graphical_session(EAGLE_LIST_TABLE, show).unwrap();
        assert_eq!(chosen.uid, 1000);
        assert_eq!(chosen.runtime_dir, PathBuf::from("/run/user/1000"));
    }

    #[test]
    fn graphical_session_picks_active_seat0_wayland_uid() {
        // Two sessions: gdm greeter (uid 42, online) + the operator (uid 1000,
        // active). Both seat0 + wayland → the operator (regular uid + active) wins.
        let list = "c1\n2\n";
        let show = |id: &str| {
            Some(match id {
                "c1" => "Type=wayland\nState=online\nSeat=seat0\nUser=42\n\
                         RuntimePath=/run/user/42\n"
                    .to_string(),
                "2" => "Type=wayland\nState=active\nSeat=seat0\nUser=1000\n\
                        RuntimePath=/run/user/1000\n"
                    .to_string(),
                _ => String::new(),
            })
        };
        let chosen = parse_graphical_session(list, show).unwrap();
        assert_eq!(chosen.uid, 1000);
        assert_eq!(chosen.runtime_dir, PathBuf::from("/run/user/1000"));
    }

    #[test]
    fn graphical_session_regular_uid_beats_system_greeter_even_when_greeter_active() {
        // The Eagle-window bug: the gdm greeter (uid 42) is the ACTIVE seat0
        // wayland session while the operator's (uid 1000) is merely `online`.
        // Without the regular-uid preference the greeter would win and the
        // worker would watch the greeter's clipboard → operator clips lost.
        let list = "greeter\nuser\n";
        let show = |id: &str| {
            Some(match id {
                "greeter" => "Type=wayland\nState=active\nSeat=seat0\nUser=42\n".to_string(),
                "user" => "Type=wayland\nState=online\nSeat=seat0\nUser=1000\n".to_string(),
                _ => String::new(),
            })
        };
        assert_eq!(parse_graphical_session(list, show).unwrap().uid, 1000);
    }

    #[test]
    fn graphical_session_falls_back_to_run_user_when_no_runtime_path() {
        // logind didn't report RuntimePath → derive /run/user/<uid>.
        let list = "2\n";
        let show =
            |_: &str| Some("Type=wayland\nState=active\nSeat=seat0\nUser=1000\n".to_string());
        let chosen = parse_graphical_session(list, show).unwrap();
        assert_eq!(chosen.runtime_dir, PathBuf::from("/run/user/1000"));
    }

    #[test]
    fn graphical_session_skips_session_with_unreadable_uid() {
        // A drivable wayland session whose `show-session` omits `User=` is
        // skipped (we won't spawn a credential drop to an unknown uid) rather
        // than defaulting to a bogus uid.
        let list = "ghost\n2\n";
        let show = |id: &str| {
            Some(match id {
                "ghost" => "Type=wayland\nState=active\nSeat=seat0\n".to_string(),
                "2" => "Type=wayland\nState=active\nSeat=seat0\nUser=1000\n".to_string(),
                _ => String::new(),
            })
        };
        assert_eq!(parse_graphical_session(list, show).unwrap().uid, 1000);
    }

    #[test]
    fn graphical_session_prefers_seat0_over_seatless() {
        // A seatless active wayland session (e.g. a nested/headless compositor)
        // loses to the seat0 one — the physical desktop is what the operator sees.
        // (Both regular uids, so the seat0 bit is the tiebreak.)
        let list = "nested\n2\n";
        let show = |id: &str| {
            Some(match id {
                "nested" => "Type=wayland\nState=active\nSeat=\nUser=1001\n".to_string(),
                "2" => "Type=wayland\nState=active\nSeat=seat0\nUser=1000\n".to_string(),
                _ => String::new(),
            })
        };
        assert_eq!(parse_graphical_session(list, show).unwrap().uid, 1000);
    }

    #[test]
    fn graphical_session_none_when_headless() {
        // tty / x11 only — nothing wl-paste can drive → None (worker idles).
        let list = "1\n3\n";
        let show = |id: &str| {
            Some(match id {
                "1" => "Type=tty\nState=active\nSeat=seat0\nUser=1000\n".to_string(),
                "3" => "Type=x11\nState=active\nSeat=\nUser=1000\n".to_string(),
                _ => String::new(),
            })
        };
        assert_eq!(parse_graphical_session(list, show), None);
    }

    #[test]
    fn graphical_session_none_on_empty_list() {
        assert_eq!(parse_graphical_session("", |_| Some(String::new())), None);
        assert_eq!(parse_graphical_session("[]", |_| Some(String::new())), None);
    }

    #[test]
    fn passwd_parse_reads_gid_and_home_for_uid() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      gdm:x:42:42:GDM:/var/lib/gdm:/usr/sbin/nologin\n\
                      mm:x:1000:1001:Matthew:/home/mm:/bin/bash\n";
        let id = parse_passwd_for_uid(passwd, 1000).unwrap();
        assert_eq!(id.gid, 1001);
        assert_eq!(id.home, PathBuf::from("/home/mm"));
        // A different uid resolves independently.
        assert_eq!(parse_passwd_for_uid(passwd, 42).unwrap().gid, 42);
        // Absent uid → None.
        assert_eq!(parse_passwd_for_uid(passwd, 9999), None);
    }

    #[test]
    fn passwd_parse_skips_malformed_lines() {
        let passwd = "# a comment\n\
                      truncated:x:1000\n\
                      mm:x:1000:1001:Matthew:/home/mm:/bin/bash\n";
        let id = parse_passwd_for_uid(passwd, 1000).unwrap();
        assert_eq!(id.gid, 1001);
        assert_eq!(id.home, PathBuf::from("/home/mm"));
    }
}
