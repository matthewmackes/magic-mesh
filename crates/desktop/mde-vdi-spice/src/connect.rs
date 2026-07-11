//! The live SPICE transport — drive `spice-client`'s async client and bridge it
//! to the egui-facing [`SpiceSession`].
//!
//! This is the **`spice-client`-dependent layer**, the SPICE analogue of
//! mde-vdi-rdp's `connect` module. It owns a [`spice_client::SpiceClient`], runs
//! the connection + channel handshake, then bridges in both directions:
//!
//! * [`SpiceTransport::pump_frame`] pulls the display channel's latest decoded
//!   primary surface and folds it into the session
//!   ([`SpiceSession::apply_surface`]) — the connect→frame direction.
//! * [`SpiceTransport::flush_input`] drains the session's queued
//!   [`SpiceInputEvent`]s and puts them on the SPICE inputs channel
//!   (`send_key_down`/`send_mouse_*`) — the input direction.
//!
//! The async transport's connect path is exercised headlessly against a closed
//! loopback port (`tests/loopback_spice.rs` — the real connect runs and returns a
//! typed error, never hanging), and the full connect→frame→input round-trip is
//! the env-gated live proof (`tests/live_spice.rs`, a real KVM console).
//! [`BlockingSpiceTransport`] wraps the transport in a small current-thread
//! runtime so the sync egui shell can drive the connect loop off one worker
//! thread (the E12-4 wire-transport seam), exactly as mde-vdi-rdp's connect layer
//! does.
//!
//! The intent→wire translation ([`spice_button`], the scancode packing in
//! [`crate::input::to_spice`], the wheel-to-clicks expansion) is pure and
//! unit-tested here; the connect + pump are proven against a real (loopback /
//! live) server since they need one to exercise.

use spice_client::{MouseButton as SpiceMouseButton, SpiceClientShared, SpiceError};

use crate::config::SpiceConfig;
use crate::input::{to_spice, MouseButton, SpiceInputEvent};
use crate::session::SpiceSession;

/// The SPICE channel id the primary display + inputs ride (display 0).
const PRIMARY_CHANNEL: u8 = 0;

/// Why a SPICE transport step failed.
#[derive(Debug)]
pub enum ConnectError {
    /// The [`SpiceConfig`] was invalid (empty host / zero port / bad size).
    Config(crate::config::ConfigError),
    /// The `spice-client` stack surfaced a protocol / IO error.
    Spice(SpiceError),
    /// A decoded surface was malformed (zero-dimension / truncated).
    Surface(crate::pixel::FramebufferError),
    /// The current-thread runtime backing [`BlockingSpiceTransport`] failed to
    /// build (the sync shell facade).
    Runtime(std::io::Error),
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "SPICE config invalid: {e}"),
            Self::Spice(e) => write!(f, "SPICE transport error: {e}"),
            Self::Surface(e) => write!(f, "SPICE surface error: {e}"),
            Self::Runtime(e) => write!(f, "SPICE runtime error: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<SpiceError> for ConnectError {
    fn from(e: SpiceError) -> Self {
        Self::Spice(e)
    }
}

/// Map a protocol-neutral [`MouseButton`] to the `spice-client` button.
#[must_use]
const fn spice_button(b: MouseButton) -> SpiceMouseButton {
    match b {
        MouseButton::Left => SpiceMouseButton::Left,
        MouseButton::Right => SpiceMouseButton::Right,
        MouseButton::Middle => SpiceMouseButton::Middle,
    }
}

/// A connected SPICE transport: the async `spice-client` bridged to a
/// [`SpiceSession`].
pub struct SpiceTransport {
    client: SpiceClientShared,
}

