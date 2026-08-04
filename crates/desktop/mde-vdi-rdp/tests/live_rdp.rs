//! E12-4 live remainder — attach the REAL crate path to a REAL RDP server.
//!
//! The unit suite proves the decode→egui and egui→input surfaces on synthetic
//! bytes, and `connect.rs`'s own tests prove the config/input mapping; this
//! test proves the assembled stack against a live RDP endpoint (the E12-4
//! acceptance "an RDP connection to a test guest renders live"), mirroring the
//! E12-6 VNC proof (`mde-vdi-vnc/tests/live_console.rs`).
//!
//! Everything goes through the crate's public API — [`RdpConnection::connect`]
//! runs the real ironrdp connection sequence *built from the session's codec
//! tier* ([`RdpSession::connect_settings`], E12-10), the pump decodes real
//! framebuffer updates into the session via the same `apply_rect` path the
//! unit tests drive, [`RdpSession::frame`] yields the egui [`ColorImage`] the
//! shell would upload, and [`RdpSession::send_input`] +
//! [`RdpConnection::flush_input`] put a real keystroke on the wire. The tier
//! contract is then exercised end-to-end: pin a lighter tier, observe
//! `needs_reconnect`, reconnect, and render again at the new depth.
//!
//! Env-gated + `#[ignore]` — a live server cannot exist in CI. Run:
//!
//! ```text
//! MDE_RDP_LIVE_TARGET=127.0.0.1:13389,mde,mde-live-proof \
//!   cargo test -p mde-vdi-rdp --features live-connect --test live_rdp \
//!   -- --ignored --nocapture
//! ```
//!
//! Chromium VM acceptance additionally sets
//! `MDE_RDP_LIVE_POINTER_PROBE=chromium-app-menu`. That strict probe moves the
//! real remote pointer to Chromium's three-dot menu button, proves the desktop
//! is quiet, then sends a primary-button click at the *same* point. It accepts
//! only a dense, menu-sized, pointer-anchored, predominantly local framebuffer
//! change delivered by inbound damage, and Escape must restore that same region
//! to its pre-click pixels. Reconnect must then repaint most of the desktop from
//! inbound damage while retaining the same endpoint and visual workload
//! identity. Login transitions, page animation, outbound-only writes, cursor
//! movement, and inherited pre-reconnect pixels therefore fail closed.
//! `MDE_RDP_LIVE_CAPTURE_PPM=/absolute/path.ppm` optionally writes the settled
//! pre-click framebuffer for artifact-bound diagnostics; it does not alter any
//! acceptance decision.
//!
//! (target format `host:port[,user,pass]`; user/pass default to the
//! `mde`/`mde-live-proof` fixture account of the xrdp proof container).

#![cfg(feature = "live-connect")]
#![allow(
    clippy::panic,
    reason = "test-only transport: a live-probe failure must abort with typed \
              wire-level evidence, and panicking IS the test failure mechanism"
)]

use std::time::{Duration, Instant};

use mde_vdi_core::{DamageRect, FrameDamage};
use mde_vdi_rdp::egui::{pos2, ColorImage, Event, Key, Modifiers, PointerButton};
use mde_vdi_rdp::link::{QualityMode, QualityTier};
use mde_vdi_rdp::{PumpOutcome, RdpConfig, RdpConnection, RdpSession};

const POINTER_PROBE_ENV: &str = "MDE_RDP_LIVE_POINTER_PROBE";
const CHROMIUM_POINTER_PROBE: &str = "chromium-app-menu";
const POINTER_CLICK_RADIUS: usize = 1_024;
const MIN_POINTER_CLICK_PIXELS: usize = 4_096;
const MIN_CONTEXT_MENU_EDGE: usize = 96;
const MAX_CONTEXT_MENU_WIDTH: usize = 768;
const CHROMIUM_MENU_REFRESH_WIDTH: u16 = 640;
// Modern Chromium's tall white menu can overlay a white New Tab page.  In that
// case only text, separators, shadows, and highlighted rows differ, yielding
// about 119 changed pixels per thousand inside the menu bounds.  The A→menu→A
// restoration, inbound-damage, anchoring, locality, and minimum-area checks
// below remain the primary fail-closed proof; retain margin below the measured
// density without allowing cursor-sized noise.
const MIN_CONTEXT_MENU_DENSITY_PER_MILLE: usize = 100;
const MIN_CONTEXT_MENU_LOCALITY_PER_MILLE: usize = 750;
const MAX_CONTEXT_MENU_SCREEN_PER_MILLE: usize = 450;
const CONTEXT_MENU_ANCHOR_MARGIN: usize = 56;
const MIN_CONTEXT_MENU_RESTORE_PER_MILLE: usize = 900;
const MAX_CONTEXT_MENU_RESIDUAL_PER_MILLE: usize = 100;
// Chromium's caret, throbber, and composited software pointer can touch a few
// hundred pixels while the page is otherwise stationary. Keep this well below
// the 4,096-pixel menu threshold so ambient chrome cannot satisfy the proof.
const MAX_POINTER_QUIET_NOISE_PIXELS: usize = 512;
// Chromium's intentionally sparse error/interstitial pages can quantize below
// 64 colors on xrdp's classic bitmap path (the live Google CAPTCHA frame has
// 43). Thirty-two still rejects a blank canvas; the reversible browser-menu
// challenge and reconnect identity checks below provide the strict workload
// proof rather than relying on color richness alone.
const MIN_CHROMIUM_DISTINCT_COLORS: usize = 32;
// A software cursor can contain dozens of antialiased colors while covering
// only a few hundred pixels on an otherwise black xorgxrdp root window. Require
// a real browser-sized foreground in addition to color variety so cursor-only
// sessions can never qualify as Chromium.
const MIN_CHROMIUM_NON_DOMINANT_PIXELS: usize = 16_384;
const MIN_RECONNECT_DAMAGE_PER_MILLE: usize = 700;
const MIN_RECONNECT_IDENTITY_PER_MILLE: usize = 750;
const CHROMIUM_DESKTOP_PX: (u16, u16) = (1920, 1080);

