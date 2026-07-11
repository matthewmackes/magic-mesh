//! `VoipRtt` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `voip-rtt` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::voip_rtt::{
            own_nebula_ip, publish_link_rtt, rtt_topic, sample_link_rtt, VITELITY_PROXY_HOST,
            VITELITY_PROXY_PORT,
        };
        let peer = own_nebula_ip().unwrap_or_default();
        let sample = sample_link_rtt(&peer);
        match sample.rtt_ms {
            Some(ms) => {
                println!("voip-link-rtt: {ms} ms ({VITELITY_PROXY_HOST}:{VITELITY_PROXY_PORT})");
            }
            None => {
                println!(
                    "voip-link-rtt: unreachable ({VITELITY_PROXY_HOST}:{VITELITY_PROXY_PORT})"
                );
            }
        }
        if peer.is_empty() {
            eprintln!("voip-rtt: no nebula1 overlay IP — measured but not published");
        } else {
            publish_link_rtt(&sample);
            eprintln!("voip-rtt: published to {}", rtt_topic(&peer));
        }
    }
    Ok(())
}
