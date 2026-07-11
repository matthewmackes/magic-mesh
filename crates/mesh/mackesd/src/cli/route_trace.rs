//! `RouteTrace` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `route-trace` subcommand.
#[allow(unreachable_code)]
pub fn run(to: String, from: String, direction: String) -> anyhow::Result<()> {
    {
        // ROUTE-TRACE-1 — run the assembler locally against the shared
        // substrate state + print the PathGraph (CLI parity with the
        // action/route/trace responder).
        let root = mackesd_core::default_qnm_shared_root();
        let svc = mackesd_core::ipc::route::RouteService::new(root);
        let body =
            serde_json::json!({ "to": to, "from": from, "direction": direction }).to_string();
        let reply = mackesd_core::ipc::route::build_reply(&svc, "trace", Some(&body));
        println!("{reply}");
    }
    Ok(())
}