/// FNV-1a 64 over the frame's RGBA bytes — the pixel checksum recorded as
/// evidence (stable across runs for an unchanged screen).
fn fnv1a64(image: &ColorImage) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for px in &image.pixels {
        for byte in px.to_array() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

fn maybe_write_capture(image: &ColorImage) {
    let Ok(path) = std::env::var("MDE_RDP_LIVE_CAPTURE_PPM") else {
        return;
    };
    assert!(
        path.starts_with('/'),
        "MDE_RDP_LIVE_CAPTURE_PPM must be an absolute path"
    );
    write_capture(image, &path);
}

fn maybe_write_capture_variant(image: &ColorImage, variant: &str) {
    let Ok(path) = std::env::var("MDE_RDP_LIVE_CAPTURE_PPM") else {
        return;
    };
    assert!(
        path.starts_with('/') && path.ends_with(".ppm"),
        "MDE_RDP_LIVE_CAPTURE_PPM must be an absolute .ppm path"
    );
    let path = format!("{}-{variant}.ppm", path.trim_end_matches(".ppm"));
    write_capture(image, &path);
}

fn write_capture(image: &ColorImage, path: &str) {
    let mut ppm = format!("P6\n{} {}\n255\n", image.size[0], image.size[1]).into_bytes();
    ppm.reserve(image.pixels.len().saturating_mul(3));
    for pixel in &image.pixels {
        let [red, green, blue, _alpha] = pixel.to_array();
        ppm.extend_from_slice(&[red, green, blue]);
    }
    std::fs::write(path, ppm).unwrap_or_else(|error| {
        panic!("live: failed to write framebuffer capture {path}: {error}")
    });
    println!("live: settled framebuffer capture written to {path}");
}

/// Distinct RGBA values in the frame — a rendered desktop shows more than a
/// blank surface would. The generic live test accepts the first painted frame
/// by default; Browser VM acceptance can raise the minimum with
/// `MDE_RDP_LIVE_MIN_DISTINCT_COLORS` so xrdp's pre-session login bitmap cannot
/// masquerade as the Chromium desktop.
fn distinct_colors(image: &ColorImage) -> usize {
    let mut seen: std::collections::HashSet<[u8; 4]> = std::collections::HashSet::with_capacity(64);
    for px in &image.pixels {
        seen.insert(px.to_array());
    }
    seen.len()
}

/// Count pixels which are not the frame's most common color. Chromium's chrome
/// and page content occupy a substantial region; an empty root window plus an
/// antialiased pointer does not.
fn non_dominant_pixels(image: &ColorImage) -> usize {
    let mut counts = std::collections::HashMap::<[u8; 4], usize>::new();
    for pixel in &image.pixels {
        *counts.entry(pixel.to_array()).or_default() += 1;
    }
    image
        .pixels
        .len()
        .saturating_sub(counts.into_values().max().unwrap_or(0))
}

fn required_distinct_colors() -> usize {
    let Ok(raw) = std::env::var("MDE_RDP_LIVE_MIN_DISTINCT_COLORS") else {
        return 1;
    };
    let required = raw
        .parse::<usize>()
        .expect("MDE_RDP_LIVE_MIN_DISTINCT_COLORS must be an integer");
    assert!(
        (1..=4096).contains(&required),
        "MDE_RDP_LIVE_MIN_DISTINCT_COLORS must be between 1 and 4096"
    );
    required
}

/// The generic xrdp fixture does not promise Chromium UI. Chromium VM runs opt
/// into the strict right-click challenge explicitly so the old live transport
/// proof remains useful against its xterm fixture without weakening Browser
/// acceptance.
fn chromium_pointer_probe_requested() -> bool {
    match std::env::var(POINTER_PROBE_ENV) {
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("{POINTER_PROBE_ENV} must be valid Unicode")
        }
        Ok(value) => {
            assert_eq!(
                value, CHROMIUM_POINTER_PROBE,
                "{POINTER_PROBE_ENV} must be {CHROMIUM_POINTER_PROBE}"
            );
            true
        }
    }
}

/// Parse `host:port[,user,pass]`, defaulting the credentials to the xrdp
/// proof-container fixture account.
fn parse_target(raw: &str) -> (String, u16, String, String) {
    let mut parts = raw.split(',');
    let hostport = parts.next().expect("split always yields one part");
    let (host, port_str) = hostport
        .rsplit_once(':')
        .expect("MDE_RDP_LIVE_TARGET must start with host:port");
    let port: u16 = port_str.parse().expect("MDE_RDP_LIVE_TARGET port parses");
    let user = parts.next().unwrap_or("mde").to_owned();
    let pass = parts.next().unwrap_or("mde-live-proof").to_owned();
    (host.to_owned(), port, user, pass)
}

/// Pump until at least one region has been painted AND the session yields a
/// frame, or the deadline passes. Returns the frame + how many regions were
/// painted getting there.
fn pump_until_frame(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    deadline: Duration,
    what: &str,
    required_colors: usize,
    required_non_dominant_pixels: usize,
) -> (ColorImage, usize) {
    let start = Instant::now();
    let mut painted_total = 0_usize;
    let mut most_colors = 0_usize;
    let mut most_non_dominant_pixels = 0_usize;
    let mut last_checksum = None;
    while start.elapsed() < deadline {
        match conn.pump_once(session, Duration::from_secs(5)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                painted_total += painted_rects;
                if painted_rects > 0 {
                    if let Some(frame) = session.frame() {
                        let colors = distinct_colors(&frame);
                        let non_dominant = non_dominant_pixels(&frame);
                        let improved =
                            colors > most_colors || non_dominant > most_non_dominant_pixels;
                        most_colors = most_colors.max(colors);
                        most_non_dominant_pixels = most_non_dominant_pixels.max(non_dominant);
                        last_checksum = Some(fnv1a64(&frame));
                        if improved {
                            maybe_write_capture_variant(&frame, "qualification-best");
                        }
                        if colors >= required_colors && non_dominant >= required_non_dominant_pixels
                        {
                            return (frame, painted_total);
                        }
                    }
                }
            }
            Ok(PumpOutcome::TimedOut) => {} // keep waiting inside the deadline
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated while waiting for {what}: {reason}")
            }
            Err(e) => panic!("live: pump failed while waiting for {what}: {e}"),
        }
    }
    panic!(
        "live: no qualifying framebuffer decoded for {what} within {}s \
         ({painted_total} rects, most_colors={most_colors}, last_checksum={last_checksum:?}, \
         required_colors={required_colors}, \
         most_non_dominant_pixels={most_non_dominant_pixels}, \
         required_non_dominant_pixels={required_non_dominant_pixels})",
        deadline.as_secs(),
    );
}