impl SpiceTransport {
    /// Connect to the SPICE console described by `config` and run the channel
    /// handshake. The returned transport is ready to [`pump_frame`] /
    /// [`flush_input`].
    ///
    /// [`pump_frame`]: SpiceTransport::pump_frame
    /// [`flush_input`]: SpiceTransport::flush_input
    ///
    /// # Errors
    /// [`ConnectError::Config`] if the config is invalid, [`ConnectError::Spice`]
    /// if the connection / handshake fails.
    pub async fn connect(config: &SpiceConfig) -> Result<Self, ConnectError> {
        config.validate().map_err(ConnectError::Config)?;
        let mut client = SpiceClientShared::new(config.host.clone(), config.port);
        if let Some(ref password) = config.password {
            client.set_password(password.clone()).await;
        }
        client.connect().await?;
        Ok(Self { client })
    }

    /// Borrow the underlying `spice-client` (the live event loop / channel
    /// readiness API the shell's transport thread drives, e.g.
    /// [`SpiceClientShared::start_event_loop`]).
    #[must_use]
    pub const fn client(&self) -> &SpiceClientShared {
        &self.client
    }

    /// Pull the display channel's latest decoded primary surface and fold it into
    /// `session`. Returns `true` only if the desktop actually **changed** (a new
    /// frame is now available via [`SpiceSession::frame`]), `false` if the channel
    /// has no surface yet **or** the polled surface is byte-identical to the one
    /// already shown — the dirty-check ([`SpiceSession::apply_surface`]) so a
    /// static desktop costs no normalise/copy and no texture upload.
    ///
    /// # Errors
    /// [`ConnectError::Surface`] if the decoded surface is malformed.
    pub async fn pump_frame(&self, session: &mut SpiceSession) -> Result<bool, ConnectError> {
        let Some(surface) = self.client.get_display_surface(PRIMARY_CHANNEL).await else {
            return Ok(false);
        };
        session
            .apply_surface(&surface)
            .map_err(ConnectError::Surface)
    }

    /// Drain the session's queued input intents onto the SPICE inputs channel.
    ///
    /// # Errors
    /// [`ConnectError::Spice`] if a wire send fails.
    pub async fn flush_input(&self, session: &mut SpiceSession) -> Result<(), ConnectError> {
        for intent in session.take_input() {
            self.send_intent(intent).await?;
        }
        Ok(())
    }

    /// Put one input intent on the SPICE inputs channel.
    async fn send_intent(&self, intent: SpiceInputEvent) -> Result<(), ConnectError> {
        match intent {
            SpiceInputEvent::PointerMove { x, y } => {
                self.client
                    .send_mouse_motion(PRIMARY_CHANNEL, i32::from(x), i32::from(y))
                    .await?;
            }
            SpiceInputEvent::PointerButton { button, down, x, y } => {
                // Keep the guest pointer synced with the click position first.
                self.client
                    .send_mouse_motion(PRIMARY_CHANNEL, i32::from(x), i32::from(y))
                    .await?;
                self.client
                    .send_mouse_button(PRIMARY_CHANNEL, spice_button(button), down)
                    .await?;
            }
            SpiceInputEvent::Wheel { delta, horizontal } => {
                // SPICE expresses the wheel as discrete button clicks; `spice-client`
                // models only the vertical wheel, so each vertical notch is one
                // click and the (unmodelled) horizontal wheel is dropped honestly.
                if !horizontal {
                    let step = i32::from(delta.signum());
                    for _ in 0..delta.unsigned_abs() {
                        self.client
                            .send_mouse_wheel(PRIMARY_CHANNEL, 0, step)
                            .await?;
                    }
                }
            }
            SpiceInputEvent::Key { scancode, down } => {
                let code = to_spice(scancode);
                if down {
                    self.client.send_key_down(PRIMARY_CHANNEL, code).await?;
                } else {
                    self.client.send_key_up(PRIMARY_CHANNEL, code).await?;
                }
            }
        }
        Ok(())
    }
}

