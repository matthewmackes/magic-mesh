//! Card schema migration registry (Portal-31, R10-Q36).
//!
//! Every Card carries `schema_version`. Consumers run [`migrate`]
//! before reading; the function looks up the migration chain in
//! [`MIGRATIONS`] and walks each step forward to the current
//! [`SCHEMA_VERSION`].
//!
//! Today only schema_version = 1 exists, so [`MIGRATIONS`] is empty
//! and [`migrate`] is a fast pass-through. When v2 lands, append a
//! `Migration { from: 1, to: 2, apply: … }` row and add a test that
//! round-trips a v1 payload through the new step.

use serde_json::Value;
use std::fmt;

/// Current Card schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// Errors emitted by [`migrate`].
#[derive(Debug)]
pub enum MigrationError {
    /// Card lacked a `schema_version` field and we couldn't infer one.
    Missing,
    /// Card's `schema_version` is newer than [`SCHEMA_VERSION`] — the
    /// reader is older than the writer and can't safely upgrade.
    FromFuture { found: u32, current: u32 },
    /// No migration step exists for a transition we expected to find.
    NoStepFor { from: u32, to: u32 },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "card is missing the schema_version field"),
            Self::FromFuture { found, current } => write!(
                f,
                "card schema_version {found} is newer than this build's {current} — \
                 upgrade the reader or drop the card"
            ),
            Self::NoStepFor { from, to } => {
                write!(f, "no migration step registered for {from} → {to}")
            }
        }
    }
}

impl std::error::Error for MigrationError {}

/// One forward migration step.
pub struct Migration {
    /// Schema version this step starts from.
    pub from: u32,
    /// Schema version this step lands on.
    pub to: u32,
    /// Transform fn: mutate the JSON in place so the result is valid
    /// at the `to` schema version.
    pub apply: fn(&mut Value) -> Result<(), MigrationError>,
}

/// Registered migration steps. Walked in order.
pub const MIGRATIONS: &[Migration] = &[
    // No migrations yet — schema_version = 1 is the floor.
];

/// Walk `raw` forward to [`SCHEMA_VERSION`].
///
/// Mutates `raw` in place. Returns the final schema_version on
/// success.
pub fn migrate(raw: &mut Value) -> Result<u32, MigrationError> {
    let mut current = raw
        .get("schema_version")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .ok_or(MigrationError::Missing)?;

    if current > SCHEMA_VERSION {
        return Err(MigrationError::FromFuture {
            found: current,
            current: SCHEMA_VERSION,
        });
    }

    while current < SCHEMA_VERSION {
        let step =
            MIGRATIONS
                .iter()
                .find(|m| m.from == current)
                .ok_or(MigrationError::NoStepFor {
                    from: current,
                    to: current + 1,
                })?;
        (step.apply)(raw)?;
        current = step.to;
        if let Some(slot) = raw.get_mut("schema_version") {
            *slot = serde_json::json!(current);
        }
    }

    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_passes_through_current_version() {
        let mut raw = serde_json::json!({
            "id": "abc",
            "schema_version": SCHEMA_VERSION,
            "kind": "note",
            "title": "t",
        });
        let v = migrate(&mut raw).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn migrate_errors_on_missing_version() {
        let mut raw = serde_json::json!({ "kind": "note", "title": "t" });
        let err = migrate(&mut raw).unwrap_err();
        assert!(matches!(err, MigrationError::Missing));
    }

    #[test]
    fn migrate_errors_on_future_version() {
        let mut raw = serde_json::json!({
            "id": "abc",
            "schema_version": SCHEMA_VERSION + 5,
            "kind": "note",
            "title": "t",
        });
        let err = migrate(&mut raw).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::FromFuture {
                found,
                current
            } if found == SCHEMA_VERSION + 5 && current == SCHEMA_VERSION
        ));
    }

    #[test]
    fn current_schema_version_is_one() {
        // Lock the v1 floor. Bumping this requires adding a migration
        // step + extending these tests.
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn migration_step_chain_is_dense() {
        // If MIGRATIONS ever ships steps, they must cover every
        // intermediate version with no gaps.
        if MIGRATIONS.is_empty() {
            return;
        }
        let mut steps: Vec<_> = MIGRATIONS.iter().map(|m| (m.from, m.to)).collect();
        steps.sort();
        for w in steps.windows(2) {
            assert_eq!(w[0].1, w[1].0, "migration chain has a gap at {:?}", w);
        }
    }

    #[test]
    fn migration_steps_advance_by_one() {
        for m in MIGRATIONS {
            assert_eq!(m.to, m.from + 1, "step must move exactly one version");
        }
    }
}
