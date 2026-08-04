//! Browser control through the production `mde-vdi-rdp` public API.

use crate::config::HostConfig;
use anyhow::{bail, ensure, Context, Result};
use mde_vdi_core::FrameDamage;
use mde_vdi_rdp::egui::{ColorImage, Event, Key, Modifiers, PointerButton, Pos2};
use mde_vdi_rdp::{PumpOutcome, RdpAudioUnsupportedReason, RdpConfig, RdpConnection, RdpSession};
use std::collections::HashSet;
use std::env;
use std::time::{Duration, Instant};

const PUMP_SLICE: Duration = Duration::from_millis(75);
const PRE_NAVIGATION_SETTLE: Duration = Duration::from_millis(150);
const OMNIBOX_FOCUS_SETTLE: Duration = Duration::from_millis(350);
const POST_NAVIGATION_BROWSER_SETTLE: Duration = Duration::from_secs(1);
const BETWEEN_JOB_CONTROLLER_SETTLE: Duration = Duration::from_secs(6);
const POINTER_FOCUS_SETTLE: Duration = Duration::from_millis(50);
const POINTER_BUTTON_HOLD: Duration = Duration::from_millis(75);
const POST_CLICK_BROWSER_SETTLE: Duration = Duration::from_millis(350);
const INITIAL_FRAME_TIMEOUT: Duration = Duration::from_secs(60);
const RECONNECT_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const MIN_BROWSER_COLORS: usize = 24;
const MIN_BROWSER_NON_DOMINANT: usize = 25_000;
pub const MIN_RECONNECT_IDENTITY_PER_MILLE: u16 = 850;
pub const MIN_RECONNECT_DAMAGE_PER_MILLE: u16 = 700;

pub struct RdpDriver {
    session: RdpSession,
    connection: Option<RdpConnection>,
}

impl RdpDriver {
    pub fn connect(config: &HostConfig) -> Result<Self> {
        let password = config.rdp_password()?;
        let rdp = RdpConfig::new(
            config.rdp_host.to_string(),
            config.rdp_username.clone(),
            password.to_string(),
        )
        .with_port(config.rdp_port)
        .with_resolution(config.desktop_width, config.desktop_height);
        let mut session = RdpSession::new(rdp).context("validate RDP control endpoint")?;
        // The constructor's all-black local canvas can never qualify as a
        // Browser observation.
        let _discarded = session.frame();

        // The collector captures the VM's QEMU-owned virtio audio streams. An
        // RDPSND client would move Chromium audio to the RDP client and make the
        // evidence bind the wrong process. `mde-vdi-rdp` currently probes pw-play
        // by PATH; suppress that optional probe for this single-threaded control
        // connect, then require the typed NoHostPlaybackSink result before any
        // browser navigation.
        let connection =
            connect_without_rdpsnd(&mut session).context("connect Browser-owned RDP session")?;
        ensure!(
            connection.audio_capability().unsupported_reason()
                == Some(RdpAudioUnsupportedReason::NoHostPlaybackSink),
            "RDP control unexpectedly advertised RDPSND; QEMU audio ownership is not trustworthy"
        );
        let mut driver = Self {
            session,
            connection: Some(connection),
        };
        let _frame = driver.wait_for_browser_frame(INITIAL_FRAME_TIMEOUT)?;
        Ok(driver)
    }

    pub fn navigate(&mut self, url: &str) -> Result<()> {
        ensure!(
            url.starts_with("http://127.0.0.1:")
                && url.is_ascii()
                && url.len() <= 512
                && !url
                    .bytes()
                    .any(|value| value.is_ascii_control() || value == b' '),
            "probe URL is not a bounded guest-loopback URL"
        );
        self.navigate_location(url)
    }