/// Drain whatever the server still wants to send for up to `window`, then
/// return the latest frame if anything repainted.
fn settle(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    window: Duration,
) -> Option<ColorImage> {
    let start = Instant::now();
    let mut latest = None;
    while start.elapsed() < window {
        match conn.pump_once(session, Duration::from_millis(700)) {
            Ok(PumpOutcome::Processed { .. }) => {
                if let Some(frame) = session.frame() {
                    latest = Some(frame);
                }
            }
            Ok(PumpOutcome::TimedOut) => break, // server went quiet — settled
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated during settle: {reason}")
            }
            Err(e) => panic!("live: pump failed during settle: {e}"),
        }
    }
    latest
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelBounds {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl PixelBounds {
    fn around(center: (u16, u16), radius: usize, size: [usize; 2]) -> Self {
        let x = usize::from(center.0);
        let y = usize::from(center.1);
        assert!(
            x < size[0] && y < size[1],
            "live: pointer challenge lies outside the framebuffer"
        );
        Self {
            left: x.saturating_sub(radius),
            top: y.saturating_sub(radius),
            right: x.saturating_add(radius).saturating_add(1).min(size[0]),
            bottom: y.saturating_add(radius).saturating_add(1).min(size[1]),
        }
    }

    const fn one_pixel(x: usize, y: usize) -> Self {
        Self {
            left: x,
            top: y,
            right: x + 1,
            bottom: y + 1,
        }
    }

    fn include(&mut self, x: usize, y: usize) {
        self.left = self.left.min(x);
        self.top = self.top.min(y);
        self.right = self.right.max(x.saturating_add(1));
        self.bottom = self.bottom.max(y.saturating_add(1));
    }

    const fn width(self) -> usize {
        self.right - self.left
    }

    const fn height(self) -> usize {
        self.bottom - self.top
    }

    const fn area(self) -> usize {
        self.width() * self.height()
    }

    fn near_corner(self, point: (u16, u16), margin: usize) -> bool {
        let x = usize::from(point.0);
        let y = usize::from(point.1);
        let right = self.right.saturating_sub(1);
        let bottom = self.bottom.saturating_sub(1);
        x.abs_diff(self.left).min(x.abs_diff(right)) <= margin
            && y.abs_diff(self.top).min(y.abs_diff(bottom)) <= margin
    }

    const fn intersects(self, other: Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    fn from_damage(rect: DamageRect, size: [usize; 2]) -> Option<Self> {
        rect.clamped(size[0], size[1]).map(|rect| Self {
            left: rect.x(),
            top: rect.y(),
            right: rect.x().saturating_add(rect.w()),
            bottom: rect.y().saturating_add(rect.h()),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ChangeSummary {
    changed_total: usize,
    changed_near_pointer: usize,
    bounds_near_pointer: Option<PixelBounds>,
}

impl ChangeSummary {
    fn looks_like_context_menu(self, point: (u16, u16), size: [usize; 2]) -> bool {
        let Some(bounds) = self.bounds_near_pointer else {
            return false;
        };
        let screen_pixels = size[0].saturating_mul(size[1]);
        let bounds_area = bounds.area();
        self.changed_near_pointer >= MIN_POINTER_CLICK_PIXELS
            && (MIN_CONTEXT_MENU_EDGE..=MAX_CONTEXT_MENU_WIDTH).contains(&bounds.width())
            && bounds.height() >= MIN_CONTEXT_MENU_EDGE
            && bounds.height() <= size[1]
            && bounds.near_corner(point, CONTEXT_MENU_ANCHOR_MARGIN)
            && self.changed_near_pointer.saturating_mul(1_000)
                >= bounds_area.saturating_mul(MIN_CONTEXT_MENU_DENSITY_PER_MILLE)
            && self.changed_near_pointer.saturating_mul(1_000)
                >= self
                    .changed_total
                    .saturating_mul(MIN_CONTEXT_MENU_LOCALITY_PER_MILLE)
            && self.changed_total.saturating_mul(1_000)
                <= screen_pixels.saturating_mul(MAX_CONTEXT_MENU_SCREEN_PER_MILLE)
    }
}

/// Describe exact pixel changes after the click. The local bounds and locality
/// ratio reject broad login transitions; edge, density, and corner anchoring
/// reject a cursor, spinner, or unrelated animated patch.
fn summarize_changes(
    before: &ColorImage,
    after: &ColorImage,
    center: (u16, u16),
    radius: usize,
) -> ChangeSummary {
    assert_eq!(
        before.size, after.size,
        "live: pointer reaction changed framebuffer geometry"
    );
    let width = before.size[0];
    let challenge = PixelBounds::around(center, radius, before.size);
    let mut summary = ChangeSummary::default();
    for (index, (before_pixel, after_pixel)) in before.pixels.iter().zip(&after.pixels).enumerate()
    {
        if before_pixel == after_pixel {
            continue;
        }
        summary.changed_total += 1;
        let x = index % width;
        let y = index / width;
        if x < challenge.left || x >= challenge.right || y < challenge.top || y >= challenge.bottom
        {
            continue;
        }
        summary.changed_near_pointer += 1;
        match &mut summary.bounds_near_pointer {
            Some(bounds) => bounds.include(x, y),
            None => summary.bounds_near_pointer = Some(PixelBounds::one_pixel(x, y)),
        }
    }
    summary
}

fn inbound_damage_hits(
    damage: Option<&FrameDamage>,
    target: PixelBounds,
    size: [usize; 2],
) -> bool {
    match damage {
        None => false,
        Some(FrameDamage::Full) => true,
        Some(FrameDamage::Rects(rects)) => rects.iter().copied().any(|rect| {
            PixelBounds::from_damage(rect, size).is_some_and(|rect| rect.intersects(target))
        }),
    }
}

/// Require a real quiet control window before injection. Repeated damage with
/// identical pixels is harmless, but animation in the challenged region resets
/// the window and eventually fails instead of becoming the click's "reaction".
fn wait_for_quiet_pointer_frame(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    mut latest: ColorImage,
    point: (u16, u16),
    deadline: Duration,
    quiet_window: Duration,
    what: &str,
) -> ColorImage {
    let started = Instant::now();
    let mut quiet_since = Instant::now();
    let mut painted_total = 0_usize;
    let mut most_noise = 0_usize;
    while started.elapsed() < deadline {
        if quiet_since.elapsed() >= quiet_window {
            return latest;
        }
        match conn.pump_once(session, Duration::from_millis(250)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                painted_total += painted_rects;
                if painted_rects == 0 {
                    continue;
                }
                let (frame, _damage) = session.frame_with_damage().unwrap_or_else(|| {
                    panic!("live: inbound paint produced no frame while waiting for {what}")
                });
                let changed = summarize_changes(&latest, &frame, point, POINTER_CLICK_RADIUS)
                    .changed_near_pointer;
                most_noise = most_noise.max(changed);
                if changed > MAX_POINTER_QUIET_NOISE_PIXELS {
                    quiet_since = Instant::now();
                }
                latest = frame;
            }
            Ok(PumpOutcome::TimedOut) => {}
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated while waiting for {what}: {reason}")
            }
            Err(e) => panic!("live: pump failed while waiting for {what}: {e}"),
        }
    }
    panic!(
        "live: challenged region never became quiet for {}ms while waiting for {what} \
         (deadline={}s point={},{} painted_rects={painted_total} most_noise={most_noise})",
        quiet_window.as_millis(),
        deadline.as_secs(),
        point.0,
        point.1,
    );
}

fn pump_until_context_menu_opens(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    before: &ColorImage,
    point: (u16, u16),
    deadline: Duration,
) -> (ColorImage, ChangeSummary, usize) {
    let started = Instant::now();
    let challenge = PixelBounds::around(point, POINTER_CLICK_RADIUS, before.size);
    let mut painted_total = 0_usize;
    let mut saw_inbound_damage = false;
    let mut best = ChangeSummary::default();
    let mut last_checksum = None;
    while started.elapsed() < deadline {
        match conn.pump_once(session, Duration::from_secs(1)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                painted_total += painted_rects;
                if painted_rects == 0 {
                    continue;
                }
                let (frame, damage) = session.frame_with_damage().unwrap_or_else(|| {
                    panic!("live: inbound context-menu paint produced no frame")
                });
                saw_inbound_damage |= inbound_damage_hits(Some(&damage), challenge, before.size);
                let summary = summarize_changes(before, &frame, point, POINTER_CLICK_RADIUS);
                if summary.changed_near_pointer > best.changed_near_pointer {
                    best = summary;
                    maybe_write_capture_variant(&frame, "best-menu-reaction");
                }
                last_checksum = Some(fnv1a64(&frame));
                let current_damage_hits_menu = summary
                    .bounds_near_pointer
                    .is_some_and(|bounds| inbound_damage_hits(Some(&damage), bounds, before.size));
                if saw_inbound_damage
                    && current_damage_hits_menu
                    && summary.looks_like_context_menu(point, before.size)
                {
                    return (frame, summary, painted_total);
                }
            }
            Ok(PumpOutcome::TimedOut) => {}
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated while waiting for Chromium context menu: {reason}")
            }
            Err(e) => panic!("live: pump failed while waiting for Chromium context menu: {e}"),
        }
    }
    panic!(
        "live: no strict Chromium context-menu reaction within {}s \
         (point={},{} painted_rects={painted_total} inbound_damage={saw_inbound_damage} \
         best={best:?} last_checksum={last_checksum:?})",
        deadline.as_secs(),
        point.0,
        point.1,
    );
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RestorationSummary {
    opened_changed: usize,
    restored: usize,
    changed_since_open: usize,
    residual_from_baseline: usize,
}

impl RestorationSummary {
    fn proves_context_menu_closed(self) -> bool {
        self.opened_changed >= MIN_POINTER_CLICK_PIXELS
            && self.restored.saturating_mul(1_000)
                >= self
                    .opened_changed
                    .saturating_mul(MIN_CONTEXT_MENU_RESTORE_PER_MILLE)
            && self.changed_since_open.saturating_mul(1_000)
                >= self
                    .opened_changed
                    .saturating_mul(MIN_CONTEXT_MENU_RESTORE_PER_MILLE)
            && self.residual_from_baseline.saturating_mul(1_000)
                <= self
                    .opened_changed
                    .saturating_mul(MAX_CONTEXT_MENU_RESIDUAL_PER_MILLE)
    }
}

fn summarize_restoration(
    baseline: &ColorImage,
    opened: &ColorImage,
    closed: &ColorImage,
    bounds: PixelBounds,
) -> RestorationSummary {
    assert_eq!(
        baseline.size, opened.size,
        "live: menu-open geometry changed"
    );
    assert_eq!(
        baseline.size, closed.size,
        "live: menu-close geometry changed"
    );
    let width = baseline.size[0];
    let mut summary = RestorationSummary::default();
    for y in bounds.top..bounds.bottom {
        for x in bounds.left..bounds.right {
            let index = y * width + x;
            if baseline.pixels[index] != opened.pixels[index] {
                summary.opened_changed += 1;
                if closed.pixels[index] == baseline.pixels[index] {
                    summary.restored += 1;
                }
                if closed.pixels[index] != opened.pixels[index] {
                    summary.changed_since_open += 1;
                }
            }
            if closed.pixels[index] != baseline.pixels[index] {
                summary.residual_from_baseline += 1;
            }
        }
    }
    summary
}

fn dismiss_chromium_context_menu(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    baseline: &ColorImage,
    opened: &ColorImage,
    bounds: PixelBounds,
) -> (ColorImage, RestorationSummary, usize) {
    for pressed in [true, false] {
        session.send_input(&Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        });
    }
    let cleanup_sent = conn
        .flush_input(session)
        .unwrap_or_else(|e| panic!("live: pointer-probe cleanup flush failed: {e}"));
    assert_eq!(cleanup_sent, 2, "Escape cleanup emits key-down + key-up");
    let _local_escape_frame = session.frame_with_damage();

    let started = Instant::now();
    let deadline = Duration::from_secs(10);
    let mut painted_total = 0_usize;
    let mut saw_inbound_damage = false;
    let mut best = RestorationSummary::default();
    while started.elapsed() < deadline {
        match conn.pump_once(session, Duration::from_secs(1)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                painted_total += painted_rects;
                if painted_rects == 0 {
                    continue;
                }
                let (frame, damage) = session.frame_with_damage().unwrap_or_else(|| {
                    panic!("live: inbound context-menu dismissal produced no frame")
                });
                let current_damage_hits_menu =
                    inbound_damage_hits(Some(&damage), bounds, baseline.size);
                saw_inbound_damage |= current_damage_hits_menu;
                let summary = summarize_restoration(baseline, opened, &frame, bounds);
                if summary.restored > best.restored {
                    best = summary;
                }
                if saw_inbound_damage
                    && current_damage_hits_menu
                    && summary.proves_context_menu_closed()
                {
                    return (frame, summary, painted_total);
                }
            }
            Ok(PumpOutcome::TimedOut) => {}
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated while dismissing Chromium context menu: {reason}")
            }
            Err(e) => panic!("live: pump failed while dismissing Chromium context menu: {e}"),
        }
    }
    panic!(
        "live: Chromium context menu did not restore its pre-click region within {}s \
         (painted_rects={painted_total} inbound_damage={saw_inbound_damage} best={best:?})",
        deadline.as_secs(),
    );
}

