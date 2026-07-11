//! The off-thread seat-snapshot pump (perf-2).
//!
//! [`crate::system::SystemState`] renders from a [`mde_seat::SeatSnapshot`], which
//! folds ~11 host probes — several of which SHELL OUT and block: the `PipeWire`
//! `pw-dump` mixer read, and (worst) the `ddcutil` DDC/CI probe, whose per-monitor
//! I2C `getvcp` can take hundreds of ms *each*. Running `seat.snapshot()` inline on
//! the egui render thread every 5 s therefore froze the whole UI for the probe's
//! duration on real hardware — a guaranteed periodic stutter.
//!
//! This pump moves that work OFF the render thread using the same producer→drain
//! shape the VDI desktop uses for decoded frames ([`crate::vdi`]): a dedicated
//! background thread owns a read-only [`Seat`], produces a snapshot on the 5 s
//! cadence, and publishes the newest over an `mpsc` channel. The render thread only
//! ever *drains* the latest published snapshot ([`SnapshotPump::drain_latest`]) — it
//! NEVER touches the blocking probe.
//!
//! Two further economies keep the expensive DDC path off even the background beat:
//! * `ddcutil detect` (the monitor inventory) is cached and re-run ONLY when the DRM
//!   connector set changes — the SAME [`connector_key`] signal the System surface's
//!   `layout_key` uses to rebuild its display layout ([`DdcCache`]).
//! * per-monitor brightness (`getvcp`) is re-read on a much slower cadence
//!   ([`DDC_BRIGHTNESS_REFRESH`]) than the 5 s fast reads, plus once immediately
//!   after a fresh detect (so a just-plugged monitor's level shows without waiting).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mde_egui::egui;
use mde_seat::{Backend, Connector, DdcDisplay, Probe, Seat, SeatError, SeatSnapshot};

/// The background snapshot cadence — the fast host probes (`BlueZ` / `UPower` /
/// logind / DRM / backlight / `PipeWire`) refresh this often. Matches the System surface's
/// former inline refresh, so the data is exactly as fresh as before — it just no
/// longer blocks the render thread.
const REFRESH: Duration = Duration::from_secs(5);

/// The DDC/CI brightness (`getvcp`) re-read cadence — deliberately MUCH slower than
/// [`REFRESH`] because the per-monitor I2C read is the slowest probe on the seat. A
/// fresh detect (a re-plug) still reads brightness immediately, so a newly connected
/// monitor's level shows without waiting this out.
const DDC_BRIGHTNESS_REFRESH: Duration = Duration::from_secs(60);

/// The DRM connector-set signal the System surface's `layout_key` is built from — the
/// ordered connector names. Shared so the [`DdcCache`] invalidates on the EXACT same
/// signal the surface rebuilds its display layout on (a re-plug), never diverging.
pub(crate) fn connector_key(connectors: &[Connector]) -> Vec<String> {
    connectors.iter().map(|c| c.name.clone()).collect()
}

/// The two DDC operations the [`DdcCache`] needs, behind a seam so the cache logic is
/// unit-tested against a counting fake (no real `ddcutil`, no thread).
trait DdcSource {
    fn detect(&self) -> Result<Vec<DdcDisplay>, SeatError>;
    fn fill_brightness(&self, displays: &mut [DdcDisplay]);
}

impl DdcSource for Seat {
    fn detect(&self) -> Result<Vec<DdcDisplay>, SeatError> {
        self.ddc_detect()
    }
    fn fill_brightness(&self, displays: &mut [DdcDisplay]) {
        self.ddc_read_brightness(displays);
    }
}

/// The placeholder DDC probe the pump hands [`Seat::snapshot_with_ddc`] before it
/// overwrites the field with the cache's answer — never observed by the UI (it is
/// the fallback the cache also degrades to if it somehow holds nothing).
fn ddc_pending() -> Probe<Vec<DdcDisplay>> {
    Probe::Absent {
        backend: Backend::Ddc,
        reason: "DDC probe pending".to_owned(),
    }
}

