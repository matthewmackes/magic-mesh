//! Fleet revision DISPLAY model — legacy `r-YYYY-MM-DD-NNNN` ids.
//!
//! **FPG-1 (2026-06-09): retired to display-only.** The canonical
//! fleet revision identity is `magic_fleet::Revision::version` — a
//! monotonic `u64` minted via `magic_fleet::store::next_version` and
//! logged append-only on the Syncthing-replicated workgroup root (FPG-2,
//! `magic_fleet::store`). The date-string scheme here (and the SQLite
//! rowid scheme in `fleet.rs`) survive only as human-facing display
//! fields / per-node read-mirror keys; nothing elects or orders by
//! them. New code reads + writes the `magic_fleet` model.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stable revision id — `r-YYYY-MM-DD-NNNN` per the worklist's
/// shape. Stored as a plain string so the operator can read the
/// date inline without a lookup.
pub type RevisionId = String;

/// One revision row. The payload is the full serialized desired
/// config at the moment of write — not a delta — so rollback is
/// always a copy operation, never a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    /// Stable id, e.g. `r-2026-05-19-0042`.
    pub id: RevisionId,
    /// Human author (operator name or `automatic`).
    pub author: String,
    /// Free-form summary the author provided.
    pub summary: String,
    /// Unix epoch milliseconds.
    pub created_at: i64,
    /// Full serialized desired config at this revision.
    pub payload_json: String,
}

/// Field-level diff between two revisions. We use a flat
/// path-keyed map so the GUI's side-by-side view can render each
/// changed key on its own row.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RevisionDiff {
    /// Source revision (the older one).
    pub from: RevisionId,
    /// Target revision (the newer one).
    pub to: RevisionId,
    /// Keys whose value changed: path → (from_value, to_value).
    pub changed: BTreeMap<String, (String, String)>,
    /// Keys present in `to` but not in `from`.
    pub added: BTreeMap<String, String>,
    /// Keys present in `from` but not in `to`.
    pub removed: BTreeMap<String, String>,
}

/// Compute a flat diff between two revisions' payloads. Walks the
/// top-level JSON objects + stringifies values. Sufficient for the
/// GUI's diff viewer (12.8.3); deep-structure diffs land if needed.
///
/// # Errors
/// Returns the underlying `serde_json::Error` when either payload
/// isn't a valid JSON object.
pub fn diff(from: &Revision, to: &Revision) -> Result<RevisionDiff, serde_json::Error> {
    let from_obj: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&from.payload_json).unwrap_or_default();
    let to_obj: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&to.payload_json).unwrap_or_default();

    let mut out = RevisionDiff {
        from: from.id.clone(),
        to: to.id.clone(),
        ..Default::default()
    };

    for (k, v) in &from_obj {
        match to_obj.get(k) {
            None => {
                out.removed.insert(k.clone(), v.to_string());
            }
            Some(tv) if tv != v => {
                out.changed
                    .insert(k.clone(), (v.to_string(), tv.to_string()));
            }
            _ => {}
        }
    }
    for (k, v) in &to_obj {
        if !from_obj.contains_key(k) {
            out.added.insert(k.clone(), v.to_string());
        }
    }
    Ok(out)
}