fn visual_identity_similarity_per_mille(before: &ColorImage, after: &ColorImage) -> usize {
    assert_eq!(
        before.size, after.size,
        "live: reconnect changed visual-identity geometry"
    );
    assert!(!before.pixels.is_empty(), "live: empty identity anchor");
    let close_pixels = before
        .pixels
        .iter()
        .zip(&after.pixels)
        .filter(|(before, after)| {
            let [before_r, before_g, before_b, _] = before.to_array();
            let [after_r, after_g, after_b, _] = after.to_array();
            before_r.abs_diff(after_r) <= 12
                && before_g.abs_diff(after_g) <= 8
                && before_b.abs_diff(after_b) <= 12
        })
        .count();
    close_pixels.saturating_mul(1_000) / before.pixels.len()
}

struct DamageCoverage {
    size: [usize; 2],
    pixels: Vec<bool>,
    covered: usize,
}

impl DamageCoverage {
    fn new(size: [usize; 2]) -> Self {
        Self {
            size,
            pixels: vec![false; size[0].saturating_mul(size[1])],
            covered: 0,
        }
    }

    fn record(&mut self, damage: &FrameDamage) {
        match damage {
            FrameDamage::Full => {
                self.pixels.fill(true);
                self.covered = self.pixels.len();
            }
            FrameDamage::Rects(rects) => {
                for rect in rects {
                    let Some(bounds) = PixelBounds::from_damage(*rect, self.size) else {
                        continue;
                    };
                    for y in bounds.top..bounds.bottom {
                        for x in bounds.left..bounds.right {
                            let index = y * self.size[0] + x;
                            if !self.pixels[index] {
                                self.pixels[index] = true;
                                self.covered += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    fn per_mille(&self) -> usize {
        if self.pixels.is_empty() {
            0
        } else {
            self.covered.saturating_mul(1_000) / self.pixels.len()
        }
    }
}

fn assert_same_reconnect_endpoint(before: &RdpConfig, after: &RdpConfig) {
    assert_eq!(
        after.host, before.host,
        "reconnect changed RDP host identity"
    );
    assert_eq!(
        after.port, before.port,
        "reconnect changed RDP port identity"
    );
    assert_eq!(
        after.username, before.username,
        "reconnect changed guest user identity"
    );
    assert_eq!(
        after.domain, before.domain,
        "reconnect changed guest domain identity"
    );
    assert!(
        after.password == before.password,
        "reconnect changed the in-memory guest credential"
    );
    assert_eq!(
        (after.width, after.height),
        (before.width, before.height),
        "reconnect changed desktop identity geometry"
    );
}

fn pump_until_reconnect_identity_frame(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    anchor: &ColorImage,
    deadline: Duration,
    required_colors: usize,
) -> (ColorImage, usize, usize, usize) {
    let started = Instant::now();
    let mut damage_coverage = DamageCoverage::new(anchor.size);
    let mut painted_total = 0_usize;
    let mut most_colors = 0_usize;
    let mut best_identity = 0_usize;
    let mut last_checksum = None;
    while started.elapsed() < deadline {
        match conn.pump_once(session, Duration::from_secs(2)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                painted_total += painted_rects;
                if painted_rects == 0 {
                    continue;
                }
                let (frame, damage) = session
                    .frame_with_damage()
                    .unwrap_or_else(|| panic!("live: inbound reconnect paint produced no frame"));
                damage_coverage.record(&damage);
                let colors = distinct_colors(&frame);
                let identity = visual_identity_similarity_per_mille(anchor, &frame);
                most_colors = most_colors.max(colors);
                best_identity = best_identity.max(identity);
                last_checksum = Some(fnv1a64(&frame));
                let coverage = damage_coverage.per_mille();
                if coverage >= MIN_RECONNECT_DAMAGE_PER_MILLE
                    && colors >= required_colors
                    && identity >= MIN_RECONNECT_IDENTITY_PER_MILLE
                {
                    return (frame, painted_total, coverage, identity);
                }
            }
            Ok(PumpOutcome::TimedOut) => {}
            Ok(PumpOutcome::Terminated { reason }) => {
                panic!("live: server terminated while proving reconnect identity: {reason}")
            }
            Err(e) => panic!("live: pump failed while proving reconnect identity: {e}"),
        }
    }
    panic!(
        "live: reconnect did not repaint and preserve the Chromium workload within {}s \
         (painted_rects={painted_total} damage_per_mille={} required_damage={} \
         most_colors={most_colors} required_colors={required_colors} \
         best_identity={best_identity} required_identity={} last_checksum={last_checksum:?})",
        deadline.as_secs(),
        damage_coverage.per_mille(),
        MIN_RECONNECT_DAMAGE_PER_MILLE,
        MIN_RECONNECT_IDENTITY_PER_MILLE,
    );
}

struct ChromiumPointerProof {
    point: (u16, u16),
    identity_anchor: ColorImage,
}

/// Execute an input-correlated A→menu→A challenge. The opening and closing
/// transitions must each arrive as inbound damage, which is much stronger than
/// accepting an arbitrary checksum change after an outbound click.
#[allow(
    clippy::too_many_lines,
    reason = "one auditable live challenge: quiet control → primary app-menu click → \
              menu shape → Escape restoration"
)]
fn prove_chromium_pointer_input(
    conn: &mut RdpConnection,
    session: &mut RdpSession,
    current: &ColorImage,
) -> ChromiumPointerProof {
    let (width, height) = session.desktop_size();
    // The guest runtime and fresh xorgxrdp session share the requested desktop
    // geometry. Chromium's three-dot app-menu button is a stable browser-owned
    // target at the top-right of that output. Clicking arbitrary page space is
    // not deterministic (New Tab search/customize controls move with content).
    let challenge = (width.saturating_sub(22), 62);

    for pressed in [true, false] {
        session.send_input(&Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        });
    }
    assert_eq!(
        conn.flush_input(session)
            .unwrap_or_else(|e| panic!("live: preflight Escape flush failed: {e}")),
        2,
        "preflight Escape emits key-down + key-up"
    );
    let ready = wait_for_quiet_pointer_frame(
        conn,
        session,
        current.clone(),
        challenge,
        Duration::from_secs(10),
        Duration::from_millis(1_500),
        "Chromium transient overlays to close",
    );

    // Establish pointer focus inside the nested Sway output before targeting
    // Chromium.  xorgxrdp keeps its own last absolute coordinate across RDP
    // reconnects; a challenge that starts at that same coordinate can be
    // deduplicated before wlroots observes a motion/enter event.  Use a visible
    // but browser-local detour so the subsequent click cannot inherit stale
    // pointer focus from an earlier connection.
    let detour = (challenge.0.saturating_sub(96), challenge.1);
    session.send_input(&Event::PointerMoved(pos2(
        f32::from(detour.0),
        f32::from(detour.1),
    )));
    let detour_sent = conn
        .flush_input(session)
        .unwrap_or_else(|e| panic!("live: pointer detour flush failed: {e}"));
    assert_eq!(detour_sent, 1, "pointer detour emits one move PDU");
    std::thread::sleep(Duration::from_millis(50));

    session.send_input(&Event::PointerMoved(pos2(
        f32::from(challenge.0),
        f32::from(challenge.1),
    )));
    let move_sent = conn
        .flush_input(session)
        .unwrap_or_else(|e| panic!("live: pointer move flush failed: {e}"));
    assert_eq!(move_sent, 1, "pointer challenge emits one move PDU");
    assert_eq!(
        session.pointer_position(),
        challenge,
        "session must track the challenged absolute coordinate"
    );
    // Drain local software-cursor output before the negative-control window.
    // Only a later pump_once can contribute to the opening proof.
    let _local_move_frame = session.frame_with_damage();
    let before_click = wait_for_quiet_pointer_frame(
        conn,
        session,
        ready,
        challenge,
        Duration::from_secs(10),
        Duration::from_millis(1_500),
        "the pre-click quiet control",
    );
    println!(
        "live: POINTER ARMED point={},{} sent={move_sent} quiet_ms=1500 fnv1a64={:#018x}",
        challenge.0,
        challenge.1,
        fnv1a64(&before_click),
    );
    maybe_write_capture(&before_click);

    let challenge_pos = pos2(f32::from(challenge.0), f32::from(challenge.1));
    session.send_input(&Event::PointerButton {
        pos: challenge_pos,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
    });
    let down_sent = conn
        .flush_input(session)
        .unwrap_or_else(|e| panic!("live: pointer-down flush failed: {e}"));
    assert_eq!(down_sent, 1, "primary down emits one PDU");

    // Keep the two physical button transitions in distinct FastPathInput
    // frames.  A real click has a non-zero hold interval; sending down + up in
    // one frame lets some xrdp/module paths observe an already-released button
    // before the desktop's input loop runs.  The bounded hold is long enough
    // to cross one ordinary compositor frame without weakening the inbound
    // A→menu→A pixel challenge below.
    std::thread::sleep(Duration::from_millis(75));
    session.send_input(&Event::PointerButton {
        pos: challenge_pos,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
    });
    let up_sent = conn
        .flush_input(session)
        .unwrap_or_else(|e| panic!("live: pointer-up flush failed: {e}"));
    assert_eq!(up_sent, 1, "primary up emits one PDU");
    let click_sent = down_sent + up_sent;
    let _local_click_frame = session.frame_with_damage();
    // xrdp can coalesce a Chromium popup's transient classic-bitmap damage even
    // though the menu is already present in the Xorg framebuffer. Ask only for
    // the top-right menu strip after the bounded click interval; this is the
    // standard RDP recovery path and avoids a wasteful whole-screen repaint.
    std::thread::sleep(Duration::from_millis(250));
    let refresh_width = width.min(CHROMIUM_MENU_REFRESH_WIDTH);
    conn.request_refresh_area(width - refresh_width, 0, refresh_width, height)
        .unwrap_or_else(|e| panic!("live: post-click menu refresh request failed: {e}"));
    let (opening_frame, _opening_summary, opening_rects) = pump_until_context_menu_opens(
        conn,
        session,
        &before_click,
        challenge,
        Duration::from_secs(10),
    );
    let opened = wait_for_quiet_pointer_frame(
        conn,
        session,
        opening_frame,
        challenge,
        // The compositor finishes the menu animation quickly, but xrdp's
        // classic-bitmap stream can deliver the resulting tall-menu repaint in
        // several batches after the first qualifying frame.  Allow that
        // bounded backlog to drain before taking the B frame for A→B→A proof.
        Duration::from_secs(12),
        Duration::from_millis(700),
        "the opened context menu to settle",
    );
    let menu = summarize_changes(&before_click, &opened, challenge, POINTER_CLICK_RADIUS);
    assert!(
        menu.looks_like_context_menu(challenge, before_click.size),
        "live: settled click reaction is not a strict context menu: {menu:?}"
    );
    let menu_bounds = menu
        .bounds_near_pointer
        .expect("qualified context menu has local bounds");

    let (closed, restoration, closing_rects) =
        dismiss_chromium_context_menu(conn, session, &before_click, &opened, menu_bounds);
    println!(
        "live: POINTER CLICK VERIFIED kind={CHROMIUM_POINTER_PROBE} point={},{} \
         sent={click_sent} opening_rects={opening_rects} closing_rects={closing_rects} \
         changed_near={} changed_total={} bounds={menu_bounds:?} \
         restored={}/{} residual={} before={:#018x} opened={:#018x} closed={:#018x}",
        challenge.0,
        challenge.1,
        menu.changed_near_pointer,
        menu.changed_total,
        restoration.restored,
        restoration.opened_changed,
        restoration.residual_from_baseline,
        fnv1a64(&before_click),
        fnv1a64(&opened),
        fnv1a64(&closed),
    );

    ChromiumPointerProof {
        point: challenge,
        identity_anchor: closed,
    }
}

/// The live acceptance: real connection sequence (tier-driven), ≥1 real
/// framebuffer update decoded through the crate's public session path into an
/// egui [`ColorImage`], a keystroke forwarded (echo recorded honestly), and
/// the E12-10 tier contract exercised with a real reconnect at a lighter tier.
#[test]
#[ignore = "live RDP server required — set MDE_RDP_LIVE_TARGET=host:port[,user,pass] (see module docs)"]
#[allow(
    clippy::too_many_lines,
    reason = "one linear protocol script — connect → frame → input → tier \
              reconnect reads best unbroken, mirroring the E12-6 VNC proof"
)]
fn live_rdp_renders_accepts_input_and_applies_tier_on_reconnect() {
    let Ok(target) = std::env::var("MDE_RDP_LIVE_TARGET") else {
        eprintln!("live: SKIP — MDE_RDP_LIVE_TARGET not set (host:port[,user,pass])");
        return;
    };
    let (host, port, user, pass) = parse_target(&target);
    let required_colors = required_distinct_colors();
    let require_pointer_probe = chromium_pointer_probe_requested();
    if require_pointer_probe {
        assert!(
            required_colors >= MIN_CHROMIUM_DISTINCT_COLORS,
            "strict Chromium pointer proof requires \
             MDE_RDP_LIVE_MIN_DISTINCT_COLORS >= {MIN_CHROMIUM_DISTINCT_COLORS}"
        );
    }
    println!("live: desktop qualification requires at least {required_colors} distinct colors");
    println!(
        "live: Chromium pointer challenge {}",
        if require_pointer_probe {
            "REQUIRED"
        } else {
            "not requested"
        }
    );

    let desktop_px = if require_pointer_probe {
        CHROMIUM_DESKTOP_PX
    } else {
        (1024, 768)
    };
    let config = RdpConfig::new(host, user, pass)
        .with_port(port)
        .with_resolution(desktop_px.0, desktop_px.1);
    let mut session = RdpSession::new(config).expect("live target config is valid");
    let reconnect_endpoint = session.config().clone();
    // Consume the initial all-black frame so the first frame we record below
    // is genuinely the decoded remote desktop, not the constructor's canvas.
    let _initial = session.frame();

    // ── Connect at the default tier (Full: 32-bpp classic bitmaps) ─────────
    assert_eq!(session.quality_tier(), QualityTier::Full);
    assert_eq!(session.connect_settings().color_depth, 32);
    let mut conn = RdpConnection::connect(&mut session)
        .unwrap_or_else(|e| panic!("live: connect failed: {e}"));
    assert!(
        !session.needs_reconnect(),
        "connect must mark the negotiated tier applied"
    );
    assert_eq!(session.applied_tier(), QualityTier::Full);
    let negotiated = conn.negotiated().clone();
    println!(
        "live: CONNECTED tier={:?} desktop={}x{} compression={:?} io_channel={} user_channel={}",
        negotiated.tier,
        negotiated.desktop_size.0,
        negotiated.desktop_size.1,
        negotiated.compression,
        negotiated.io_channel_id,
        negotiated.user_channel_id,
    );
    assert_eq!(
        negotiated.desktop_size,
        session.desktop_size(),
        "server must grant the requested desktop geometry"
    );

    // ── ≥1 real framebuffer update through the crate into an egui image ─────
    let (image, rects) = pump_until_frame(
        &mut conn,
        &mut session,
        Duration::from_secs(60),
        "the first desktop paint",
        required_colors,
        if require_pointer_probe {
            MIN_CHROMIUM_NON_DOMINANT_PIXELS
        } else {
            0
        },
    );
    assert_eq!(
        image.size,
        [
            usize::from(negotiated.desktop_size.0),
            usize::from(negotiated.desktop_size.1)
        ],
        "frame geometry must match the negotiated desktop"
    );
    if require_pointer_probe {
        assert_eq!(
            negotiated.desktop_size, CHROMIUM_DESKTOP_PX,
            "Chromium acceptance requires an exact 1920x1080 negotiated desktop"
        );
    }
    assert!(!image.pixels.is_empty(), "live frame decoded no pixels");
    let checksum = fnv1a64(&image);
    let colors = distinct_colors(&image);
    println!(
        "live: FRAME OK {}x{} rects={rects} fnv1a64={checksum:#018x} \
         distinct_colors={colors} non_dominant_pixels={}",
        image.size[0],
        image.size[1],
        non_dominant_pixels(&image),
    );
    // Let the session finish painting (xrdp sends the desktop in waves) so the
    // following input proof starts from a settled screen.
    let settled = settle(&mut conn, &mut session, Duration::from_secs(10));
    let mut current_image = settled.unwrap_or(image);
    let baseline = fnv1a64(&current_image);
    println!("live: settled baseline fnv1a64={baseline:#018x}");

    if require_pointer_probe {
        // A fresh Chromium window focuses its omnibox. The generic xterm probe
        // below would navigate to a search for "m" and make that page load look
        // like click-correlated damage. The strict A→menu→A challenge proves
        // pointer delivery and proves Escape keyboard delivery by requiring the
        // exact menu region to return to its pre-click pixels.
        println!("live: Chromium input uses the reversible app-menu + Escape challenge");
    } else {
        // ── Generic input round-trip (best effort, recorded honestly) ────────
        // Type "m" + Enter through the same session API the shell drives; the
        // fixture xterm echoes, so pixels should move.
        session.send_input(&Event::Text("m".to_owned()));
        for pressed in [true, false] {
            session.send_input(&Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::default(),
            });
        }
        let sent = conn
            .flush_input(&mut session)
            .unwrap_or_else(|e| panic!("live: input flush failed: {e}"));
        assert!(sent >= 3, "text + key-down + key-up must reach the wire");
        println!("live: sent {sent} fast-path input events via RdpConnection::flush_input");

        std::thread::sleep(Duration::from_millis(700));
        match settle(&mut conn, &mut session, Duration::from_secs(10)) {
            Some(after) => {
                let checksum_after = fnv1a64(&after);
                if checksum_after == baseline {
                    println!(
                        "live: INPUT sent OK; framebuffer UNCHANGED after keystroke \
                         (fnv1a64={checksum_after:#018x}) — desktop may not echo"
                    );
                } else {
                    println!(
                        "live: INPUT ECHOED — framebuffer changed after keystroke \
                         (before={baseline:#018x} after={checksum_after:#018x})"
                    );
                }
                current_image = after;
            }
            None => println!("live: INPUT sent OK; server repainted nothing afterwards"),
        }
    }

