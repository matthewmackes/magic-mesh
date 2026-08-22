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

    /// Production text of this leftover module (everything above the test cfg).
    fn production_source() -> &'static str {
        include_str!("voip_rtt.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("leftover module must keep a production run() above tests")
    }

    #[test]
    fn run_fails_closed_because_place_via_peer_is_retired() {
        match run() {
            Ok(()) => panic!(
                "WL-FUNC-033 leftover: voip-rtt run() must refuse; \
                 a live sample/publish path cannot return success"
            ),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("place-via-peer") && msg.contains("retired"),
                    "error must say the place-via-peer override is retired: {msg}"
                );
                assert!(
                    !msg.contains("voip-link-rtt:")
                        && !msg.contains("published to")
                        && !msg.contains("measured but not published"),
                    "retired verb must not report a sample or publish: {msg}"
                );
            }
        }

        let production = production_source();
        assert!(
            production.contains("place-via-peer") && production.contains("retired"),
            "leftover run() must keep the place-via-peer retirement in the production path"
        );
        for needle in [
            "sample_link_rtt",
            "publish_link_rtt",
            "sample_and_publish",
            "Ok(())",
        ] {
            assert!(
                !production.contains(needle),
                "leftover cli/voip_rtt.rs must not keep a live sample/publish success path (`{needle}`)"
            );
        }
    }
}
