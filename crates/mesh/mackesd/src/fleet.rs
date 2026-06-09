//! v2.0.0 Phase G — fleet-managed config layer.
//!
//! Today's `desired_config` table is the authoritative store of
//! every settings revision the reconcile loop will apply. The fleet
//! module is the thin write-side layer that operators reach for
//! through the `mded fleet push-setting` CLI (Phase G.4) and the
//! `dev.mackes.MDE.Fleet` zbus surface (Phase A.3).
//!
//! Phase G.1 / G.2 / G.3 (extend `DesiredSnapshot` with
//! `settings_keys`, hook reconcile, extend validation) hang off the
//! `PushPlan` value produced by [`plan_push`]; this module owns the
//! pure plan-builder + the SQL-writer.

use std::collections::BTreeSet;

use anyhow::Context;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::Result;

/// Plan produced by [`plan_push`]. Pure value type — no I/O or
/// database handles — so callers can preview a push (CLI dry-run,
/// zbus introspection) without touching the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushPlan {
    /// Dot-notated setting key (e.g. `theme.accent`).
    pub key: String,
    /// JSON-encoded value payload.
    pub value: String,
    /// Peer ids the push targets, lexicographically ordered.
    /// `["all"]` is the canonical "every healthy peer" sentinel —
    /// the reconcile loop expands it at apply time so the plan stays
    /// stable even when the fleet membership churns between push and
    /// apply.
    pub peers: Vec<String>,
    /// Author tag (typically `peer:<hostname>`).
    pub author: String,
    /// Provisional revision id — the SQL writer allocates the real
    /// id when the row lands. The plan stamps a deterministic
    /// preview shape (`fleet-push-<key>`) so dry-run output is
    /// addressable.
    pub revision_id: String,
}

/// Pure constructor: build a [`PushPlan`] from the CLI's raw inputs.
/// No store access — caller decides whether to persist via
/// [`record_push`].
#[must_use]
pub fn plan_push(key: &str, value: &str, peers: &str, author: &str) -> PushPlan {
    let peers = parse_peers(peers);
    PushPlan {
        key: key.to_owned(),
        value: value.to_owned(),
        peers,
        author: author.to_owned(),
        revision_id: format!("fleet-push-{}", sanitize_revision_segment(key)),
    }
}

/// Persist a push plan to the store. Writes one `desired_config`
/// row carrying the JSON `{key, value}` payload + one
/// `fleet_settings_apply_log` row per (peer, key) target with
/// `ok = 0` (the reconcile loop flips it to 1 on success). Atomic
/// via [`crate::store::with_transaction`].
///
/// # Errors
///
/// Returns an error when any of the writes fails. Atomic — if any
/// row rejects, none of them land.
pub fn record_push(conn: &mut Connection, plan: &PushPlan) -> Result<i64> {
    crate::store::with_transaction(conn, |tx| {
        let now = chrono::Utc::now().to_rfc3339();
        let payload = serde_json::json!({
            "key":   &plan.key,
            "value": &plan.value,
            "peers": &plan.peers,
        })
        .to_string();
        tx.execute(
            "INSERT INTO desired_config \
             (author, message, spec_json, state, created_at) \
             VALUES (?, ?, ?, 'approved', ?)",
            (
                &plan.author,
                &format!("fleet push: {}", plan.key),
                &payload,
                &now,
            ),
        )
        .with_context(|| format!("inserting desired_config for {}", plan.key))?;
        let revision_id = tx.last_insert_rowid();
        for peer in &plan.peers {
            tx.execute(
                "INSERT INTO fleet_settings_apply_log \
                 (peer_id, revision_id, key, applied_at, ok) \
                 VALUES (?, ?, ?, ?, 0)",
                (peer, revision_id.to_string(), &plan.key, &now),
            )
            .with_context(|| {
                format!(
                    "inserting fleet_settings_apply_log row for {peer}/{}",
                    plan.key
                )
            })?;
        }
        Ok(revision_id)
    })
}

/// Parse the `--peers` CLI option into a sorted-deduped peer-id
/// list. `"all"` lowers to the single-element list `["all"]` (the
/// reconcile loop expands at apply time). Commas + whitespace
/// separate; empty tokens are skipped.
fn parse_peers(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("all") {
        return vec!["all".to_owned()];
    }
    let mut set: BTreeSet<String> = BTreeSet::new();
    for token in trimmed.split([',', ' ', '\t']) {
        let t = token.trim();
        if !t.is_empty() {
            set.insert(t.to_owned());
        }
    }
    set.into_iter().collect()
}