    let pointer_proof = require_pointer_probe
        .then(|| prove_chromium_pointer_input(&mut conn, &mut session, &current_image));

    // ── E12-10 tier contract, exercised live ────────────────────────────────
    // Pin a lighter tier: the target moves, the session honestly demands a
    // reconnect, and reconnecting through the same public entry point applies
    // it — the next connection is negotiated at 16-bpp with bulk compression.
    let change = session
        .set_quality_mode(QualityMode::Pinned(QualityTier::Compressed), 0)
        .expect("pinning a lighter tier is a target change");
    assert_eq!(change.to, QualityTier::Compressed);
    assert!(
        session.needs_reconnect(),
        "RDP tiers are reconnect-gated (RdpTierSettings::APPLICATION)"
    );
    assert_eq!(session.connect_settings().color_depth, 16);
    conn.shutdown(&mut session)
        .unwrap_or_else(|e| panic!("live: graceful shutdown failed: {e}"));
    // A shutdown-side local redraw cannot count toward reconnect coverage.
    let _local_shutdown_frame = session.frame_with_damage();

    let mut conn2 = RdpConnection::connect(&mut session)
        .unwrap_or_else(|e| panic!("live: tier reconnect failed: {e}"));
    assert!(
        !session.needs_reconnect(),
        "reconnect applied the pinned tier"
    );
    assert_eq!(session.applied_tier(), QualityTier::Compressed);
    let renegotiated = conn2.negotiated().clone();
    println!(
        "live: RECONNECTED tier={:?} desktop={}x{} compression={:?}",
        renegotiated.tier,
        renegotiated.desktop_size.0,
        renegotiated.desktop_size.1,
        renegotiated.compression,
    );
    assert_eq!(renegotiated.tier, QualityTier::Compressed);
    if let Some(proof) = pointer_proof.as_ref() {
        assert_same_reconnect_endpoint(&reconnect_endpoint, session.config());
        assert_eq!(
            session.pointer_position(),
            proof.point,
            "reconnect replaced the challenged RDP session identity"
        );
    }

