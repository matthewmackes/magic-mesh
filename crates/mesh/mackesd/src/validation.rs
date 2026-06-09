//! Validation layer (Phase 12.7).
//!
//! Three categories — schema (12.7.1), policy (12.7.2 — see
//! [`crate::policy::detect_conflicts`]), and topology (12.7.3).
//! This module owns the schema and topology pieces.

use crate::topology::{DesiredSnapshot, Node};
use std::collections::HashSet;

/// One validation problem. The errors are accumulated by the
/// validators below (we don't short-circuit on the first finding —
/// operators want to see every problem at once so they can fix
/// them in a single edit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// A required string field was empty.
    EmptyRequiredField {
        /// JSON-path-like location, e.g. `nodes[2].id`.
        path: String,
    },
    /// Two nodes carry the same id.
    DuplicateNodeId {
        /// The duplicated id.
        id: String,
    },
    /// A peer reference points at a node id that doesn't exist.
    UnknownPeerReference {
        /// The dangling reference.
        target_id: String,
        /// Where it appeared (e.g. `routes.peer:anvil`).
        source: String,
    },
    /// A region pair in `allow_east_west` mentions a region no node
    /// claims. Most likely a typo.
    UnknownRegion {
        /// The unrecognized region name.
        region: String,
    },
    /// A node lists itself as its own peer (a self-loop). The
    /// topology engine collapses these silently but flagging at
    /// validation time gives the operator a clear error.
    SelfPeering {
        /// Offending node id.
        id: String,
    },
    /// v2.0.0 Phase G.3 — `settings_keys` contains a key that
    /// isn't a known `SettingKey` variant.
    UnknownSettingKey {
        /// The unrecognized dot-notated key.
        key: String,
    },
    /// v2.0.0 Phase G.3 — `settings_keys` contains a value whose
    /// JSON shape doesn't deserialize to a valid `SettingValue`.
    InvalidSettingValue {
        /// The matching setting key (for diagnostics).
        key: String,
        /// Free-form parse-error string.
        reason: String,
    },
}