/// Adaptive poll cadence for the blocking pump.
///
/// The original transport slept a fixed 50 ms before *every* poll — a hard 20 fps
/// ceiling and a ≥50 ms input-to-frame latency floor. This paces the poll
/// instead: snap to the short [`PumpPace::ACTIVE`] interval whenever the desktop
/// is repainting (or input was just sent), and geometrically back off toward
/// [`PumpPace::IDLE_CAP`] while it is static — so a live desktop is responsive
/// without a busy-spin over a quiet one. The idle cap is exactly the old 50 ms
/// sleep, so a resumed poll is never *slower* than the pre-change transport; the
/// pacing only ever lowers latency.
///
/// Pure and unit-tested: [`BlockingSpiceTransport::pump_frame`] just threads
/// [`PumpPace::interval`] into `tokio::time::sleep` and feeds the poll's
/// changed/unchanged outcome back through [`PumpPace::observe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PumpPace {
    interval: std::time::Duration,
}

impl PumpPace {
    /// The fast cadence while the desktop is actively changing (~125 Hz) — the
    /// latency floor a live, interactive desktop settles to.
    const ACTIVE: std::time::Duration = std::time::Duration::from_millis(8);
    /// The slow cadence a static desktop backs off to (~20 Hz). Equal to the old
    /// fixed sleep, so an idle poll is never slower than the pre-change transport.
    const IDLE_CAP: std::time::Duration = std::time::Duration::from_millis(50);

    /// Start fast, so the first frame and any freshly-sent input are picked up
    /// promptly.
    const fn new() -> Self {
        Self {
            interval: Self::ACTIVE,
        }
    }

    /// The interval to sleep before the next poll.
    const fn interval(self) -> std::time::Duration {
        self.interval
    }

    /// Fold in whether the last poll produced a *changed* frame.
    fn observe(&mut self, changed: bool) {
        self.interval = Self::next_interval(self.interval, changed);
    }

    /// Freshly-sent input means an imminent repaint — poll fast for it so input
    /// latency does not pay the idle back-off.
    const fn quicken(&mut self) {
        self.interval = Self::ACTIVE;
    }

    /// The pure pacing decision (unit-tested): a change snaps to [`Self::ACTIVE`];
    /// otherwise the interval doubles but is clamped to [`Self::IDLE_CAP`].
    #[must_use]
    fn next_interval(prev: std::time::Duration, changed: bool) -> std::time::Duration {
        if changed {
            return Self::ACTIVE;
        }
        let grown = prev.saturating_mul(2);
        if grown > Self::IDLE_CAP {
            Self::IDLE_CAP
        } else {
            grown
        }
    }
}

/// A blocking facade over [`SpiceTransport`].
///
/// It owns a small current-thread tokio runtime so the sync egui shell (the
/// E12-4 wire-transport thread) drives the connect loop without being async
/// itself — the SPICE analogue of mde-vdi-rdp's blocking connect facade.
pub struct BlockingSpiceTransport {
    runtime: tokio::runtime::Runtime,
    transport: SpiceTransport,
    event_loop: tokio::task::JoinHandle<()>,
    /// Adaptive poll cadence — replaces the old fixed 50 ms sleep.
    pace: PumpPace,
}

impl BlockingSpiceTransport {
    /// Build a current-thread runtime and connect (blocking).
    ///
    /// # Errors
    /// [`ConnectError::Runtime`] if the runtime cannot be built, or the connect
    /// errors of [`SpiceTransport::connect`].
    pub fn connect(config: &SpiceConfig) -> Result<Self, ConnectError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(ConnectError::Runtime)?;
        let transport = runtime.block_on(SpiceTransport::connect(config))?;
        let loop_client = transport.client().clone();
        let event_loop = runtime.spawn(async move {
            let _ = loop_client.start_event_loop().await;
        });
        Ok(Self {
            runtime,
            transport,
            event_loop,
            pace: PumpPace::new(),
        })
    }

    /// Pump one frame (blocking). See [`SpiceTransport::pump_frame`].
    ///
    /// Sleeps the current adaptive [`PumpPace`] interval before polling (short
    /// while the desktop is live, backing off to the old 50 ms cap while it is
    /// static) instead of the old blind fixed 50 ms — lifting the hard 20 fps
    /// ceiling and the input-to-frame latency floor without busy-spinning. The
    /// poll's changed/unchanged outcome re-paces the next call.
    ///
    /// # Errors
    /// Propagates [`SpiceTransport::pump_frame`].
    pub fn pump_frame(&mut self, session: &mut SpiceSession) -> Result<bool, ConnectError> {
        let interval = self.pace.interval();
        let changed = self.runtime.block_on(async {
            tokio::time::sleep(interval).await;
            self.transport.pump_frame(session).await
        })?;
        self.pace.observe(changed);
        Ok(changed)
    }

    /// Flush queued input (blocking). See [`SpiceTransport::flush_input`].
    ///
    /// Freshly-sent input implies an imminent repaint, so this also quickens the
    /// pump pace — the next poll runs at the fast cadence rather than paying the
    /// idle back-off, keeping input-to-frame latency low even after a quiet spell.
    ///
    /// # Errors
    /// Propagates [`SpiceTransport::flush_input`].
    pub fn flush_input(&mut self, session: &mut SpiceSession) -> Result<(), ConnectError> {
        self.pace.quicken();
        self.runtime.block_on(self.transport.flush_input(session))
    }
}

