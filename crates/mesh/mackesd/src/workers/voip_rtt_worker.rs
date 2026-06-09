//! VOIP-4.b (v5.0.0) — 60 s Vitelity-link RTT broadcast worker.
//!
//! Ticks every 60 s and broadcasts this peer's Vitelity-link RTT to
//! `voip/link-rtt/<peer>` (via [`crate::voip_rtt::sample_and_publish`]),
//! so the dialer can compare links across peers and offer an operator-
//! explicit "place via `<peer>`" route override (auto-routing stays off).
//! A no-op on a peer with no Nebula overlay IP.

use std::time::Duration;

use super::{ShutdownToken, Worker};

/// The Vitelity-link RTT broadcast worker.
pub struct VoipRttWorker {
    tick: Duration,
}

impl Default for VoipRttWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl VoipRttWorker {
    /// New worker with the default 60 s cadence.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: Duration::from_secs(60),
        }
    }
}

#[async_trait::async_trait]
impl Worker for VoipRttWorker {
    fn name(&self) -> &'static str {
        "voip_rtt"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // first sample lands after `tick`, not immediately
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // The measure is a blocking TCP connect — run it off the
                    // async executor so a slow/timing-out edge can't stall it.
                    let _ = tokio::task::spawn_blocking(crate::voip_rtt::sample_and_publish).await;
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}
