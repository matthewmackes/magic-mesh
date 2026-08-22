//! Retired `voip-rtt` CLI verb (WL-FUNC-033).
//!
//! Q9 signed 2026-08-22. The worker no longer publishes
//! `voip/link-rtt/<peer>`. This verb fails closed: it does not sample
//! or publish Vitelity-link RTT. The place-via-peer override is retired.

/// Handle the `voip-rtt` subcommand. Always fails closed.
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!(
        "voip-rtt is retired: the place-via-peer override is retired \
         (Q9, 2026-08-22); this verb no longer samples or publishes voip/link-rtt"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_fails_closed_because_place_via_peer_is_retired() {
        let err = run().expect_err("retired voip-rtt must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("place-via-peer") && msg.contains("retired"),
            "error must say the place-via-peer override is retired: {msg}"
        );
        assert!(
            !msg.contains("ms") && !msg.contains("published to"),
            "retired verb must not report a sample or publish: {msg}"
        );
    }
}