    fn navigate_location(&mut self, url: &str) -> Result<()> {
        // Clear any transient Chromium surface first. More importantly, this
        // gives xrdp one independently flushed input batch after its focus-in
        // sequence; sending focus, Ctrl-L, the URL, and Enter in one fast-path
        // frame is racy on a newly reattached persistent session.
        self.session
            .send_input(&key_event(Key::Escape, true, Modifiers::default()));
        self.session
            .send_input(&key_event(Key::Escape, false, Modifiers::default()));
        ensure!(
            self.flush_input()? >= 2,
            "RDP pre-navigation focus input did not reach the wire encoder"
        );
        self.pump_for(PRE_NAVIGATION_SETTLE)?;

        // Replace the current tab for every controlled navigation. Ctrl-L is
        // proven to focus the omnibox in this kiosk session; Ctrl-T can be
        // consumed without creating or focusing a tab. The controller accepts
        // repeated transport GETs while a job remains registered, so replacing
        // a completed page does not consume a duplicate claim.
        for event in omnibox_focus_events() {
            self.session.send_input(&event);
        }
        ensure!(
            self.flush_input()? >= 5,
            "RDP omnibox focus shortcut did not reach the wire encoder"
        );
        // Chromium must process Ctrl-L before Unicode URL events arrive. Keep
        // the wait bounded and continue pumping inbound frames so xrdp cannot
        // deadlock behind a pending repaint while the omnibox gains focus.
        self.pump_for(OMNIBOX_FOCUS_SETTLE)?;

        self.session.send_input(&Event::Text(url.to_owned()));
        self.session
            .send_input(&key_event(Key::Enter, true, Modifiers::default()));
        self.session
            .send_input(&key_event(Key::Enter, false, Modifiers::default()));
        ensure!(
            self.flush_input()? >= url.len() + 2,
            "RDP Browser URL commit did not reach the wire encoder"
        );
        // Give Chromium a bounded interval to commit the location while still
        // pumping inbound frames. For a probe URL, the caller separately
        // requires the authenticated page_loaded state before it can proceed.
        self.pump_for(POST_NAVIGATION_BROWSER_SETTLE)?;
        Ok(())
    }

    pub fn click(&mut self, x: u16, y: u16) -> Result<()> {
        let (width, height) = self.session.desktop_size();
        ensure!(
            x < width && y < height,
            "RDP click lies outside negotiated desktop"
        );
        let position = Pos2::new(f32::from(x), f32::from(y));

        // xorgxrdp retains its last absolute pointer position across reconnects.
        // Establish a fresh motion/enter path before the target move so the
        // nested compositor cannot deduplicate an apparently unchanged point.
        let detour = Pos2::new(f32::from(x.saturating_sub(96)), f32::from(y));
        self.session.send_input(&Event::PointerMoved(detour));
        ensure!(
            self.flush_input()? >= 1,
            "RDP pointer detour did not reach the wire encoder"
        );
        self.pump_for(POINTER_FOCUS_SETTLE)?;

        self.session.send_input(&Event::PointerMoved(position));
        ensure!(
            self.flush_input()? >= 1,
            "RDP target pointer move did not reach the wire encoder"
        );
        self.pump_for(POINTER_FOCUS_SETTLE)?;

        self.session.send_input(&Event::PointerButton {
            pos: position,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::default(),
        });
        ensure!(
            self.flush_input()? >= 1,
            "RDP pointer-down did not reach the wire encoder"
        );
        // Keep the two physical transitions in separate FastPath frames. A
        // zero-duration down+up batch can reach the desktop as an
        // already-released button and therefore is not a real trusted click.
        self.pump_for(POINTER_BUTTON_HOLD)?;

        self.session.send_input(&Event::PointerButton {
            pos: position,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::default(),
        });
        ensure!(
            self.flush_input()? >= 1,
            "RDP pointer-up did not reach the wire encoder"
        );
        // Give the trusted browser handler and its loopback fetch a bounded
        // head start before the host begins authenticated status polling. This
        // prevents a host GET from overtaking the event POST on the guest's
        // deliberately small controller.
        self.pump_for(POST_CLICK_BROWSER_SETTLE)?;
        Ok(())
    }

    fn flush_input(&mut self) -> Result<usize> {
        let connection = self
            .connection
            .as_mut()
            .context("RDP control transport is disconnected")?;
        connection
            .flush_input(&mut self.session)
            .context("flush Browser RDP input")
    }

    pub fn pump_once(&mut self) -> Result<Option<ColorImage>> {
        Ok(self.pump_once_with_damage()?.map(|(frame, _damage)| frame))
    }

    fn pump_for(&mut self, minimum: Duration) -> Result<()> {
        let started = Instant::now();
        while started.elapsed() < minimum {
            let _frame = self.pump_once()?;
        }
        Ok(())
    }