impl Drop for BlockingSpiceTransport {
    fn drop(&mut self) {
        self.event_loop.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::{spice_button, MouseButton, PumpPace};
    use spice_client::MouseButton as SpiceMouseButton;
    use std::time::Duration;

    #[test]
    fn buttons_map_to_spice() {
        assert_eq!(spice_button(MouseButton::Left), SpiceMouseButton::Left);
        assert_eq!(spice_button(MouseButton::Right), SpiceMouseButton::Right);
        assert_eq!(spice_button(MouseButton::Middle), SpiceMouseButton::Middle);
    }

    #[test]
    fn pace_starts_fast_and_is_faster_than_the_old_fixed_sleep() {
        // The whole point: the live cadence lifts the old hard 20 fps / 50 ms
        // floor, and the pump begins fast so the first paint is picked up promptly.
        assert_eq!(PumpPace::new().interval(), PumpPace::ACTIVE);
        assert!(PumpPace::ACTIVE < Duration::from_millis(50));
    }

    #[test]
    fn pace_decision_is_a_pure_function() {
        // A change always snaps to the fast cadence, from any prior interval.
        assert_eq!(
            PumpPace::next_interval(PumpPace::IDLE_CAP, true),
            PumpPace::ACTIVE
        );
        assert_eq!(
            PumpPace::next_interval(PumpPace::ACTIVE, true),
            PumpPace::ACTIVE
        );
        // An idle poll grows the interval geometrically...
        let grown = PumpPace::next_interval(PumpPace::ACTIVE, false);
        assert!(grown > PumpPace::ACTIVE && grown <= PumpPace::IDLE_CAP);
        // ...but is clamped to the idle cap and never past it.
        assert_eq!(
            PumpPace::next_interval(PumpPace::IDLE_CAP, false),
            PumpPace::IDLE_CAP
        );
        // The idle cadence never regresses past the old fixed 50 ms floor.
        assert!(PumpPace::next_interval(PumpPace::IDLE_CAP, false) <= Duration::from_millis(50));
    }

    #[test]
    fn pace_observe_backs_off_when_idle_and_snaps_back_on_change() {
        let mut p = PumpPace::new();
        assert_eq!(p.interval(), PumpPace::ACTIVE);
        // Many idle polls settle at the cap (never a busy-spin, never past 50 ms).
        for _ in 0..12 {
            p.observe(false);
        }
        assert_eq!(p.interval(), PumpPace::IDLE_CAP);
        // A changed frame snaps the cadence back to fast immediately.
        p.observe(true);
        assert_eq!(p.interval(), PumpPace::ACTIVE);
    }

    #[test]
    fn pace_quicken_protects_input_latency_after_idle() {
        // After backing off to idle, sending input must restore the fast cadence
        // so the input's repaint is not gated by the idle back-off.
        let mut p = PumpPace::new();
        for _ in 0..12 {
            p.observe(false);
        }
        assert_eq!(p.interval(), PumpPace::IDLE_CAP);
        p.quicken();
        assert_eq!(p.interval(), PumpPace::ACTIVE);
    }
}
