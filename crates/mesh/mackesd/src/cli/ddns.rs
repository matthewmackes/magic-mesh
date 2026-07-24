//! `Ddns` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `ddns` subcommand.
#[allow(unreachable_code)]
pub fn run(action: DdnsCmd) -> anyhow::Result<()> {
    {
        // DDNS-EGRESS-3 — CLI parity for the action/ddns/* RPCs: build a
        // DdnsService rooted at the shared workgroup root and call the SAME
        // `ipc::ddns::build_reply` verb the bus responder serves, printing the
        // JSON reply. One config, two front-ends (CLI + GUI).
        use mackesd_core::ipc::ddns::{build_reply, DdnsService};
        let (verb, body, root): (&str, Option<String>, PathBuf) = match action {
            DdnsCmd::GetConfig { workgroup_root } => (
                "get-config",
                None,
                workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root),
            ),
            DdnsCmd::SetConfig {
                config,
                workgroup_root,
            } => (
                "set-config",
                Some(config),
                workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root),
            ),
            DdnsCmd::AddRecord {
                name,
                source,
                on_down,
                workgroup_root,
            } => {
                let body = serde_json::json!({
                    "name": name, "source": source, "on_down": on_down,
                })
                .to_string();
                (
                    "add-record",
                    Some(body),
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root),
                )
            }
            DdnsCmd::RemoveRecord {
                name,
                workgroup_root,
            } => (
                "remove-record",
                Some(name),
                workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root),
            ),
            DdnsCmd::Status {
                name,
                state,
                ip,
                port_forward,
                kill_switch,
                last,
                workgroup_root,
            } => {
                // Build the {name,state[,last]} record-status query body. An
                // `up` state carries the verified IP + port-forward flag; a
                // `down` state carries the kill-switch flag.
                let state_obj = if state.eq_ignore_ascii_case("down") {
                    serde_json::json!({ "state": "down", "kill_switch": kill_switch })
                } else {
                    serde_json::json!({ "state": "up", "ip": ip, "port_forward": port_forward })
                };
                let mut q = serde_json::json!({ "name": name, "state": state_obj });
                if !last.is_empty() {
                    q["last"] = serde_json::Value::String(last);
                }
                (
                    "record-status",
                    Some(q.to_string()),
                    workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root),
                )
            }
        };
        let svc = DdnsService::new(root);
        let reply = build_reply(&svc, verb, body.as_deref());
        println!("{reply}");
        // Exit non-zero on an error reply so scripts can branch on it. Inspect
        // the reply's top-level JSON field rather than searching the serialized
        // payload: a valid record/config value may itself contain the string
        // `"error"`.
        if reply_is_error(&reply) {
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Return whether a DDNS reply is an error envelope.
///
/// `build_reply` always emits a JSON object, but treat an unexpected or
/// malformed reply as an error so the CLI remains fail-closed for scripts.
fn reply_is_error(reply: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(reply)
        .map(|value| value.get("error").is_some())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::reply_is_error;

    #[test]
    fn success_payload_with_error_value_is_not_an_error_reply() {
        let reply = r#"{
            "ok": true,
            "config": {"record": [{"name": "error", "source": "wan"}]}
        }"#;
        assert!(!reply_is_error(reply));
    }

    #[test]
    fn top_level_error_and_malformed_replies_fail_closed() {
        assert!(reply_is_error(r#"{"error":"bad config"}"#));
        assert!(reply_is_error("not-json"));
    }
}