    fn pump_once_with_damage(&mut self) -> Result<Option<(ColorImage, FrameDamage)>> {
        let connection = self
            .connection
            .as_mut()
            .context("RDP control transport is disconnected")?;
        let outcome = connection
            .pump_once(&mut self.session, PUMP_SLICE)
            .context("pump Browser RDP session")?;
        match outcome {
            PumpOutcome::Processed { .. } | PumpOutcome::TimedOut => {}
            PumpOutcome::Terminated { reason } => {
                bail!("Browser RDP session terminated: {reason}")
            }
        }
        Ok(self.session.frame_with_damage())
    }

    pub fn wait_for_browser_frame(&mut self, timeout: Duration) -> Result<ColorImage> {
        let started = Instant::now();
        let mut best = None;
        while started.elapsed() < timeout {
            if let Some(frame) = self.pump_once()? {
                if frame_is_browser(&frame) {
                    return Ok(frame);
                }
                best = Some(frame);
            }
        }
        let detail = best.map_or_else(
            || "no inbound frame".to_owned(),
            |frame| {
                format!(
                    "best frame colors={} non_dominant={}",
                    distinct_colors(&frame),
                    non_dominant_pixels(&frame)
                )
            },
        );
        bail!("no qualifying Browser frame before RDP deadline ({detail})")
    }

    pub fn request_full_refresh(&mut self) -> Result<()> {
        self.connection
            .as_mut()
            .context("RDP control transport is disconnected")?
            .request_full_refresh()
            .context("request post-reconnect Browser repaint")
    }

    pub fn settle_between_browser_jobs(&mut self) -> Result<()> {
        // Keep servicing RDP while the bounded guest controller retires any
        // idle speculative sockets. The controller handles those sockets on
        // separate capped workers, so one cannot block the next probe page.
        self.pump_for(BETWEEN_JOB_CONTROLLER_SETTLE)
    }

    /// Complete a real graceful disconnect. The returned timestamp must be
    /// recorded only after the disconnect PDU was encoded/written and the old
    /// transport object was consumed and dropped.
    pub fn disconnect(&mut self) -> Result<()> {
        let connection = self
            .connection
            .take()
            .context("RDP connection is already disconnected")?;
        connection
            .shutdown(&mut self.session)
            .context("gracefully disconnect Browser RDP transport")?;
        let _local_shutdown_frame = self.session.frame();
        Ok(())
    }

