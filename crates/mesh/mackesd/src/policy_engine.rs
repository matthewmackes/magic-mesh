//! PLANES-13 (W46–W51) — the declarative policy engine.
//! (Distinct from `policy` — that's the routing DSL; this asserts
//! over the directory record + emits drift.)
//!
//! A policy is a named **assertion over data the mesh already
//! replicates** (the directory record, descriptors, revision
//! currency) — no new runtime, no Rego (W46/D-W1): policies are
//! TOML rules (W47) of the shape `selector + field + op + expected`,
//! and a failed check **emits a drift event** so the W41 remediation
//! pipeline covers policy too (W49 — one pipeline). A core pack ships
//! enabled (W50); each policy is report-only by default and becomes
//! enforcing only by an opt-in auto-fix binding (W51 — owned by the
//! remediation layer, not here).
//!
//! This module is the rule model + the pure evaluator. The on-change
//! + hourly-leader sweep (W48) and the drift emission wire on top.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A comparison operator (W47 TOML rule grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// The field equals the expected value.
    Eq,
    /// The field does not equal the expected value.
    Ne,
    /// The (numeric) field is ≤ the expected value.
    Le,
    /// The (numeric) field is ≥ the expected value.
    Ge,
}

/// Severity a violation carries into the drift event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Crit,
}

impl Severity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Crit => "crit",
        }
    }
}

/// One policy: assert that `field` of every selected peer satisfies
/// `op expected`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// Stable id (the drift event's policy.<name>).
    pub name: String,
    /// Human description.
    #[serde(default)]
    pub description: String,
    /// Dotted field into the directory record (e.g. `revision.currency`,
    /// `health`, `mde_version`).
    pub field: String,
    pub op: Op,
    /// Expected value (string-compared; numeric ops parse both sides).
    pub expected: String,
    pub severity: Severity,
}

impl Policy {
    /// Evaluate the policy against one peer's directory record JSON.
    /// `true` = compliant. A missing field is a violation (the policy
    /// asserts presence + value).
    #[must_use]
    pub fn holds(&self, record: &serde_json::Value) -> bool {
        let Some(actual) = dotted(record, &self.field) else {
            return false;
        };
        match self.op {
            Op::Eq => value_str(&actual) == self.expected,
            Op::Ne => value_str(&actual) != self.expected,
            Op::Le | Op::Ge => {
                let (Some(a), Some(e)) = (actual.as_f64(), self.expected.parse::<f64>().ok())
                else {
                    return false;
                };
                if self.op == Op::Le {
                    a <= e
                } else {
                    a >= e
                }
            }
        }
    }
}

/// A violation, ready to become a drift event (W49).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub policy: String,
    pub peer: String,
    pub severity: String,
    pub detail: String,
}

/// Evaluate every policy against every peer record; return the
/// violations (W48 produces the record set, this is the pure core).
/// `peers` is `[(hostname, record_json)]`.
#[must_use]
pub fn evaluate(policies: &[Policy], peers: &[(String, serde_json::Value)]) -> Vec<Violation> {
    let mut out = Vec::new();
    for policy in policies {
        for (host, record) in peers {
            if !policy.holds(record) {
                out.push(Violation {
                    policy: policy.name.clone(),
                    peer: host.clone(),
                    severity: policy.severity.as_str().to_string(),
                    detail: format!(
                        "{} {:?} {} — failed",
                        policy.field, policy.op, policy.expected
                    ),
                });
            }
        }
    }
    out
}

fn dotted<'a>(v: &'a serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur.clone())
}

fn value_str(v: &serde_json::Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

/// The policies directory.
#[must_use]
pub fn policies_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("policies")
}

/// Read every policy TOML (junk-tolerant), plus the built-in core
/// pack (W50 — ships enabled): all-nodes-current, role-pinned (a
/// healthy fleet asserts these by default).
#[must_use]
pub fn load_policies(workgroup_root: &Path) -> Vec<Policy> {
    let mut out = core_pack();
    if let Ok(entries) = std::fs::read_dir(policies_dir(workgroup_root)) {
        for e in entries.filter_map(Result::ok) {
            if e.path().extension().is_some_and(|x| x == "toml") {
                if let Ok(raw) = std::fs::read_to_string(e.path()) {
                    if let Ok(p) = toml::from_str::<Policy>(&raw) {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// W50 — the platform's own invariants, shipped enabled.
#[must_use]
pub fn core_pack() -> Vec<Policy> {
    vec![
        Policy {
            name: "all-nodes-current".into(),
            description: "Every node is on the newest fleet revision.".into(),
            field: "revision.currency".into(),
            op: Op::Eq,
            expected: "synced".into(),
            severity: Severity::Warn,
        },
        Policy {
            name: "no-critical-alarms".into(),
            description: "No node carries a critical Netdata alarm.".into(),
            field: "health".into(),
            op: Op::Ne,
            expected: "critical".into(),
            severity: Severity::Crit,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ops_compare_correctly_incl_numeric() {
        let rec = json!({"health":"healthy","revision":{"currency":"behind"},"load":3});
        let eq = Policy {
            name: "e".into(),
            description: String::new(),
            field: "health".into(),
            op: Op::Eq,
            expected: "healthy".into(),
            severity: Severity::Info,
        };
        assert!(eq.holds(&rec));
        let ne = Policy {
            name: "n".into(),
            description: String::new(),
            field: "revision.currency".into(),
            op: Op::Ne,
            expected: "synced".into(),
            severity: Severity::Warn,
        };
        assert!(ne.holds(&rec), "behind != synced");
        let le = Policy {
            name: "l".into(),
            description: String::new(),
            field: "load".into(),
            op: Op::Le,
            expected: "5".into(),
            severity: Severity::Info,
        };
        assert!(le.holds(&rec));
        let ge = Policy {
            name: "g".into(),
            description: String::new(),
            field: "load".into(),
            op: Op::Ge,
            expected: "10".into(),
            severity: Severity::Info,
        };
        assert!(!ge.holds(&rec));
    }

    #[test]
    fn missing_field_is_a_violation() {
        let p = Policy {
            name: "m".into(),
            description: String::new(),
            field: "nope".into(),
            op: Op::Eq,
            expected: "x".into(),
            severity: Severity::Info,
        };
        assert!(!p.holds(&json!({})));
    }

    #[test]
    fn evaluate_flags_each_offending_peer() {
        let peers = vec![
            ("pine".into(), json!({"revision":{"currency":"synced"}})),
            ("oak".into(), json!({"revision":{"currency":"behind"}})),
        ];
        let pol = vec![Policy {
            name: "all-nodes-current".into(),
            description: String::new(),
            field: "revision.currency".into(),
            op: Op::Eq,
            expected: "synced".into(),
            severity: Severity::Warn,
        }];
        let v = evaluate(&pol, &peers);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].peer, "oak");
        assert_eq!(v[0].policy, "all-nodes-current");
        assert_eq!(v[0].severity, "warn");
    }

    #[test]
    fn core_pack_ships_the_platform_invariants() {
        let pack = core_pack();
        let names: Vec<&str> = pack.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"all-nodes-current"));
        assert!(names.contains(&"no-critical-alarms"));
    }
}