    let (image2, rects2) = if let Some(proof) = pointer_proof.as_ref() {
        let (image, rects, coverage, identity) = pump_until_reconnect_identity_frame(
            &mut conn2,
            &mut session,
            &proof.identity_anchor,
            Duration::from_secs(60),
            required_colors,
        );
        println!(
            "live: RECONNECT IDENTITY OK point={},{} inbound_damage_per_mille={coverage} \
             visual_identity_per_mille={identity}",
            proof.point.0, proof.point.1,
        );
        (image, rects)
    } else {
        pump_until_frame(
            &mut conn2,
            &mut session,
            Duration::from_secs(60),
            "the post-reconnect paint",
            required_colors,
            0,
        )
    };
    let checksum2 = fnv1a64(&image2);
    println!(
        "live: TIER FRAME OK {}x{} rects={rects2} fnv1a64={checksum2:#018x} distinct_colors={}",
        image2.size[0],
        image2.size[1],
        distinct_colors(&image2)
    );
    conn2
        .shutdown(&mut session)
        .unwrap_or_else(|e| panic!("live: final shutdown failed: {e}"));
}

#[cfg(test)]
mod strict_pointer_gate_tests {
    use mde_vdi_core::{DamageRect, FrameDamage};
    use mde_vdi_rdp::egui::{Color32, ColorImage};