    /// Establish a new TLS/CredSSP/session transport and require a fresh inbound
    /// Browser repaint that retains the pre-disconnect visual identity.
    pub fn reconnect_and_observe(&mut self, baseline: &ColorImage) -> Result<u16> {
        ensure!(
            self.connection.is_none(),
            "old RDP transport is still active"
        );
        let connection =
            connect_without_rdpsnd(&mut self.session).context("reconnect Browser RDP transport")?;
        ensure!(
            connection.audio_capability().unsupported_reason()
                == Some(RdpAudioUnsupportedReason::NoHostPlaybackSink),
            "reconnected control path unexpectedly advertised RDPSND"
        );
        self.connection = Some(connection);
        self.request_full_refresh()?;
        let started = Instant::now();
        let mut last_refresh = Instant::now();
        let mut best_identity = 0_u16;
        let mut coverage = DamageCoverage::new(baseline.size);
        let mut observed_frames = 0_u32;
        let mut browser_frames = 0_u32;
        let mut coverage_ready_frames = 0_u32;
        let mut browser_frames_after_coverage = 0_u32;
        let mut refresh_requests = 1_u32;
        let mut last_identity = 0_u16;
        let mut last_colors = 0_usize;
        let mut last_non_dominant = 0_usize;
        while started.elapsed() < INITIAL_FRAME_TIMEOUT {
            if let Some((frame, damage)) = self.pump_once_with_damage()? {
                if frame.size != baseline.size {
                    continue;
                }
                observed_frames = observed_frames.saturating_add(1);
                coverage.record(&damage);
                last_identity = visual_identity_per_mille(baseline, &frame);
                last_colors = distinct_colors(&frame);
                last_non_dominant = non_dominant_pixels(&frame);
                let coverage_ready = coverage.per_mille() >= MIN_RECONNECT_DAMAGE_PER_MILLE;
                if coverage_ready {
                    coverage_ready_frames = coverage_ready_frames.saturating_add(1);
                }
                if last_colors < MIN_BROWSER_COLORS || last_non_dominant < MIN_BROWSER_NON_DOMINANT
                {
                    continue;
                }
                browser_frames = browser_frames.saturating_add(1);
                if coverage_ready {
                    browser_frames_after_coverage = browser_frames_after_coverage.saturating_add(1);
                }
                best_identity = best_identity.max(last_identity);
                if last_identity >= MIN_RECONNECT_IDENTITY_PER_MILLE && coverage_ready {
                    return Ok(last_identity);
                }
            }
            // xrdp may deliver identity and repaint coverage in separate frame
            // updates, then become quiet. Once coverage is sufficient, request
            // another bounded refresh so success still requires a qualifying
            // Browser frame observed after the repaint threshold was met.
            if coverage.per_mille() >= MIN_RECONNECT_DAMAGE_PER_MILLE
                && last_refresh.elapsed() >= RECONNECT_REFRESH_INTERVAL
            {
                self.request_full_refresh()
                    .context("request coverage-qualified Browser repaint")?;
                last_refresh = Instant::now();
                refresh_requests = refresh_requests.saturating_add(1);
            }
        }
        bail!(
            "new RDP transport produced no identity-preserving full Browser repaint \
             (identity={best_identity} damage={} per mille frames={observed_frames} \
             browser_frames={browser_frames} coverage_ready_frames={coverage_ready_frames} \
             browser_frames_after_coverage={browser_frames_after_coverage} \
             refresh_requests={refresh_requests} last_identity={last_identity} \
             last_colors={last_colors} last_non_dominant={last_non_dominant})",
            coverage.per_mille(),
        )
    }

    pub fn shutdown(mut self) -> Result<()> {
        if self.connection.is_some() {
            self.disconnect()?;
        }
        Ok(())
    }
}

struct DamageCoverage {
    width: usize,
    height: usize,
    covered: Vec<bool>,
    count: usize,
}

impl DamageCoverage {
    fn new(size: [usize; 2]) -> Self {
        let length = size[0].saturating_mul(size[1]);
        Self {
            width: size[0],
            height: size[1],
            covered: vec![false; length],
            count: 0,
        }
    }