/// Validate a `DesiredSnapshot` end-to-end. Returns every error
/// found; an empty Vec means the snapshot is clean.
#[must_use]
pub fn validate(snapshot: &DesiredSnapshot) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // 12.7.1 schema-shape checks
    for (i, n) in snapshot.nodes.iter().enumerate() {
        if n.id.trim().is_empty() {
            errors.push(ValidationError::EmptyRequiredField {
                path: format!("nodes[{i}].id"),
            });
        }
        if n.region.trim().is_empty() {
            errors.push(ValidationError::EmptyRequiredField {
                path: format!("nodes[{i}].region"),
            });
        }
    }

    // 12.7.3 topology checks: duplicate ids, unknown refs, self peering, region typos
    let mut seen_ids: HashSet<&str> = HashSet::new();
    for n in &snapshot.nodes {
        if !seen_ids.insert(&n.id) {
            errors.push(ValidationError::DuplicateNodeId { id: n.id.clone() });
        }
    }

    let known_regions: HashSet<&str> = snapshot.nodes.iter().map(|n| n.region.as_str()).collect();
    for (from, to) in &snapshot.allow_east_west {
        if !known_regions.contains(from.as_str()) {
            errors.push(ValidationError::UnknownRegion {
                region: from.clone(),
            });
        }
        if !known_regions.contains(to.as_str()) {
            errors.push(ValidationError::UnknownRegion { region: to.clone() });
        }
    }

    // v2.0.0 Phase G.3 — settings_keys validation. Each (key,
    // value_json) pair must parse to a known SettingKey + a
    // SettingValue whose JSON shape matches the key's expected
    // type. v4.1 (2026-05-24) — shape check landed via
    // settings_value_shape_matches; previously deferred with
    // `let _ = parsed_key;`.
    for (key, value_json) in &snapshot.settings_keys {
        let Ok(parsed_key): Result<crate::settings::SettingKey, _> = key.parse() else {
            errors.push(ValidationError::UnknownSettingKey { key: key.clone() });
            continue;
        };
        let value = match serde_json::from_str::<serde_json::Value>(value_json) {
            Ok(value) => value,
            Err(e) => {
                errors.push(ValidationError::InvalidSettingValue {
                    key: key.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        if let Err(reason) = settings_value_shape_matches(parsed_key, &value) {
            errors.push(ValidationError::InvalidSettingValue {
                key: key.clone(),
                reason,
            });
        }
    }

    errors
}

/// v4.1 (2026-05-24) — per-key shape validation for
/// `SettingValue` payloads. Each variant of [`crate::settings::SettingKey`]
/// has a canonical JSON shape (string / unsigned integer /
/// boolean / float / array of string / object). Returns
/// `Err("expected <shape>; got <kind>")` when the payload's
/// JSON kind doesn't match, `Ok(())` otherwise.
///
/// Closes the previously-deferred shape-check gap captured in
/// the v4.0.1 validation pass (`let _ = parsed_key;` with the
/// "lands when SettingValue carries shape info" comment).
pub fn settings_value_shape_matches(
    key: crate::settings::SettingKey,
    value: &serde_json::Value,
) -> Result<(), String> {
    // Six canonical shapes. Locked by the SettingKey doc-comments
    // — see crates/mackesd/src/settings/mod.rs.
    let shape = key_expected_shape(key);
    let got = describe_value_kind(value);
    let ok = match shape {
        ValueShape::Str => value.is_string(),
        // Bools serialize/deserialize as Value::Bool, never
        // Value::String — strict.
        ValueShape::Bool => value.is_boolean(),
        // Unsigned ints — accept any non-negative integer.
        // serde_json::Value::Number can be i64/u64/f64; accept
        // i64 >= 0 OR u64 OR (f64 with no fractional part >= 0).
        ValueShape::UnsignedInt => is_unsigned_integer(value),
        // Floats — accept any number (int subtype is fine for a
        // float field; e.g. scale=1 is valid even when the JSON
        // serialized to `1` instead of `1.0`).
        ValueShape::Float => value.is_number(),
        // Arrays of string — accept empty arrays + arrays whose
        // every element is a string.
        ValueShape::ArrayOfStr => value
            .as_array()
            .map(|a| a.iter().all(serde_json::Value::is_string))
            .unwrap_or(false),
        // Object — any JSON object.
        ValueShape::Object => value.is_object(),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("expected {}; got {got}", shape.describe(),))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueShape {
    Str,
    Bool,
    UnsignedInt,
    Float,
    ArrayOfStr,
    Object,
}

impl ValueShape {
    fn describe(self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Bool => "boolean",
            Self::UnsignedInt => "unsigned integer",
            Self::Float => "number",
            Self::ArrayOfStr => "array of string",
            Self::Object => "object",
        }
    }
}

fn key_expected_shape(key: crate::settings::SettingKey) -> ValueShape {
    use crate::settings::SettingKey as K;
    match key {
        // Strings — every name / enum-as-string variant.
        K::ThemeName
        | K::ThemeAccent
        | K::ThemeMode
        | K::ThemeIconSet
        | K::FontName
        | K::FontMonospace
        | K::FontHinting
        | K::FontAntialias
        | K::DisplayPrimary
        | K::PowerLidAction
        | K::PowerProfile
        | K::NotificationLocation
        | K::WallpaperPath
        | K::WallpaperMode
        | K::KeyboardXkbLayout => ValueShape::Str,
        // Unsigned integers — brightness, kelvin, idle seconds, ms.
        K::DisplayBrightness
        | K::DisplayNightLightTemp
        | K::PowerSuspendIdleBatteryS
        | K::PowerSuspendIdleAcS
        | K::NotificationDefaultExpireMs
        | K::KeyboardRepeatDelay
        | K::KeyboardRepeatRate => ValueShape::UnsignedInt,
        // Floats — fractional scale factor + libinput pointer accel.
        K::DisplayScale | K::MousePointerAccel => ValueShape::Float,
        // Booleans.
        K::DisplayNightLight
        | K::PowerPresentationMode
        | K::NotificationDoNotDisturb
        | K::AutomountOnInsert
        | K::AutomountOpenOnMount
        | K::AutomountAutorun
        | K::MouseNaturalScroll
        | K::MouseTapToClick
        | K::MouseLeftHanded => ValueShape::Bool,
        // Object — keybinds map.
        K::KeybindsMap => ValueShape::Object,
        // Arrays of string — autostart hidden/extra lists.
        K::AutostartHidden | K::AutostartExtra => ValueShape::ArrayOfStr,
    }
}

fn is_unsigned_integer(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                let _ = u;
                true
            } else if let Some(i) = n.as_i64() {
                i >= 0
            } else if let Some(f) = n.as_f64() {
                // Allow whole-number floats >= 0 (e.g. `1.0`).
                f >= 0.0 && f.fract() == 0.0
            } else {
                false
            }
        }
        _ => false,
    }
}