    use super::{
        distinct_colors, inbound_damage_hits, non_dominant_pixels, summarize_changes,
        summarize_restoration, visual_identity_similarity_per_mille, DamageCoverage, PixelBounds,
        MIN_CHROMIUM_DISTINCT_COLORS, MIN_CHROMIUM_NON_DOMINANT_PIXELS,
        MIN_RECONNECT_IDENTITY_PER_MILLE, POINTER_CLICK_RADIUS,
    };

    const SIZE: [usize; 2] = [640, 480];
    const POINT: (u16, u16) = (240, 180);
    const MENU: PixelBounds = PixelBounds {
        left: 238,
        top: 178,
        right: 418,
        bottom: 398,
    };

    fn paint(image: &mut ColorImage, bounds: PixelBounds, color: Color32) {
        for y in bounds.top..bounds.bottom {
            for x in bounds.left..bounds.right {
                image.pixels[y * image.size[0] + x] = color;
            }
        }
    }

    fn menu_frames() -> (ColorImage, ColorImage) {
        let baseline = ColorImage::new(SIZE, Color32::from_rgb(24, 30, 38));
        let mut opened = baseline.clone();
        paint(&mut opened, MENU, Color32::from_rgb(238, 240, 244));
        (baseline, opened)
    }

    #[test]
    fn dense_pointer_anchored_menu_and_reversible_close_qualify() {
        let (baseline, opened) = menu_frames();
        let menu = summarize_changes(&baseline, &opened, POINT, POINTER_CLICK_RADIUS);
        assert!(menu.looks_like_context_menu(POINT, SIZE), "{menu:?}");
        assert_eq!(menu.bounds_near_pointer, Some(MENU));

        let closed = baseline.clone();
        let restoration = summarize_restoration(&baseline, &opened, &closed, MENU);
        assert!(restoration.proves_context_menu_closed(), "{restoration:?}");
        assert_eq!(restoration.restored, MENU.area());
        assert_eq!(restoration.residual_from_baseline, 0);
    }