/// Normalize a setting key for use in the preview revision-id
/// string. Replaces non-`[a-z0-9_-]` chars with `_` so the preview
/// stays grep-friendly.
fn sanitize_revision_segment(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for c in key.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_peers_handles_all_keyword() {
        assert_eq!(parse_peers("all"), vec!["all".to_owned()]);
        assert_eq!(parse_peers("  all  "), vec!["all".to_owned()]);
        assert_eq!(parse_peers("ALL"), vec!["all".to_owned()]);
    }

    #[test]
    fn parse_peers_splits_and_dedupes() {
        let got = parse_peers("peer:a, peer:b, peer:a");
        assert_eq!(got, vec!["peer:a".to_owned(), "peer:b".to_owned()]);
    }

    #[test]
    fn parse_peers_ignores_empty_tokens() {
        assert_eq!(parse_peers(",,,peer:x,,"), vec!["peer:x".to_owned()]);
        assert_eq!(parse_peers(""), Vec::<String>::new());
    }

    #[test]
    fn parse_peers_handles_whitespace_separators() {
        let got = parse_peers("peer:a\tpeer:b peer:c");
        assert_eq!(
            got,
            vec![
                "peer:a".to_owned(),
                "peer:b".to_owned(),
                "peer:c".to_owned()
            ]
        );
    }

    #[test]
    fn sanitize_revision_segment_keeps_safe_chars() {
        assert_eq!(sanitize_revision_segment("theme.accent"), "theme_accent",);
        assert_eq!(sanitize_revision_segment("ok-key_1"), "ok-key_1");
        assert_eq!(sanitize_revision_segment("path/to/key"), "path_to_key");
    }

    #[test]
    fn plan_push_builds_expected_shape_for_all_target() {
        let plan = plan_push("theme.accent", "\"#ff00aa\"", "all", "peer:host");
        assert_eq!(plan.key, "theme.accent");
        assert_eq!(plan.peers, vec!["all".to_owned()]);
        assert_eq!(plan.author, "peer:host");
        assert_eq!(plan.revision_id, "fleet-push-theme_accent");
    }

    #[test]
    fn plan_push_builds_expected_shape_for_explicit_peers() {
        let plan = plan_push("font.size", "13", "peer:c, peer:a, peer:b", "alice");
        assert_eq!(
            plan.peers,
            vec![
                "peer:a".to_owned(),
                "peer:b".to_owned(),
                "peer:c".to_owned(),
            ]
        );
    }

    #[test]
    fn record_push_writes_one_revision_plus_per_peer_log_rows() {
        let mut conn = crate::store::open_in_memory().expect("open");
        let plan = plan_push("theme.mode", r#""dark""#, "peer:a, peer:b", "peer:host");
        let revision_id = record_push(&mut conn, &plan).expect("record");
        let dc_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM desired_config WHERE revision_id = ?",
                [revision_id],
                |r| r.get(0),
            )
            .expect("count desired_config");
        assert_eq!(dc_count, 1);
        let log_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fleet_settings_apply_log WHERE revision_id = ?",
                [revision_id.to_string()],
                |r| r.get(0),
            )
            .expect("count log rows");
        assert_eq!(log_count, 2, "one log row per targeted peer");
    }

    #[test]
    fn record_push_revision_state_is_approved() {
        let mut conn = crate::store::open_in_memory().expect("open");
        let plan = plan_push("theme.name", r#""Mackes""#, "all", "alice");
        let revision_id = record_push(&mut conn, &plan).expect("record");
        let state: String = conn
            .query_row(
                "SELECT state FROM desired_config WHERE revision_id = ?",
                [revision_id],
                |r| r.get(0),
            )
            .expect("state");
        assert_eq!(state, "approved");
    }

    #[test]
    fn push_plan_round_trips_through_serde() {
        let plan = plan_push("k", "v", "peer:a,peer:b", "alice");
        let json = serde_json::to_string(&plan).expect("ser");
        let back: PushPlan = serde_json::from_str(&json).expect("de");
        assert_eq!(plan, back);
    }
}