    fn record(&mut self, damage: &FrameDamage) {
        match damage {
            FrameDamage::Full => {
                self.covered.fill(true);
                self.count = self.covered.len();
            }
            FrameDamage::Rects(rects) => {
                for rect in rects {
                    let Some(rect) = rect.clamped(self.width, self.height) else {
                        continue;
                    };
                    for y in rect.y()..rect.y() + rect.h() {
                        for x in rect.x()..rect.x() + rect.w() {
                            let index = y * self.width + x;
                            if !self.covered[index] {
                                self.covered[index] = true;
                                self.count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    fn per_mille(&self) -> u16 {
        if self.covered.is_empty() {
            return 0;
        }
        u16::try_from(self.count.saturating_mul(1_000) / self.covered.len()).unwrap_or(1_000)
    }
}

fn connect_without_rdpsnd(
    session: &mut RdpSession,
) -> Result<RdpConnection, mde_vdi_rdp::ConnectError> {
    let saved_path = env::var_os("PATH");
    env::set_var("PATH", "/run/mcnf-browser-vm-control/no-rdpsnd-executables");
    env::set_var("MDE_RDP_STRICT_PIN", "1");
    let result = RdpConnection::connect(session);
    match saved_path {
        Some(path) => env::set_var("PATH", path),
        None => env::remove_var("PATH"),
    }
    result
}

fn key_event(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn omnibox_focus_events() -> [Event; 3] {
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    [
        key_event(Key::L, true, ctrl),
        key_event(Key::L, false, ctrl),
        // A button-up with default modifiers releases the synthesized Ctrl key
        // without adding a printable key that would alter the omnibox.
        Event::PointerButton {
            pos: Pos2::new(0.0, 0.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::default(),
        },
    ]
}

fn frame_is_browser(frame: &ColorImage) -> bool {
    distinct_colors(frame) >= MIN_BROWSER_COLORS
        && non_dominant_pixels(frame) >= MIN_BROWSER_NON_DOMINANT
}

fn distinct_colors(frame: &ColorImage) -> usize {
    frame.pixels.iter().copied().collect::<HashSet<_>>().len()
}

fn non_dominant_pixels(frame: &ColorImage) -> usize {
    let mut counts = std::collections::HashMap::new();
    for pixel in &frame.pixels {
        let count = counts.entry(*pixel).or_insert(0_usize);
        *count += 1;
    }
    frame
        .pixels
        .len()
        .saturating_sub(counts.values().copied().max().unwrap_or(0))
}

fn visual_identity_per_mille(before: &ColorImage, after: &ColorImage) -> u16 {
    if before.size != after.size || before.pixels.is_empty() {
        return 0;
    }
    let compatible = before
        .pixels
        .iter()
        .zip(&after.pixels)
        .filter(|(left, right)| {
            let [lr, lg, lb, _] = left.to_array();
            let [rr, rg, rb, _] = right.to_array();
            lr.abs_diff(rr) <= 16 && lg.abs_diff(rg) <= 16 && lb.abs_diff(rb) <= 16
        })
        .count();
    u16::try_from(compatible.saturating_mul(1_000) / before.pixels.len()).unwrap_or(1_000)
}

#[cfg(test)]
mod tests {
    use super::{
        frame_is_browser, omnibox_focus_events, visual_identity_per_mille, DamageCoverage,
        MIN_RECONNECT_IDENTITY_PER_MILLE,
    };
    use mde_vdi_core::{DamageRect, FrameDamage};
    use mde_vdi_rdp::egui::{Color32, ColorImage, Event, Key, PointerButton};

    #[test]
    fn every_navigation_focuses_the_current_omnibox() {
        let events = omnibox_focus_events();
        assert!(matches!(
            &events[0],
            Event::Key {
                key: Key::L,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl
        ));
        assert!(matches!(
            &events[1],
            Event::Key {
                key: Key::L,
                pressed: false,
                modifiers,
                ..
            } if modifiers.ctrl
        ));
        assert!(matches!(
            &events[2],
            Event::PointerButton {
                button: PointerButton::Primary,
                pressed: false,
                modifiers,
                ..
            } if !modifiers.ctrl
        ));
    }

    #[test]
    fn reconnect_identity_tolerates_small_codec_quantization() {
        let mut before = ColorImage::new([320, 240], Color32::from_rgb(245, 246, 248));
        for (index, pixel) in before.pixels.iter_mut().enumerate() {
            if index % 2 == 0 {
                *pixel = Color32::from_rgb(
                    u8::try_from(index % 251).unwrap_or_default(),
                    u8::try_from((index * 3) % 251).unwrap_or_default(),
                    u8::try_from((index * 7) % 251).unwrap_or_default(),
                );
            }
        }
        assert!(frame_is_browser(&before));
        let mut after = before.clone();
        for pixel in &mut after.pixels {
            let [red, green, blue, _] = pixel.to_array();
            *pixel = Color32::from_rgb(red & 0xf8, green & 0xfc, blue & 0xf8);
        }
        assert!(visual_identity_per_mille(&before, &after) >= MIN_RECONNECT_IDENTITY_PER_MILLE);
    }

    #[test]
    fn unrelated_desktop_cannot_be_a_reconnect_receipt() {
        let before = ColorImage::new([320, 240], Color32::WHITE);
        let after = ColorImage::new([320, 240], Color32::BLACK);
        assert_eq!(visual_identity_per_mille(&before, &after), 0);
    }

    #[test]
    fn reconnect_damage_counts_unique_inbound_pixels() {
        let mut coverage = DamageCoverage::new([100, 100]);
        coverage.record(&FrameDamage::Rects(vec![DamageRect::new(0, 0, 50, 100)]));
        assert_eq!(coverage.per_mille(), 500);
        coverage.record(&FrameDamage::Rects(vec![DamageRect::new(0, 0, 50, 100)]));
        assert_eq!(coverage.per_mille(), 500);
        coverage.record(&FrameDamage::Rects(vec![DamageRect::new(50, 0, 50, 100)]));
        assert_eq!(coverage.per_mille(), 1_000);
    }
}