fn describe_value_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Validate a single node for ad-hoc checks (enrollment, manual
/// add). Same rules as `validate`, scoped to one row.
#[must_use]
pub fn validate_node(n: &Node) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if n.id.trim().is_empty() {
        errors.push(ValidationError::EmptyRequiredField { path: "id".into() });
    }
    if n.region.trim().is_empty() {
        errors.push(ValidationError::EmptyRequiredField {
            path: "region".into(),
        });
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingKey;
    use serde_json::json;

    // ---- v4.1 settings_value_shape_matches coverage ------------

    #[test]
    fn shape_check_accepts_string_for_theme_name() {
        let ok = settings_value_shape_matches(SettingKey::ThemeName, &json!("Mackes-Carbon"));
        assert!(ok.is_ok());
    }

    #[test]
    fn shape_check_rejects_int_for_theme_name() {
        let err = settings_value_shape_matches(SettingKey::ThemeName, &json!(42))
            .expect_err("int is not a string");
        assert!(err.contains("expected string"), "msg: {err}");
        assert!(err.contains("number"), "msg: {err}");
    }

    #[test]
    fn shape_check_rejects_bool_for_brightness() {
        let err = settings_value_shape_matches(SettingKey::DisplayBrightness, &json!(true))
            .expect_err("bool is not an int");
        assert!(err.contains("expected unsigned integer"));
    }

    #[test]
    fn shape_check_accepts_unsigned_int_for_brightness() {
        for raw in [json!(0), json!(50), json!(100), json!(1.0)] {
            assert!(
                settings_value_shape_matches(SettingKey::DisplayBrightness, &raw).is_ok(),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn shape_check_rejects_negative_int_for_brightness() {
        let err = settings_value_shape_matches(SettingKey::DisplayBrightness, &json!(-5))
            .expect_err("negative");
        assert!(err.contains("expected unsigned integer"));
    }

    #[test]
    fn shape_check_accepts_bool_for_do_not_disturb() {
        assert!(
            settings_value_shape_matches(SettingKey::NotificationDoNotDisturb, &json!(true))
                .is_ok()
        );
        assert!(
            settings_value_shape_matches(SettingKey::NotificationDoNotDisturb, &json!(false))
                .is_ok()
        );
    }

    #[test]
    fn shape_check_rejects_string_bool() {
        // JSON booleans should land as Value::Bool. A string
        // "true" is a regression we want flagged.
        let err =
            settings_value_shape_matches(SettingKey::NotificationDoNotDisturb, &json!("true"))
                .expect_err("string is not a bool");
        assert!(err.contains("expected boolean"));
    }

    #[test]
    fn shape_check_accepts_float_for_scale() {
        for raw in [json!(0.5), json!(1.0), json!(1.5), json!(3.0), json!(2)] {
            assert!(
                settings_value_shape_matches(SettingKey::DisplayScale, &raw).is_ok(),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn shape_check_accepts_array_of_strings_for_autostart_hidden() {
        let v = json!(["a.desktop", "b.desktop"]);
        assert!(settings_value_shape_matches(SettingKey::AutostartHidden, &v).is_ok());
        assert!(settings_value_shape_matches(SettingKey::AutostartHidden, &json!([])).is_ok());
    }

    #[test]
    fn shape_check_rejects_mixed_array_for_autostart_hidden() {
        let err = settings_value_shape_matches(
            SettingKey::AutostartHidden,
            &json!(["a.desktop", 7, "c.desktop"]),
        )
        .expect_err("mixed types");
        assert!(err.contains("expected array of string"));
    }

    #[test]
    fn shape_check_accepts_object_for_keybinds_map() {
        let v = json!({"super+Return": "exec foot", "super+d": "exec wofi"});
        assert!(settings_value_shape_matches(SettingKey::KeybindsMap, &v).is_ok());
        assert!(settings_value_shape_matches(SettingKey::KeybindsMap, &json!({})).is_ok());
    }

    #[test]
    fn shape_check_rejects_array_for_keybinds_map() {
        let err = settings_value_shape_matches(SettingKey::KeybindsMap, &json!(["super+Return"]))
            .expect_err("array is not an object");
        assert!(err.contains("expected object"));
    }

    #[test]
    fn shape_check_full_validate_flags_wrong_shape_in_snapshot() {
        // End-to-end: a snapshot whose settings_keys contains a
        // wrongly-typed value surfaces an InvalidSettingValue
        // error from validate() via the new shape check.
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![
                ("theme.name".into(), serde_json::json!(42).to_string()),
                (
                    "display.brightness".into(),
                    serde_json::json!(75).to_string(),
                ),
            ],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        let invalids: Vec<_> = errors
            .iter()
            .filter_map(|e| match e {
                ValidationError::InvalidSettingValue { key, reason } => {
                    Some((key.clone(), reason.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(invalids.len(), 1);
        assert_eq!(invalids[0].0, "theme.name");
        assert!(invalids[0].1.contains("expected string"));
    }

    #[test]
    fn shape_check_full_validate_clean_snapshot_emits_no_setting_errors() {
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![
                (
                    "theme.name".into(),
                    serde_json::json!("Mackes-Carbon").to_string(),
                ),
                (
                    "display.brightness".into(),
                    serde_json::json!(75).to_string(),
                ),
                (
                    "notification.do_not_disturb".into(),
                    serde_json::json!(true).to_string(),
                ),
                (
                    "autostart.hidden".into(),
                    serde_json::json!(["a.desktop"]).to_string(),
                ),
            ],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        let any_setting_error = errors.iter().any(|e| {
            matches!(
                e,
                ValidationError::InvalidSettingValue { .. }
                    | ValidationError::UnknownSettingKey { .. }
            )
        });
        assert!(!any_setting_error, "unexpected errors: {errors:?}");
    }

    fn n(id: &str, region: &str) -> Node {
        Node {
            id: id.to_owned(),
            region: region.to_owned(),
            healthy: true,
            is_host: false,
        }
    }

    #[test]
    fn clean_snapshot_validates() {
        let snap = DesiredSnapshot {
            nodes: vec![n("peer:a", "us-east"), n("peer:b", "us-east")],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        assert!(validate(&snap).is_empty());
    }

    #[test]
    fn empty_id_is_an_error() {
        let snap = DesiredSnapshot {
            nodes: vec![n("", "us-east")],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::EmptyRequiredField { .. })));
    }

    #[test]
    fn duplicate_id_is_an_error() {
        let snap = DesiredSnapshot {
            nodes: vec![n("peer:a", "us-east"), n("peer:a", "us-west")],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        assert!(errors.iter().any(|e| matches!(
            e,
            ValidationError::DuplicateNodeId { id } if id == "peer:a"
        )));
    }

    #[test]
    fn unknown_region_in_allow_list_is_an_error() {
        let snap = DesiredSnapshot {
            nodes: vec![n("peer:a", "us-east")],
            allow_east_west: vec![("us-east".into(), "typo-region".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        assert!(errors.iter().any(|e| matches!(
            e,
            ValidationError::UnknownRegion { region } if region == "typo-region"
        )));
    }

    #[test]
    fn validate_node_catches_individual_errors() {
        let errors = validate_node(&n("", ""));
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn validation_accumulates_does_not_short_circuit() {
        let snap = DesiredSnapshot {
            nodes: vec![n("", ""), n("", "")],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        // 4 empty-field errors (2 nodes × 2 fields each) + 1 duplicate
        // id error (both empty ids count as "peer:" — twice).
        assert_eq!(errors.len(), 5);
    }

    #[test]
    fn empty_region_field_is_an_error() {
        let snap = DesiredSnapshot {
            nodes: vec![n("peer:a", "")],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::EmptyRequiredField { path } if path.ends_with(".region")))
        );
    }

    #[test]
    fn whitespace_only_fields_are_empty() {
        // `.trim().is_empty()` treats whitespace as empty.
        let snap = DesiredSnapshot {
            nodes: vec![n("   ", "\t\t")],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn allow_east_west_with_known_regions_does_not_error() {
        let snap = DesiredSnapshot {
            nodes: vec![n("peer:a", "us-east"), n("peer:b", "us-west")],
            allow_east_west: vec![("us-east".into(), "us-west".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        assert!(validate(&snap).is_empty());
    }

    #[test]
    fn allow_east_west_flags_both_unknown_regions() {
        let snap = DesiredSnapshot {
            nodes: vec![n("peer:a", "us-east")],
            allow_east_west: vec![("typo-a".into(), "typo-b".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let errors = validate(&snap);
        let region_errs: Vec<&str> = errors
            .iter()
            .filter_map(|e| {
                if let ValidationError::UnknownRegion { region } = e {
                    Some(region.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(region_errs.contains(&"typo-a"));
        assert!(region_errs.contains(&"typo-b"));
    }

    #[test]
    fn validate_node_returns_empty_for_valid_node() {
        let valid = n("peer:ok", "us-east");
        assert!(validate_node(&valid).is_empty());
    }

    #[test]
    fn validation_error_round_trips_through_clone() {
        let e = ValidationError::DuplicateNodeId {
            id: "peer:a".into(),
        };
        assert_eq!(e, e.clone());
        let e2 = ValidationError::SelfPeering {
            id: "peer:b".into(),
        };
        let e3 = ValidationError::UnknownPeerReference {
            target_id: "peer:c".into(),
            source: "routes.peer:x".into(),
        };
        // Exercise PartialEq/Clone on every variant so coverage counts.
        assert_eq!(e2, e2.clone());
        assert_eq!(e3, e3.clone());
        assert_ne!(e2, e3);
    }

    #[test]
    fn duplicate_with_more_than_two_collisions() {
        // Three nodes share the same id — should produce 2 dup errors
        // (one per re-insert).
        let snap = DesiredSnapshot {
            nodes: vec![
                n("peer:dup", "us-east"),
                n("peer:dup", "us-east"),
                n("peer:dup", "us-east"),
            ],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let dups = validate(&snap)
            .into_iter()
            .filter(|e| matches!(e, ValidationError::DuplicateNodeId { .. }))
            .count();
        assert_eq!(dups, 2);
    }
}