/// Allocate the next revision id given the most recent existing id.
/// Format: `r-YYYY-MM-DD-NNNN` where `NNNN` is a 4-digit zero-padded
/// counter that increments within the day. Wraps to the next day's
/// `-0001` when `prev_date` is older than `today`.
#[must_use]
pub fn next_revision_id(today: &str, prev: Option<&str>) -> RevisionId {
    let counter = prev
        .and_then(|p| {
            let parts: Vec<&str> = p.split('-').collect();
            if parts.len() == 5 && parts[1..4].join("-") == today {
                parts[4].parse::<u32>().ok()
            } else {
                None
            }
        })
        .map_or(1, |c| c + 1);
    format!("r-{today}-{counter:04}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rev(id: &str, payload: &str) -> Revision {
        Revision {
            id: id.to_owned(),
            author: "test".into(),
            summary: String::new(),
            created_at: 0,
            payload_json: payload.to_owned(),
        }
    }

    #[test]
    fn empty_diff_when_payloads_match() {
        let a = rev("r-2026-05-19-0001", r#"{"k":"v"}"#);
        let b = rev("r-2026-05-19-0002", r#"{"k":"v"}"#);
        let d = diff(&a, &b).unwrap();
        assert!(d.changed.is_empty());
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
    }

    #[test]
    fn diff_detects_changed_value() {
        let a = rev("r1", r#"{"k":"old"}"#);
        let b = rev("r2", r#"{"k":"new"}"#);
        let d = diff(&a, &b).unwrap();
        assert_eq!(d.changed.len(), 1);
        let (from_v, to_v) = &d.changed["k"];
        assert!(from_v.contains("old"));
        assert!(to_v.contains("new"));
    }

    #[test]
    fn diff_detects_added_key() {
        let a = rev("r1", r#"{"a":"1"}"#);
        let b = rev("r2", r#"{"a":"1","b":"2"}"#);
        let d = diff(&a, &b).unwrap();
        assert_eq!(d.added.len(), 1);
        assert!(d.added.contains_key("b"));
    }

    #[test]
    fn diff_detects_removed_key() {
        let a = rev("r1", r#"{"a":"1","b":"2"}"#);
        let b = rev("r2", r#"{"a":"1"}"#);
        let d = diff(&a, &b).unwrap();
        assert_eq!(d.removed.len(), 1);
        assert!(d.removed.contains_key("b"));
    }

    #[test]
    fn next_revision_id_starts_at_0001() {
        assert_eq!(next_revision_id("2026-05-19", None), "r-2026-05-19-0001");
    }

    #[test]
    fn next_revision_id_increments_within_day() {
        let next = next_revision_id("2026-05-19", Some("r-2026-05-19-0007"));
        assert_eq!(next, "r-2026-05-19-0008");
    }

    #[test]
    fn next_revision_id_resets_on_new_day() {
        let next = next_revision_id("2026-05-20", Some("r-2026-05-19-9999"));
        assert_eq!(next, "r-2026-05-20-0001");
    }

    #[test]
    fn next_revision_id_falls_to_0001_on_malformed_prev() {
        // Wrong number of parts → can't parse counter → starts at 1.
        assert_eq!(
            next_revision_id("2026-05-20", Some("not-a-revision-id")),
            "r-2026-05-20-0001"
        );
    }

    #[test]
    fn next_revision_id_falls_to_0001_when_counter_unparseable() {
        // Right shape, but counter isn't a number.
        assert_eq!(
            next_revision_id("2026-05-19", Some("r-2026-05-19-XXXX")),
            "r-2026-05-19-0001"
        );
    }

    #[test]
    fn diff_treats_invalid_json_as_empty_object() {
        // Either payload not a JSON object → diff sees an empty map
        // on that side. No error surfaces — the function returns Ok.
        let a = rev("r1", "not json at all");
        let b = rev("r2", r#"{"k":"v"}"#);
        let d = diff(&a, &b).unwrap();
        // `k` shows up as added (a's object is empty).
        assert!(d.added.contains_key("k"));
        assert!(d.changed.is_empty());
        assert!(d.removed.is_empty());
    }

    #[test]
    fn diff_with_both_payloads_invalid_returns_empty_diff() {
        let a = rev("r1", "garbage");
        let b = rev("r2", "more garbage");
        let d = diff(&a, &b).unwrap();
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
        assert!(d.changed.is_empty());
        assert_eq!(d.from, "r1");
        assert_eq!(d.to, "r2");
    }

    #[test]
    fn revision_round_trips_through_json() {
        let r = Revision {
            id: "r-2026-05-19-0001".into(),
            author: "alice".into(),
            summary: "first revision".into(),
            created_at: 1_700_000_000_000,
            payload_json: r#"{"k":"v"}"#.into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: Revision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn diff_combined_changed_added_removed() {
        let a = rev("r1", r#"{"a":"1","b":"2"}"#);
        let b = rev("r2", r#"{"a":"changed","c":"3"}"#);
        let d = diff(&a, &b).unwrap();
        assert!(d.changed.contains_key("a"));
        assert!(d.added.contains_key("c"));
        assert!(d.removed.contains_key("b"));
    }
}
