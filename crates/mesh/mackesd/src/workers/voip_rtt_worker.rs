//! Retired VOIP-4.b Vitelity-link RTT broadcast worker (WL-FUNC-033).
//!
//! Q9 signed 2026-08-22. The Kamailio stack is gone and the
//! `voip/link-rtt/<peer>` place-via-peer override was never wired.
//! This worker stays on the supervisor roster so spawn sites are
//! unchanged, but it fails closed: it does not sample Vitelity RTT
//! and does not publish. It idles on the shutdown token and returns.

use super::{ShutdownToken, Worker};

/// Retired Vitelity-link RTT broadcast worker. Spawned, but a no-op.
pub struct VoipRttWorker;

impl Default for VoipRttWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl VoipRttWorker {
    /// Construct the retired worker. `run` never samples or publishes.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Worker for VoipRttWorker {
    fn name(&self) -> &'static str {
        "voip_rtt"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Fail closed / retired: do not sample or publish RTT.
        shutdown.wait().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use mde_bus::persist::Persist;

    struct EnvRestore {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[tokio::test]
    async fn run_publishes_nothing_and_returns_on_shutdown() {
        let bus = tempfile::tempdir().expect("bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let _bus_pin = EnvRestore::set("MDE_BUS_ROOT", bus.path());

        let bin = bus.path().join("bin");
        std::fs::create_dir_all(&bin).expect("stub bin");
        let stub = bin.join("mde-bus");
        let marker = bus.path().join("mde-bus-invoked");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                marker.display()
            ),
        )
        .expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&stub).expect("stub meta").permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&stub, perm).expect("chmod stub");
        }
        let path = match std::env::var_os("PATH") {
            Some(existing) => {
                let mut next = bin.into_os_string();
                next.push(":");
                next.push(existing);
                next
            }
            None => bin.into_os_string(),
        };
        let _path_pin = EnvRestore::set("PATH", path);

        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut worker = VoipRttWorker::new();
        assert_eq!(worker.name(), "voip_rtt");
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { worker.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());

        assert!(!marker.exists(), "retired worker must not invoke mde-bus");
        let topics = persist
            .list_topics_with_prefix("voip/link-rtt/")
            .expect("list rtt topics");
        assert!(
            topics.is_empty(),
            "retired worker must not publish voip/link-rtt: {topics:?}"
        );
    }
}