/// Caches the `ddcutil detect` inventory across ticks, re-detecting ONLY when the
/// connector set changes, and re-reading brightness on a slow cadence.
#[derive(Default)]
struct DdcCache {
    /// The connector key the cached probe was detected at (`None` before the first
    /// detect, or on a host with no displays).
    key: Option<Vec<String>>,
    /// The last resolved DDC probe — `Present` monitors carry live brightness,
    /// `Absent` is the honest "no ddcutil / DDC refused" state — reused until the
    /// connector set changes.
    cached: Option<Probe<Vec<DdcDisplay>>>,
}

impl DdcCache {
    /// Resolve the DDC probe for this tick from `src`, given the current connector
    /// `key` (`None` when the display probe is Absent) and whether the slow
    /// brightness cadence is due. Returns the probe plus whether brightness was
    /// (re-)read this call, so the caller can reset its cadence timer.
    fn resolve(
        &mut self,
        src: &dyn DdcSource,
        key: Option<Vec<String>>,
        brightness_due: bool,
    ) -> (Probe<Vec<DdcDisplay>>, bool) {
        // Re-detect on the first run, or when a *present* connector set differs from
        // the one the cache was built at (a re-plug). A transiently-Absent display
        // probe (key None) never invalidates the cache — it keeps the last-known
        // monitors rather than dropping them on a blip.
        let key_changed = key
            .as_deref()
            .is_some_and(|k| self.key.as_deref() != Some(k));
        let need_detect = self.cached.is_none() || key_changed;
        let mut read_brightness = false;

        if need_detect {
            let mut probe = Probe::from_result(src.detect());
            if let Probe::Present(list) = &mut probe {
                // A fresh detect always reads brightness once, so a just-plugged
                // monitor's level shows immediately (not after the slow cadence).
                src.fill_brightness(list);
                read_brightness = true;
            }
            self.cached = Some(probe);
            if let Some(k) = key {
                self.key = Some(k);
            }
        } else if brightness_due {
            if let Some(Probe::Present(list)) = self.cached.as_mut() {
                src.fill_brightness(list);
                read_brightness = true;
            }
        }

        let probe = self.cached.clone().unwrap_or_else(ddc_pending);
        (probe, read_brightness)
    }
}

/// The background snapshot producer's handle: the drain channel plus the lifecycle
/// (stop signal + join) so it spawns once and shuts down cleanly with the shell.
pub(crate) struct SnapshotPump {
    /// The newest-wins snapshot channel — the render thread drains it.
    rx: Receiver<SeatSnapshot>,
    /// Signals the background thread to stop (on drop). `None` for the test seam.
    stop_tx: Option<Sender<()>>,
    /// The background thread, joined on drop. `None` for the test seam / a failed
    /// spawn.
    handle: Option<JoinHandle<()>>,
}