    #[test]
    fn sparse_pointer_anchored_menu_can_qualify() {
        let baseline = ColorImage::new(SIZE, Color32::WHITE);
        let mut opened = baseline.clone();
        for y in MENU.top..MENU.bottom {
            if (y - MENU.top) % 8 != 0 && y + 1 != MENU.bottom {
                continue;
            }
            for x in MENU.left..MENU.right {
                opened.pixels[y * SIZE[0] + x] = Color32::from_rgb(80, 84, 92);
            }
        }

        let menu = summarize_changes(&baseline, &opened, POINT, POINTER_CLICK_RADIUS);
        assert!(menu.looks_like_context_menu(POINT, SIZE), "{menu:?}");
        assert_eq!(menu.bounds_near_pointer, Some(MENU));

        let restoration = summarize_restoration(&baseline, &opened, &baseline, MENU);
        assert!(restoration.proves_context_menu_closed(), "{restoration:?}");
    }

    #[test]
    fn antialiased_cursor_on_black_cannot_qualify_as_chromium() {
        let mut cursor_only = ColorImage::new(SIZE, Color32::BLACK);
        for index in 0..512 {
            let shade = u8::try_from(index % 64).expect("bounded shade") * 4;
            cursor_only.pixels[index] = Color32::from_gray(shade);
        }
        assert!(distinct_colors(&cursor_only) >= MIN_CHROMIUM_DISTINCT_COLORS);
        assert!(
            non_dominant_pixels(&cursor_only) < MIN_CHROMIUM_NON_DOMINANT_PIXELS,
            "cursor-sized color variety must not satisfy browser qualification"
        );
    }

    #[test]
    fn login_transition_and_animation_noise_do_not_look_like_a_menu() {
        let baseline = ColorImage::new(SIZE, Color32::BLACK);

        let login = ColorImage::new(SIZE, Color32::WHITE);
        let login_change = summarize_changes(&baseline, &login, POINT, POINTER_CLICK_RADIUS);
        assert!(!login_change.looks_like_context_menu(POINT, SIZE));

        let mut centered_animation = baseline.clone();
        paint(
            &mut centered_animation,
            PixelBounds {
                left: 140,
                top: 80,
                right: 340,
                bottom: 280,
            },
            Color32::WHITE,
        );
        let centered =
            summarize_changes(&baseline, &centered_animation, POINT, POINTER_CLICK_RADIUS);
        assert!(
            !centered.looks_like_context_menu(POINT, SIZE),
            "a dense animation centered on the pointer is not pointer-anchored: {centered:?}"
        );

        let mut sparse_noise = baseline.clone();
        paint(
            &mut sparse_noise,
            PixelBounds {
                left: 238,
                top: 178,
                right: 278,
                bottom: 218,
            },
            Color32::WHITE,
        );
        let sparse = summarize_changes(&baseline, &sparse_noise, POINT, POINTER_CLICK_RADIUS);
        assert!(
            !sparse.looks_like_context_menu(POINT, SIZE),
            "small cursor/spinner noise must stay below the proof floor: {sparse:?}"
        );

        let (menu_baseline, mut menu_plus_animation) = menu_frames();
        paint(
            &mut menu_plus_animation,
            PixelBounds {
                left: 540,
                top: 20,
                right: 640,
                bottom: 420,
            },
            Color32::WHITE,
        );
        let noisy_menu = summarize_changes(
            &menu_baseline,
            &menu_plus_animation,
            POINT,
            POINTER_CLICK_RADIUS,
        );
        assert!(
            !noisy_menu.looks_like_context_menu(POINT, SIZE),
            "a menu-shaped local change cannot hide dominant unrelated animation: {noisy_menu:?}"
        );
    }

    #[test]
    fn menu_that_does_not_restore_fails_the_a_b_a_challenge() {
        let (baseline, opened) = menu_frames();
        let stuck = summarize_restoration(&baseline, &opened, &opened, MENU);
        assert!(!stuck.proves_context_menu_closed());

        let mut half_restored = opened.clone();
        paint(
            &mut half_restored,
            PixelBounds {
                left: MENU.left,
                top: MENU.top,
                right: MENU.right,
                bottom: MENU.top + MENU.height() / 2,
            },
            Color32::from_rgb(24, 30, 38),
        );
        let partial = summarize_restoration(&baseline, &opened, &half_restored, MENU);
        assert!(
            !partial.proves_context_menu_closed(),
            "an unrelated or partial repaint cannot stand in for Escape: {partial:?}"
        );
    }

    #[test]
    fn proof_fails_closed_without_inbound_damage_at_the_menu() {
        let target = MENU;
        assert!(!inbound_damage_hits(None, target, SIZE));

        let far = FrameDamage::Rects(vec![DamageRect::new(0, 0, 32, 32)]);
        assert!(!inbound_damage_hits(Some(&far), target, SIZE));

        let local = FrameDamage::Rects(vec![DamageRect::new(250, 200, 20, 20)]);
        assert!(inbound_damage_hits(Some(&local), target, SIZE));
        assert!(inbound_damage_hits(Some(&FrameDamage::Full), target, SIZE));
    }

    #[test]
    fn reconnect_coverage_counts_unique_inbound_pixels() {
        let mut coverage = DamageCoverage::new([100, 100]);
        coverage.record(&FrameDamage::Rects(vec![DamageRect::new(0, 0, 50, 100)]));
        assert_eq!(coverage.per_mille(), 500);

        coverage.record(&FrameDamage::Rects(vec![DamageRect::new(0, 0, 50, 100)]));
        assert_eq!(coverage.per_mille(), 500, "overlap is not double-counted");

        coverage.record(&FrameDamage::Rects(vec![DamageRect::new(50, 0, 50, 100)]));
        assert_eq!(coverage.per_mille(), 1_000);
    }

    #[test]
    fn reconnect_identity_tolerates_rgb565_but_rejects_a_different_screen() {
        let mut browser = ColorImage::new([320, 240], Color32::from_rgb(245, 246, 248));
        paint(
            &mut browser,
            PixelBounds {
                left: 0,
                top: 0,
                right: 320,
                bottom: 48,
            },
            Color32::from_rgb(38, 44, 54),
        );
        paint(
            &mut browser,
            PixelBounds {
                left: 28,
                top: 12,
                right: 286,
                bottom: 36,
            },
            Color32::from_rgb(214, 219, 227),
        );
        paint(
            &mut browser,
            PixelBounds {
                left: 32,
                top: 80,
                right: 288,
                bottom: 208,
            },
            Color32::from_rgb(92, 132, 220),
        );

        let mut rgb565 = browser.clone();
        for pixel in &mut rgb565.pixels {
            let [red, green, blue, _] = pixel.to_array();
            *pixel = Color32::from_rgb(red & 0xf8, green & 0xfc, blue & 0xf8);
        }
        let compatible = visual_identity_similarity_per_mille(&browser, &rgb565);
        assert!(compatible >= MIN_RECONNECT_IDENTITY_PER_MILLE);

        let login = ColorImage::new([320, 240], Color32::from_rgb(8, 12, 18));
        let unrelated = visual_identity_similarity_per_mille(&browser, &login);
        assert!(
            unrelated < MIN_RECONNECT_IDENTITY_PER_MILLE,
            "a different login/layout must not preserve visual identity: {unrelated}"
        );
    }
}