impl SnapshotPump {
    /// Spawn the background producer over a dedicated read-only [`Seat`]. `ctx` is
    /// cloned so the thread can wake the render thread to drain each fresh snapshot,
    /// keeping the UI as fresh as the old inline poll.
    pub(crate) fn spawn(ctx: egui::Context) -> Self {
        let (snap_tx, rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        // A failed spawn degrades honestly: no thread, so `drain_latest` always
        // yields `None` and the surface renders its pre-poll state — never a panic.
        let handle = thread::Builder::new()
            .name("mde-seat-snapshot".to_owned())
            .spawn(move || run(&ctx, &snap_tx, &stop_rx))
            .ok();
        Self {
            rx,
            stop_tx: Some(stop_tx),
            handle,
        }
    }

    /// Drain to the NEWEST published snapshot without blocking (latest-wins, like the
    /// VDI frame drain). Returns `None` when nothing new has arrived, so the render
    /// thread keeps the snapshot it already holds. This is the ONLY seat-snapshot
    /// path on the render thread — it never runs the blocking probe.
    pub(crate) fn drain_latest(&self) -> Option<SeatSnapshot> {
        let mut latest = None;
        while let Ok(snap) = self.rx.try_recv() {
            latest = Some(snap);
        }
        latest
    }

    /// A pump backed by a plain receiver with no thread — the test seam, so the drain
    /// path is exercised deterministically without a real `Seat` or a spawned worker.
    #[cfg(test)]
    pub(crate) const fn from_receiver(rx: Receiver<SeatSnapshot>) -> Self {
        Self {
            rx,
            stop_tx: None,
            handle: None,
        }
    }
}

impl Drop for SnapshotPump {
    fn drop(&mut self) {
        // Signal stop, then join — a clean shutdown. The thread is either sleeping in
        // `recv_timeout` (returns at once on the stop) or finishing one bounded probe,
        // so the join returns promptly.
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The background loop: produce a snapshot every [`REFRESH`], publish the newest,
/// wake the UI, and stop promptly on the shutdown signal (or a dropped receiver).
fn run(ctx: &egui::Context, snap_tx: &Sender<SeatSnapshot>, stop_rx: &Receiver<()>) {
    let seat = Seat::new();
    let mut ddc = DdcCache::default();
    let mut last_brightness: Option<Instant> = None;

    loop {
        // Fold the fast probes with a placeholder DDC (so the expensive `ddcutil`
        // path is NOT run inline here), read the connector set the fold produced,
        // then attach the cache's DDC answer keyed on that exact set.
        let mut snap = seat.snapshot_with_ddc(ddc_pending());
        let key = snap.displays.present().map(|c| connector_key(c.as_slice()));
        let brightness_due = last_brightness.is_none_or(|t| t.elapsed() >= DDC_BRIGHTNESS_REFRESH);
        let (probe, read_brightness) = ddc.resolve(&seat, key, brightness_due);
        if read_brightness {
            last_brightness = Some(Instant::now());
        }
        snap.ddc = probe;

        // Publish newest-wins; a dropped receiver means the shell is gone — stop.
        if snap_tx.send(snap).is_err() {
            return;
        }
        // Wake the render thread so it drains this snapshot on the next frame.
        ctx.request_repaint();

        // Sleep until the next tick, but return AT ONCE on a stop signal (or a
        // dropped sender) — no 5 s shutdown latency, no busy-wait.
        match stop_rx.recv_timeout(REFRESH) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use super::*;

    /// A counting DDC source: canned detect output, tallying detect + fill calls so
    /// the cache's invalidation is asserted precisely.
    #[derive(Default)]
    struct FakeDdc {
        detects: AtomicUsize,
        fills: AtomicUsize,
        monitors: Vec<DdcDisplay>,
    }

    impl DdcSource for FakeDdc {
        fn detect(&self) -> Result<Vec<DdcDisplay>, SeatError> {
            self.detects.fetch_add(1, Ordering::SeqCst);
            Ok(self.monitors.clone())
        }
        fn fill_brightness(&self, _displays: &mut [DdcDisplay]) {
            self.fills.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn monitor(bus: &str) -> DdcDisplay {
        DdcDisplay {
            bus: bus.to_owned(),
            connector: None,
            model: None,
            brightness: 0,
        }
    }

    #[test]
    fn detect_is_cached_across_ticks_with_an_unchanged_connector_key() {
        let src = FakeDdc {
            monitors: vec![monitor("i2c-4")],
            ..Default::default()
        };
        let mut cache = DdcCache::default();
        let key = Some(vec!["DP-1".to_owned()]);

        // First resolve on a key → one detect (+ its immediate brightness read).
        let (p1, b1) = cache.resolve(&src, key.clone(), false);
        assert!(matches!(p1, Probe::Present(_)));
        assert!(b1, "a fresh detect reads brightness once");
        assert_eq!(src.detects.load(Ordering::SeqCst), 1);

        // Same key, brightness not due → NO second detect.
        let (_p2, b2) = cache.resolve(&src, key, false);
        assert!(!b2);
        assert_eq!(
            src.detects.load(Ordering::SeqCst),
            1,
            "an unchanged connector key must not re-run ddcutil detect"
        );
    }

    #[test]
    fn a_changed_connector_key_re_detects() {
        let src = FakeDdc {
            monitors: vec![monitor("i2c-4")],
            ..Default::default()
        };
        let mut cache = DdcCache::default();

        cache.resolve(&src, Some(vec!["DP-1".to_owned()]), false);
        assert_eq!(src.detects.load(Ordering::SeqCst), 1);
        // A re-plug: the connector set changed → re-detect.
        cache.resolve(
            &src,
            Some(vec!["DP-1".to_owned(), "HDMI-A-1".to_owned()]),
            false,
        );
        assert_eq!(
            src.detects.load(Ordering::SeqCst),
            2,
            "a changed layout_key must re-run detect"
        );
    }

    #[test]
    fn brightness_is_reread_only_on_the_slow_cadence_not_every_tick() {
        let src = FakeDdc {
            monitors: vec![monitor("i2c-4")],
            ..Default::default()
        };
        let mut cache = DdcCache::default();
        let key = Some(vec!["DP-1".to_owned()]);

        // Fresh detect reads brightness once.
        cache.resolve(&src, key.clone(), false);
        assert_eq!(src.fills.load(Ordering::SeqCst), 1);
        // Not due → no re-read, and no re-detect.
        cache.resolve(&src, key.clone(), false);
        assert_eq!(src.fills.load(Ordering::SeqCst), 1);
        // Due (the slow cadence elapsed) → one more read, still no re-detect.
        let (_p, b) = cache.resolve(&src, key, true);
        assert!(b);
        assert_eq!(src.fills.load(Ordering::SeqCst), 2);
        assert_eq!(src.detects.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_transiently_absent_display_probe_keeps_the_cached_monitors() {
        let src = FakeDdc {
            monitors: vec![monitor("i2c-4")],
            ..Default::default()
        };
        let mut cache = DdcCache::default();
        cache.resolve(&src, Some(vec!["DP-1".to_owned()]), false);
        assert_eq!(src.detects.load(Ordering::SeqCst), 1);
        // The DRM probe blipped Absent (key None) — must NOT invalidate / re-detect.
        let (p, _b) = cache.resolve(&src, None, false);
        assert!(
            matches!(p, Probe::Present(_)),
            "an Absent display blip keeps the cached monitors"
        );
        assert_eq!(src.detects.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_detect_failure_is_cached_absent_and_not_hammered_each_tick() {
        // A host with no ddcutil: detect Errs → Absent, and the cache holds it (no
        // re-detect storm) until the connector set changes.
        struct FailingDdc {
            detects: AtomicUsize,
        }
        impl DdcSource for FailingDdc {
            fn detect(&self) -> Result<Vec<DdcDisplay>, SeatError> {
                self.detects.fetch_add(1, Ordering::SeqCst);
                Err(SeatError::Unavailable {
                    backend: Backend::Ddc,
                    reason: "no ddcutil".into(),
                })
            }
            fn fill_brightness(&self, _d: &mut [DdcDisplay]) {}
        }
        let src = FailingDdc {
            detects: AtomicUsize::new(0),
        };
        let mut cache = DdcCache::default();
        let key = Some(vec!["DP-1".to_owned()]);

        let (p1, _) = cache.resolve(&src, key.clone(), true);
        assert!(matches!(
            p1,
            Probe::Absent {
                backend: Backend::Ddc,
                ..
            }
        ));
        cache.resolve(&src, key, true);
        assert_eq!(
            src.detects.load(Ordering::SeqCst),
            1,
            "a cached Absent must not re-detect every tick"
        );
    }

    #[test]
    fn drain_latest_returns_the_newest_and_none_when_empty() {
        let (tx, rx) = mpsc::channel();
        let pump = SnapshotPump::from_receiver(rx);
        assert!(
            pump.drain_latest().is_none(),
            "empty channel drains to None"
        );

        // Publish three; the drain keeps only the newest (latest-wins).
        for n in 1u8..=3 {
            let mut snap = Seat::new().snapshot();
            snap.charge_limit = Probe::Present(Some(n));
            tx.send(snap).expect("publish");
        }
        let latest = pump.drain_latest().expect("a snapshot");
        assert!(matches!(latest.charge_limit, Probe::Present(Some(3))));
        assert!(
            pump.drain_latest().is_none(),
            "drained dry after taking the latest"
        );
    }
}
