//! Header-pinned CEF windowless browser creation for the native bridge.
//!
//! This module carries only the C callback surface needed to prove that the
//! pinned CEF 149 runtime can create an offscreen browser and invoke paint. The
//! full shell socket lifecycle is intentionally a later slice; this keeps the
//! probe honest while replacing the previous "offscreen pending" blocker with a
//! real browser-process boundary.

use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::os::raw::c_int;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::cef_abi::{CefAbi, CefStringUserfreeUtf16Free};
use crate::offscreen::{OffscreenError, OffscreenFrameSink};
use crate::sock::{self, RecvOutcome};
use crate::wire::{self, ControlMsg, EventMsg, InputEvent, KeyCode, Modifiers, PointerButton};

/// `sizeof(cef_base_ref_counted_t)` for pinned Linux CEF 149.
pub const CEF_BASE_REF_COUNTED_SIZE: usize = 40;
/// `sizeof(cef_window_info_t)` for pinned Linux CEF 149.
pub const CEF_WINDOW_INFO_SIZE: usize = 88;
/// `offsetof(cef_window_info_t, bounds)`.
pub const CEF_WINDOW_INFO_BOUNDS_OFFSET: usize = 32;
/// `offsetof(cef_window_info_t, windowless_rendering_enabled)`.
pub const CEF_WINDOW_INFO_WINDOWLESS_OFFSET: usize = 56;
/// `offsetof(cef_window_info_t, shared_texture_enabled)`.
pub const CEF_WINDOW_INFO_SHARED_TEXTURE_OFFSET: usize = 60;
/// `offsetof(cef_window_info_t, external_begin_frame_enabled)`.
pub const CEF_WINDOW_INFO_EXTERNAL_BEGIN_FRAME_OFFSET: usize = 64;
/// `offsetof(cef_window_info_t, runtime_style)`.
pub const CEF_WINDOW_INFO_RUNTIME_STYLE_OFFSET: usize = 80;
/// `cef_runtime_style_t::CEF_RUNTIME_STYLE_ALLOY` for pinned Linux CEF 149.
pub const CEF_RUNTIME_STYLE_ALLOY: i32 = 2;
/// `sizeof(cef_browser_settings_t)` for pinned Linux CEF 149.
pub const CEF_BROWSER_SETTINGS_SIZE: usize = 264;
/// `offsetof(cef_browser_settings_t, windowless_frame_rate)`.
pub const CEF_BROWSER_SETTINGS_FRAME_RATE_OFFSET: usize = 8;
/// `offsetof(cef_browser_settings_t, background_color)`.
pub const CEF_BROWSER_SETTINGS_BACKGROUND_COLOR_OFFSET: usize = 248;
/// `sizeof(cef_client_t)` for pinned Linux CEF 149.
pub const CEF_CLIENT_SIZE: usize = 192;
/// `offsetof(cef_client_t, get_life_span_handler)`.
pub const CEF_CLIENT_GET_LIFE_SPAN_HANDLER_OFFSET: usize = 144;
/// `offsetof(cef_client_t, get_print_handler)`.
pub const CEF_CLIENT_GET_PRINT_HANDLER_OFFSET: usize = 160;
/// `offsetof(cef_client_t, get_render_handler)`.
pub const CEF_CLIENT_GET_RENDER_HANDLER_OFFSET: usize = 168;
/// `offsetof(cef_client_t, get_request_handler)`.
pub const CEF_CLIENT_GET_REQUEST_HANDLER_OFFSET: usize = 176;
/// `sizeof(cef_life_span_handler_t)` for pinned Linux CEF 149.
pub const CEF_LIFE_SPAN_HANDLER_SIZE: usize = 88;
/// `offsetof(cef_life_span_handler_t, on_after_created)`.
pub const CEF_LIFE_SPAN_ON_AFTER_CREATED_OFFSET: usize = 64;
/// `sizeof(cef_render_handler_t)` for pinned Linux CEF 149.
pub const CEF_RENDER_HANDLER_SIZE: usize = 176;
/// `offsetof(cef_render_handler_t, get_view_rect)`.
pub const CEF_RENDER_HANDLER_GET_VIEW_RECT_OFFSET: usize = 56;
/// `offsetof(cef_render_handler_t, on_paint)`.
pub const CEF_RENDER_HANDLER_ON_PAINT_OFFSET: usize = 96;
/// `sizeof(cef_request_handler_t)` for pinned Linux CEF 149.
pub const CEF_REQUEST_HANDLER_SIZE: usize = 128;
/// `offsetof(cef_request_handler_t, get_resource_request_handler)`.
pub const CEF_REQUEST_HANDLER_GET_RESOURCE_REQUEST_HANDLER_OFFSET: usize = 56;
/// `sizeof(cef_resource_request_handler_t)` for pinned Linux CEF 149.
pub const CEF_RESOURCE_REQUEST_HANDLER_SIZE: usize = 104;
/// `offsetof(cef_resource_request_handler_t, on_before_resource_load)`.
pub const CEF_RESOURCE_REQUEST_HANDLER_ON_BEFORE_RESOURCE_LOAD_OFFSET: usize = 48;
const CEF_PAGE_TEXT_BEACON_PREFIX: &str = "https://mde-page-text.invalid/capture/";
const CEF_PAGE_TEXT_BEACON_LEGACY_PREFIX: &str = "mde-page-text://capture/";
const CEF_PAGE_TEXT_BEACON_MAX_BYTES: u32 = 8 * 1024;
const CEF_PAGE_SCRAPE_BEACON_PREFIX: &str = "https://mde-page-scrape.invalid/capture/";
const CEF_PAGE_SCRAPE_BEACON_MAX_BYTES: usize = 32 * 1024;
const CEF_PASSKEY_BEACON_PREFIX: &str = "https://mde-passkey.invalid/request/";
const CEF_PASSKEY_BEACON_MAX_BYTES: usize = 8 * 1024;
/// `sizeof(cef_print_handler_t)` for pinned Linux CEF 149.
pub const CEF_PRINT_HANDLER_SIZE: usize = 88;
/// `offsetof(cef_print_handler_t, on_print_dialog)`.
pub const CEF_PRINT_HANDLER_ON_PRINT_DIALOG_OFFSET: usize = 56;
/// `offsetof(cef_print_handler_t, on_print_job)`.
pub const CEF_PRINT_HANDLER_ON_PRINT_JOB_OFFSET: usize = 64;
/// `offsetof(cef_print_handler_t, get_pdf_paper_size)`.
pub const CEF_PRINT_HANDLER_GET_PDF_PAPER_SIZE_OFFSET: usize = 80;
/// `sizeof(cef_browser_t)` for pinned Linux CEF 149.
pub const CEF_BROWSER_SIZE: usize = 208;
/// `offsetof(cef_browser_t, get_host)`.
pub const CEF_BROWSER_GET_HOST_OFFSET: usize = 48;
/// `offsetof(cef_browser_t, can_go_back)`.
pub const CEF_BROWSER_CAN_GO_BACK_OFFSET: usize = 56;
/// `offsetof(cef_browser_t, go_back)`.
pub const CEF_BROWSER_GO_BACK_OFFSET: usize = 64;
/// `offsetof(cef_browser_t, can_go_forward)`.
pub const CEF_BROWSER_CAN_GO_FORWARD_OFFSET: usize = 72;
/// `offsetof(cef_browser_t, go_forward)`.
pub const CEF_BROWSER_GO_FORWARD_OFFSET: usize = 80;
/// `offsetof(cef_browser_t, reload)`.
pub const CEF_BROWSER_RELOAD_OFFSET: usize = 96;
/// `offsetof(cef_browser_t, stop_load)`.
pub const CEF_BROWSER_STOP_LOAD_OFFSET: usize = 112;
/// `offsetof(cef_browser_t, get_main_frame)`.
pub const CEF_BROWSER_GET_MAIN_FRAME_OFFSET: usize = 152;
/// `sizeof(cef_browser_host_t)` for pinned Linux CEF 149.
pub const CEF_BROWSER_HOST_SIZE: usize = 592;
/// `offsetof(cef_browser_host_t, close_browser)`.
pub const CEF_BROWSER_HOST_CLOSE_BROWSER_OFFSET: usize = 48;
/// `offsetof(cef_browser_host_t, set_focus)`.
pub const CEF_BROWSER_HOST_SET_FOCUS_OFFSET: usize = 72;
/// `offsetof(cef_browser_host_t, was_resized)`.
pub const CEF_BROWSER_HOST_WAS_RESIZED_OFFSET: usize = 304;
/// `offsetof(cef_browser_host_t, invalidate)`.
pub const CEF_BROWSER_HOST_INVALIDATE_OFFSET: usize = 328;
/// `offsetof(cef_browser_host_t, send_key_event)`.
pub const CEF_BROWSER_HOST_SEND_KEY_EVENT_OFFSET: usize = 344;
/// `offsetof(cef_browser_host_t, send_mouse_click_event)`.
pub const CEF_BROWSER_HOST_SEND_MOUSE_CLICK_EVENT_OFFSET: usize = 352;
/// `offsetof(cef_browser_host_t, send_mouse_move_event)`.
pub const CEF_BROWSER_HOST_SEND_MOUSE_MOVE_EVENT_OFFSET: usize = 360;
/// `offsetof(cef_browser_host_t, send_mouse_wheel_event)`.
pub const CEF_BROWSER_HOST_SEND_MOUSE_WHEEL_EVENT_OFFSET: usize = 368;
/// `offsetof(cef_browser_host_t, print)`.
pub const CEF_BROWSER_HOST_PRINT_OFFSET: usize = 504;
/// `offsetof(cef_browser_host_t, print_to_pdf)`.
pub const CEF_BROWSER_HOST_PRINT_TO_PDF_OFFSET: usize = 512;
/// `offsetof(cef_browser_host_t, set_audio_muted)`.
pub const CEF_BROWSER_HOST_SET_AUDIO_MUTED_OFFSET: usize = 520;
/// `offsetof(cef_browser_host_t, is_audio_muted)`.
pub const CEF_BROWSER_HOST_IS_AUDIO_MUTED_OFFSET: usize = 528;
/// `sizeof(cef_frame_t)` for pinned Linux CEF 149.
pub const CEF_FRAME_SIZE: usize = 248;
/// `offsetof(cef_frame_t, load_url)`.
pub const CEF_FRAME_LOAD_URL_OFFSET: usize = 144;
/// `offsetof(cef_frame_t, execute_java_script)`.
pub const CEF_FRAME_EXECUTE_JAVA_SCRIPT_OFFSET: usize = 152;
/// `sizeof(cef_request_t)` for pinned Linux CEF 149.
pub const CEF_REQUEST_SIZE: usize = 216;
/// `offsetof(cef_request_t, get_url)`.
pub const CEF_REQUEST_GET_URL_OFFSET: usize = 48;
/// `sizeof(cef_callback_t)` for pinned Linux CEF 149.
pub const CEF_CALLBACK_SIZE: usize = 56;
/// `offsetof(cef_callback_t, cont)`.
pub const CEF_CALLBACK_CONT_OFFSET: usize = 40;
/// `offsetof(cef_callback_t, cancel)`.
pub const CEF_CALLBACK_CANCEL_OFFSET: usize = 48;
/// `sizeof(cef_pdf_print_callback_t)` for pinned Linux CEF 149.
pub const CEF_PDF_PRINT_CALLBACK_SIZE: usize = 48;
/// `offsetof(cef_pdf_print_callback_t, on_pdf_print_finished)`.
pub const CEF_PDF_PRINT_CALLBACK_ON_FINISHED_OFFSET: usize = 40;
/// `sizeof(cef_mouse_event_t)` for pinned Linux CEF 149.
pub const CEF_MOUSE_EVENT_SIZE: usize = 12;
/// `offsetof(cef_mouse_event_t, x)`.
pub const CEF_MOUSE_EVENT_X_OFFSET: usize = 0;
/// `offsetof(cef_mouse_event_t, y)`.
pub const CEF_MOUSE_EVENT_Y_OFFSET: usize = 4;
/// `offsetof(cef_mouse_event_t, modifiers)`.
pub const CEF_MOUSE_EVENT_MODIFIERS_OFFSET: usize = 8;
/// `sizeof(cef_key_event_t)` for pinned Linux CEF 149.
pub const CEF_KEY_EVENT_SIZE: usize = 40;
/// `offsetof(cef_key_event_t, type)`.
pub const CEF_KEY_EVENT_TYPE_OFFSET: usize = 8;
/// `offsetof(cef_key_event_t, modifiers)`.
pub const CEF_KEY_EVENT_MODIFIERS_OFFSET: usize = 12;
/// `offsetof(cef_key_event_t, windows_key_code)`.
pub const CEF_KEY_EVENT_WINDOWS_KEY_CODE_OFFSET: usize = 16;
/// `offsetof(cef_key_event_t, native_key_code)`.
pub const CEF_KEY_EVENT_NATIVE_KEY_CODE_OFFSET: usize = 20;
/// `offsetof(cef_key_event_t, is_system_key)`.
pub const CEF_KEY_EVENT_IS_SYSTEM_KEY_OFFSET: usize = 24;
/// `offsetof(cef_key_event_t, character)`.
pub const CEF_KEY_EVENT_CHARACTER_OFFSET: usize = 28;
/// `offsetof(cef_key_event_t, unmodified_character)`.
pub const CEF_KEY_EVENT_UNMODIFIED_CHARACTER_OFFSET: usize = 30;
/// `offsetof(cef_key_event_t, focus_on_editable_field)`.
pub const CEF_KEY_EVENT_FOCUS_ON_EDITABLE_FIELD_OFFSET: usize = 32;

const BASE_SIZE_OFFSET: usize = 0;
const BASE_ADD_REF_OFFSET: usize = 8;
const BASE_RELEASE_OFFSET: usize = 16;
const BASE_HAS_ONE_REF_OFFSET: usize = 24;
const BASE_HAS_AT_LEAST_ONE_REF_OFFSET: usize = 32;
/// `sizeof(cef_rect_t)` for pinned Linux CEF 149.
pub const CEF_RECT_SIZE: usize = 16;
const PET_VIEW: c_int = 0;
const MBT_LEFT: c_int = 0;
const MBT_MIDDLE: c_int = 1;
const MBT_RIGHT: c_int = 2;
const KEYEVENT_RAWKEYDOWN: c_int = 0;
const KEYEVENT_KEYUP: c_int = 2;
const KEYEVENT_CHAR: c_int = 3;
const RV_CANCEL: c_int = 0;
const RV_CONTINUE: c_int = 1;
const RV_CONTINUE_ASYNC: c_int = 2;
const RESOURCE_OTHER: u8 = 255;
const EVENTFLAG_SHIFT_DOWN: c_int = 2;
const EVENTFLAG_CONTROL_DOWN: c_int = 4;
const EVENTFLAG_ALT_DOWN: c_int = 8;
const EVENTFLAG_LEFT_MOUSE_BUTTON: c_int = 16;
const EVENTFLAG_MIDDLE_MOUSE_BUTTON: c_int = 32;
const EVENTFLAG_RIGHT_MOUSE_BUTTON: c_int = 64;
const EVENTFLAG_COMMAND_DOWN: c_int = 128;

/// Result of a bounded windowless browser probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CefBrowserProbe {
    /// URL passed to CEF.
    pub url: String,
    /// Requested view width.
    pub width: u32,
    /// Requested view height.
    pub height: u32,
    /// Number of browser-created callbacks seen.
    pub created: usize,
    /// Number of view paint callbacks seen.
    pub paints: usize,
    /// Last paint width.
    pub last_paint_width: i32,
    /// Last paint height.
    pub last_paint_height: i32,
}

/// Result of a bounded page-text smoke probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CefTextProbe {
    /// Browser paint probe details.
    pub browser: CefBrowserProbe,
    /// Text marker that had to appear in visible page text.
    pub expected: String,
    /// Bytes captured from visible page text.
    pub text_bytes: usize,
}

impl CefBrowserProbe {
    /// Operator-facing status line.
    #[must_use]
    pub fn status_line(&self) -> String {
        format!(
            "CEF_BROWSER_PAINT_READY url={} view={}x{} created={} paints={} last_paint={}x{}",
            self.url,
            self.width,
            self.height,
            self.created,
            self.paints,
            self.last_paint_width,
            self.last_paint_height
        )
    }
}

impl CefTextProbe {
    /// Operator-facing status line.
    #[must_use]
    pub fn status_line(&self) -> String {
        format!(
            "CEF_TEXT_PROBE_READY url={} view={}x{} created={} paints={} marker_bytes={} text_bytes={}",
            self.browser.url,
            self.browser.width,
            self.browser.height,
            self.browser.created,
            self.browser.paints,
            self.expected.len(),
            self.text_bytes
        )
    }
}

/// Error from creating or pumping a windowless CEF browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CefBrowserError {
    /// URL contained an interior NUL-equivalent UTF-16 issue.
    BadUrl,
    /// CEF returned a null browser pointer.
    CreateReturnedNull,
    /// No paint callback arrived before the bounded deadline.
    TimedOut {
        /// Browser-created callbacks observed.
        created: usize,
        /// Paint callbacks observed.
        paints: usize,
    },
    /// Browser painted but the expected text marker was not observed.
    TextProbeMissing {
        /// Browser-created callbacks observed.
        created: usize,
        /// Paint callbacks observed.
        paints: usize,
        /// Last captured text byte count, if any page text arrived.
        text_bytes: usize,
    },
    /// Creating or publishing through the BOOKMARKS-6 frame sink failed.
    Offscreen(String),
}

impl fmt::Display for CefBrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadUrl => write!(f, "URL cannot be represented as a CEF string"),
            Self::CreateReturnedNull => {
                write!(f, "cef_browser_host_create_browser_sync returned null")
            }
            Self::TimedOut { created, paints } => write!(
                f,
                "timed out waiting for CEF paint callback (created={created} paints={paints})"
            ),
            Self::TextProbeMissing {
                created,
                paints,
                text_bytes,
            } => write!(
                f,
                "timed out waiting for CEF text marker (created={created} paints={paints} text_bytes={text_bytes})"
            ),
            Self::Offscreen(err) => write!(f, "CEF offscreen frame sink failed: {err}"),
        }
    }
}

impl std::error::Error for CefBrowserError {}

/// Create one offscreen browser and pump until the first view paint arrives.
///
/// # Errors
/// Returns [`CefBrowserError`] when CEF does not create the browser or no paint
/// arrives before `timeout`.
pub fn run_windowless_browser_probe(
    abi: &CefAbi,
    url: &str,
    width: u32,
    height: u32,
    timeout: Duration,
) -> Result<CefBrowserProbe, CefBrowserError> {
    run_windowless_browser_probe_with_stream(abi, url, width, height, timeout, None)
}

/// Create one offscreen browser, optionally attach its frames to `stream`, and
/// pump until the first view paint arrives.
///
/// # Errors
/// Returns [`CefBrowserError`] when CEF does not create the browser, frame-sink
/// setup fails, or no paint arrives before `timeout`.
pub fn run_windowless_browser_probe_with_stream(
    abi: &CefAbi,
    url: &str,
    width: u32,
    height: u32,
    timeout: Duration,
    stream: Option<&UnixStream>,
) -> Result<CefBrowserProbe, CefBrowserError> {
    let window_info = CefWindowInfo::windowless(width, height);
    let browser_settings = CefBrowserSettings::windowless(30);
    let url = CefStringOwned::new(url)?;
    let callbacks =
        CefBrowserCallbacks::new(width, height, stream, abi.string_userfree_utf16_free())?;

    let browser = abi.create_browser_sync(
        window_info.as_ptr(),
        callbacks.client_ptr(),
        url.as_ptr(),
        browser_settings.as_ptr(),
    );
    if browser.is_null() {
        abi.shutdown();
        return Err(CefBrowserError::CreateReturnedNull);
    }
    notify_browser_view_ready(browser);

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        abi.do_message_loop_work();
        if callbacks.paints() > 0 {
            let probe = CefBrowserProbe {
                url: url.text().to_owned(),
                width,
                height,
                created: callbacks.created(),
                paints: callbacks.paints(),
                last_paint_width: callbacks.last_paint_width(),
                last_paint_height: callbacks.last_paint_height(),
            };
            abi.shutdown();
            return Ok(probe);
        }
        thread::sleep(Duration::from_millis(10));
    }

    abi.shutdown();
    Err(CefBrowserError::TimedOut {
        created: callbacks.created(),
        paints: callbacks.paints(),
    })
}

/// Create a windowless browser, request visible page text, and require a marker.
///
/// This is the runtime smoke primitive used for live WebExtension proof: a test
/// extension can inject a visible marker into a page, and this probe only passes
/// when CEF paints and the existing page-text beacon observes that marker.
///
/// # Errors
/// Returns [`CefBrowserError`] when CEF does not paint or the marker is absent
/// before the bounded deadline.
pub fn run_windowless_text_probe(
    abi: &CefAbi,
    url: &str,
    width: u32,
    height: u32,
    timeout: Duration,
    expected: &str,
) -> Result<CefTextProbe, CefBrowserError> {
    const TEXT_PROBE_ID: u64 = u64::MAX - 7;
    const TEXT_PROBE_MAX_BYTES: u32 = 16 * 1024;

    let (helper, shell) = UnixStream::pair()?;
    helper.set_nonblocking(true)?;
    shell.set_nonblocking(true)?;

    let window_info = CefWindowInfo::windowless(width, height);
    let browser_settings = CefBrowserSettings::windowless(30);
    let url = CefStringOwned::new(url)?;
    let callbacks = CefBrowserCallbacks::new(
        width,
        height,
        Some(&helper),
        abi.string_userfree_utf16_free(),
    )?;

    let browser = abi.create_browser_sync(
        window_info.as_ptr(),
        callbacks.client_ptr(),
        url.as_ptr(),
        browser_settings.as_ptr(),
    );
    if browser.is_null() {
        abi.shutdown();
        return Err(CefBrowserError::CreateReturnedNull);
    }
    notify_browser_view_ready(browser);

    let deadline = Instant::now() + timeout;
    let mut rbuf = Vec::new();
    let mut first_paint = None;
    let mut last_text_bytes = 0;
    let mut last_text_request = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    while Instant::now() < deadline {
        abi.do_message_loop_work();
        match sock::recv(&shell) {
            Ok(RecvOutcome::Data { bytes, .. }) => {
                rbuf.extend_from_slice(&bytes);
                if let Some(text) = drain_page_text_events(&mut rbuf, TEXT_PROBE_ID) {
                    last_text_bytes = text.len();
                    if text.contains(expected) {
                        let browser_probe = first_paint.unwrap_or(CefBrowserProbe {
                            url: url.text().to_owned(),
                            width,
                            height,
                            created: callbacks.created(),
                            paints: callbacks.paints(),
                            last_paint_width: callbacks.last_paint_width(),
                            last_paint_height: callbacks.last_paint_height(),
                        });
                        close_browser(browser);
                        for _ in 0..8 {
                            abi.do_message_loop_work();
                            thread::sleep(Duration::from_millis(4));
                        }
                        abi.shutdown();
                        return Ok(CefTextProbe {
                            browser: browser_probe,
                            expected: expected.to_owned(),
                            text_bytes: last_text_bytes,
                        });
                    }
                }
            }
            Ok(RecvOutcome::WouldBlock) => {}
            Ok(RecvOutcome::Eof) | Err(_) => break,
        }
        if first_paint.is_none() && callbacks.paints() > 0 {
            first_paint = Some(CefBrowserProbe {
                url: url.text().to_owned(),
                width,
                height,
                created: callbacks.created(),
                paints: callbacks.paints(),
                last_paint_width: callbacks.last_paint_width(),
                last_paint_height: callbacks.last_paint_height(),
            });
        }
        if first_paint.is_some() && last_text_request.elapsed() >= Duration::from_millis(100) {
            request_page_text(browser, TEXT_PROBE_ID, TEXT_PROBE_MAX_BYTES);
            last_text_request = Instant::now();
        }
        thread::sleep(Duration::from_millis(8));
    }

    abi.shutdown();
    Err(CefBrowserError::TextProbeMissing {
        created: callbacks.created(),
        paints: callbacks.paints(),
        text_bytes: last_text_bytes,
    })
}

/// Pump interval while the tab is actively loading, painting, or receiving
/// input. Unchanged from the original 125 Hz spin so input/paint latency is not
/// regressed while active.
const PUMP_ACTIVE: Duration = Duration::from_millis(8);
/// Pump interval once the tab has gone quiet. An idle tab no longer spins at
/// 125 Hz; it wakes ~4x/s (and immediately on any socket control frame, since
/// the loop waits with `poll()` on the session fd rather than a blind sleep).
/// This is also the passkey outbound-drain heartbeat, so it doubles as the idle
/// floor rather than adding a second timer.
const PUMP_IDLE: Duration = Duration::from_millis(250);
/// Grace period of no paints/frames before the pump backs off to [`PUMP_IDLE`],
/// so a brief lull mid-interaction does not thrash the interval.
const PUMP_IDLE_AFTER: Duration = Duration::from_millis(200);
/// How long after the last activity a freshly-navigated document is still
/// treated as "settling", during which the per-context shims are re-applied (at
/// [`ShimInjector::SETTLE_INTERVAL`]) so a slow document commit is still covered
/// even though the pinned CEF ABI exposes no load/context-ready callback.
const SHIM_SETTLE: Duration = Duration::from_millis(1000);
/// Cadence of the passkey outbound-queue drain. The ceremony beacon/queue design
/// is inherently poll-based (a page-initiated `credentials.get()` enqueues a
/// request native must pick up), so a lightweight drain keeps running — but it no
/// longer recompiles the multi-KB bridge shim on every tick.
const PASSKEY_DRAIN_INTERVAL: Duration = Duration::from_millis(250);

/// perf-6: pick the next pump/poll interval from how active the tab is.
///
/// While awaiting the first paint or within [`PUMP_IDLE_AFTER`] of the last
/// paint/frame/navigation the tab is "active" and pumped at [`PUMP_ACTIVE`];
/// sustained quiet backs the pump off to [`PUMP_IDLE`] so an idle tab stops
/// spinning at 125 Hz. Input latency is preserved because the loop waits on the
/// session fd with `poll()`, which returns immediately when a control frame lands
/// regardless of the interval.
fn pump_interval(idle_for: Duration, awaiting_first_paint: bool) -> Duration {
    if awaiting_first_paint || idle_for < PUMP_IDLE_AFTER {
        PUMP_ACTIVE
    } else {
        PUMP_IDLE
    }
}

/// browser-8: decides when to (re)inject the per-context security shims (WebRTC
/// block + passkey bridge) so they land once per navigation generation instead of
/// on a blind 250 ms timer.
///
/// The pinned CEF ABI exposes no `OnContextCreated`/load-end callback, only an
/// `is_navigation` flag on the resource handler, so navigation is modelled as a
/// monotonic `generation` counter. A new generation always injects once. While
/// that generation is still `settling` (document committing / first paints
/// arriving) it re-injects at most once per [`Self::SETTLE_INTERVAL`] so a slow
/// commit is covered. Once the context is stable it never re-injects — the
/// per-document WebRTC `MutationObserver` keeps new subframes covered on its own.
#[derive(Debug, Default)]
struct ShimInjector {
    injected_generation: Option<u64>,
    last_inject: Option<Instant>,
}

impl ShimInjector {
    /// Bounded re-inject cadence while a fresh document is settling.
    const SETTLE_INTERVAL: Duration = Duration::from_millis(250);

    fn new() -> Self {
        Self::default()
    }

    /// Returns true when the shims should be injected this tick, recording the
    /// decision so the same stable context is never re-injected on a timer.
    fn should_inject(&mut self, generation: u64, settling: bool, now: Instant) -> bool {
        if self.injected_generation != Some(generation) {
            self.injected_generation = Some(generation);
            self.last_inject = Some(now);
            return true;
        }
        if settling {
            let due = self
                .last_inject
                .is_none_or(|last| now.duration_since(last) >= Self::SETTLE_INTERVAL);
            if due {
                self.last_inject = Some(now);
                return true;
            }
        }
        false
    }
}

/// Wait up to `timeout` for the session fd to become readable, so a control
/// frame wakes the loop immediately instead of after a fixed sleep. `poll()`
/// also reports `POLLHUP`/`POLLERR`, so a closed peer wakes us to observe EOF.
/// The return value is intentionally ignored: the next `sock::recv` classifies
/// data/would-block/EOF.
fn wait_for_readable(fd: RawFd, timeout: Duration) {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = c_int::try_from(timeout.as_millis())
        .unwrap_or(c_int::MAX)
        .max(1);
    // SAFETY: `pfd` is a single valid `pollfd` for the borrowed session fd; the
    // call only reads `events`/`fd` and writes `revents`.
    unsafe {
        libc::poll(std::ptr::addr_of_mut!(pfd), 1, millis);
    }
}

/// Serve one CEF tab over the BOOKMARKS-6 session socket until the shell closes
/// the socket. This currently supports initial load + frame publication; decoded
/// control frames are drained so the stream stays aligned while navigation/input
/// callbacks are added in the next slice.
///
/// # Errors
/// Returns [`CefBrowserError`] if browser creation or frame-sink setup fails.
pub fn run_windowless_tab(
    abi: &CefAbi,
    url: &str,
    width: u32,
    height: u32,
    stream: &UnixStream,
) -> Result<CefBrowserProbe, CefBrowserError> {
    stream.set_nonblocking(true)?;
    let window_info = CefWindowInfo::windowless(width, height);
    let browser_settings = CefBrowserSettings::windowless(30);
    let url = CefStringOwned::new(url)?;
    let callbacks = CefBrowserCallbacks::new(
        width,
        height,
        Some(stream),
        abi.string_userfree_utf16_free(),
    )?;

    let browser = abi.create_browser_sync(
        window_info.as_ptr(),
        callbacks.client_ptr(),
        url.as_ptr(),
        browser_settings.as_ptr(),
    );
    if browser.is_null() {
        abi.shutdown();
        return Err(CefBrowserError::CreateReturnedNull);
    }
    notify_browser_view_ready(browser);
    // Best-effort earliest injection, ahead of the poll loop below (§
    // `webrtc_block_script` doc comment covers why this cannot be airtight).
    inject_context_shims(browser);

    let mut first_paint = None;
    let started = Instant::now();
    let mut rbuf = Vec::new();
    let fd = stream.as_raw_fd();

    // browser-8: inject the per-context security shims (WebRTC block + passkey
    // bridge) once per navigation generation instead of on a fixed 250 ms timer.
    let mut shims = ShimInjector::new();
    let mut last_nav = callbacks.navigations();
    let mut last_passkey_drain = Instant::now();

    // perf-6: adaptive pump. `last_activity` tracks the last paint / control
    // frame / navigation so the pump runs fast while the tab is active and backs
    // off when idle instead of spinning at 125 Hz forever.
    let mut last_activity = Instant::now();
    let mut prev_paints = callbacks.paints();

    loop {
        abi.do_message_loop_work();
        match sock::recv(stream) {
            Ok(RecvOutcome::Data { bytes, .. }) => {
                rbuf.extend_from_slice(&bytes);
                drain_control_frames(&mut rbuf, browser, &callbacks);
                last_activity = Instant::now();
            }
            Ok(RecvOutcome::WouldBlock) => {}
            Ok(RecvOutcome::Eof) => break,
            Err(_) => break,
        }

        let paints = callbacks.paints();
        if paints != prev_paints {
            prev_paints = paints;
            last_activity = Instant::now();
        }
        if first_paint.is_none() && paints > 0 {
            first_paint = Some(CefBrowserProbe {
                url: url.text().to_owned(),
                width,
                height,
                created: callbacks.created(),
                paints,
                last_paint_width: callbacks.last_paint_width(),
                last_paint_height: callbacks.last_paint_height(),
            });
        }

        let nav = callbacks.navigations();
        if nav != last_nav {
            last_nav = nav;
            last_activity = Instant::now();
        }

        let now = Instant::now();
        let idle_for = now.duration_since(last_activity);
        let awaiting_first_paint = first_paint.is_none();
        // A freshly-navigated document is still "settling" while awaiting the
        // first paint or within SHIM_SETTLE of the last activity; re-inject the
        // shims through the commit, then leave the stable context alone.
        let settling = awaiting_first_paint || idle_for < SHIM_SETTLE;
        if shims.should_inject(nav, settling, now) {
            inject_context_shims(browser);
        }
        // Keep draining page-initiated passkey ceremonies (cheap; no shim
        // recompile) — this is genuine outbound polling, not shim re-injection.
        if last_passkey_drain.elapsed() >= PASSKEY_DRAIN_INTERVAL {
            poll_passkey_drain(browser);
            last_passkey_drain = now;
        }

        if awaiting_first_paint && started.elapsed() > Duration::from_secs(15) {
            abi.shutdown();
            return Err(CefBrowserError::TimedOut {
                created: callbacks.created(),
                paints: callbacks.paints(),
            });
        }

        wait_for_readable(fd, pump_interval(idle_for, awaiting_first_paint));
    }

    let probe = first_paint.unwrap_or(CefBrowserProbe {
        url: url.text().to_owned(),
        width,
        height,
        created: callbacks.created(),
        paints: callbacks.paints(),
        last_paint_width: callbacks.last_paint_width(),
        last_paint_height: callbacks.last_paint_height(),
    });
    close_browser(browser);
    for _ in 0..8 {
        abi.do_message_loop_work();
        thread::sleep(Duration::from_millis(4));
    }
    abi.shutdown();
    Ok(probe)
}

impl From<OffscreenError> for CefBrowserError {
    fn from(value: OffscreenError) -> Self {
        Self::Offscreen(value.to_string())
    }
}

impl From<std::io::Error> for CefBrowserError {
    fn from(value: std::io::Error) -> Self {
        Self::Offscreen(value.to_string())
    }
}

fn drain_control_frames(rbuf: &mut Vec<u8>, browser: *mut c_void, callbacks: &CefBrowserCallbacks) {
    loop {
        match wire::take_frame(rbuf) {
            Ok(Some(payload)) => {
                if let Ok(msg) = ControlMsg::decode(&payload) {
                    apply_control_frame(browser, callbacks, &msg);
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
}

fn drain_page_text_events(rbuf: &mut Vec<u8>, expected_id: u64) -> Option<String> {
    let mut latest = None;
    loop {
        match wire::take_frame(rbuf) {
            Ok(Some(payload)) => {
                if let Ok(EventMsg::PageText { id, text }) = EventMsg::decode(&payload) {
                    if id == expected_id {
                        latest = Some(text);
                    }
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    latest
}

fn apply_control_frame(browser: *mut c_void, callbacks: &CefBrowserCallbacks, msg: &ControlMsg) {
    match msg {
        ControlMsg::Load(url) => load_url(browser, url),
        ControlMsg::Reload => call_browser_void(browser, CEF_BROWSER_RELOAD_OFFSET),
        ControlMsg::Stop => call_browser_void(browser, CEF_BROWSER_STOP_LOAD_OFFSET),
        ControlMsg::Back => {
            if call_browser_bool(browser, CEF_BROWSER_CAN_GO_BACK_OFFSET) {
                call_browser_void(browser, CEF_BROWSER_GO_BACK_OFFSET);
            }
        }
        ControlMsg::Forward => {
            if call_browser_bool(browser, CEF_BROWSER_CAN_GO_FORWARD_OFFSET) {
                call_browser_void(browser, CEF_BROWSER_GO_FORWARD_OFFSET);
            }
        }
        ControlMsg::Resize { width, height } => {
            callbacks.resize(*width, *height);
            notify_browser_view_ready(browser);
        }
        ControlMsg::Input(event) => apply_input_event(browser, callbacks, event),
        ControlMsg::CosmeticFilters(css) => apply_cosmetic_filters(browser, css),
        ControlMsg::SetZoom { percent } => apply_page_zoom(browser, *percent),
        ControlMsg::FindInPage { query, backwards } => {
            apply_find_in_page(browser, query, *backwards)
        }
        ControlMsg::ClearFind => clear_find_in_page(browser),
        ControlMsg::SetAudioMuted { muted } => set_audio_muted(browser, *muted),
        ControlMsg::SetForceDark { enabled } => apply_force_dark(browser, *enabled),
        ControlMsg::SetReaderMode { enabled } => apply_reader_mode(browser, *enabled),
        ControlMsg::SetUserScripts { enabled, bundle } => {
            apply_user_scripts(browser, *enabled, bundle);
        }
        ControlMsg::SetUserAgent { user_agent } => apply_user_agent(browser, user_agent),
        ControlMsg::SetDeviceProfile {
            profile,
            width,
            height,
            scale_percent,
            touch,
        } => apply_device_profile(browser, profile, *width, *height, *scale_percent, *touch),
        ControlMsg::SetSpellcheckHighlights { words } => {
            apply_spellcheck_highlights(browser, words);
        }
        ControlMsg::ApplySpellcheckCorrection { word, replacement } => {
            apply_spellcheck_correction(browser, word, replacement);
        }
        ControlMsg::ApplySpellcheckCorrectionAll { word, replacement } => {
            apply_spellcheck_correction_all(browser, word, replacement);
        }
        ControlMsg::ApplySpellcheckCorrectionAt {
            word,
            replacement,
            occurrence,
        } => {
            apply_spellcheck_correction_at(browser, word, replacement, *occurrence);
        }
        ControlMsg::PrintPage => print_page(browser),
        ControlMsg::SavePdf { path } => save_pdf(browser, callbacks, path),
        ControlMsg::RequestPageText { id, max_bytes } => {
            request_page_text(browser, *id, *max_bytes);
        }
        ControlMsg::RequestPageScrape {
            id,
            max_bytes,
            max_links,
            max_headings,
        } => {
            request_page_scrape(browser, *id, *max_bytes, *max_links, *max_headings);
        }
        ControlMsg::CompletePasskey { body } => complete_passkey(browser, body),
        ControlMsg::ResourceVerdict { id, allow } => callbacks.apply_resource_verdict(*id, *allow),
    }
}

fn apply_input_event(browser: *mut c_void, callbacks: &CefBrowserCallbacks, event: &InputEvent) {
    let Some(host) = browser_host(browser) else {
        return;
    };
    match event {
        InputEvent::PointerMoved { x, y } => {
            let (x, y) = callbacks.update_pointer(*x, *y);
            send_mouse_move(host, x, y, 0, false);
        }
        InputEvent::PointerButton {
            x,
            y,
            button,
            pressed,
        } => {
            let (x, y) = callbacks.update_pointer(*x, *y);
            if *pressed {
                set_host_focus(host, true);
            }
            send_mouse_click(host, x, y, *button, *pressed);
        }
        InputEvent::PointerGone => {
            let (x, y) = callbacks.pointer_position();
            send_mouse_move(host, x, y, 0, true);
        }
        InputEvent::Scroll { delta_x, delta_y } => {
            let (x, y) = callbacks.pointer_position();
            send_mouse_wheel(host, x, y, *delta_x, *delta_y);
        }
        InputEvent::Key {
            key,
            pressed,
            modifiers,
        } => {
            set_host_focus(host, true);
            send_key(host, *key, *pressed, *modifiers);
        }
        InputEvent::Text(text) => {
            set_host_focus(host, true);
            for unit in text.encode_utf16() {
                send_char(host, unit, Modifiers::default());
            }
        }
    }
}

struct CefWindowInfo {
    bytes: [u8; CEF_WINDOW_INFO_SIZE],
}

impl CefWindowInfo {
    fn windowless(width: u32, height: u32) -> Self {
        let mut info = Self {
            bytes: [0; CEF_WINDOW_INFO_SIZE],
        };
        info.put_usize(0, CEF_WINDOW_INFO_SIZE);
        info.put_i32(
            CEF_WINDOW_INFO_BOUNDS_OFFSET + 8,
            i32::try_from(width).unwrap_or(i32::MAX),
        );
        info.put_i32(
            CEF_WINDOW_INFO_BOUNDS_OFFSET + 12,
            i32::try_from(height).unwrap_or(i32::MAX),
        );
        info.put_i32(CEF_WINDOW_INFO_WINDOWLESS_OFFSET, 1);
        info.put_i32(CEF_WINDOW_INFO_SHARED_TEXTURE_OFFSET, 0);
        info.put_i32(CEF_WINDOW_INFO_EXTERNAL_BEGIN_FRAME_OFFSET, 0);
        info.put_i32(
            CEF_WINDOW_INFO_RUNTIME_STYLE_OFFSET,
            CEF_RUNTIME_STYLE_ALLOY,
        );
        info
    }

    fn as_ptr(&self) -> *const c_void {
        self.bytes.as_ptr().cast()
    }

    fn put_usize(&mut self, offset: usize, value: usize) {
        self.bytes[offset..offset + std::mem::size_of::<usize>()]
            .copy_from_slice(&value.to_ne_bytes());
    }

    fn put_i32(&mut self, offset: usize, value: i32) {
        self.bytes[offset..offset + std::mem::size_of::<i32>()]
            .copy_from_slice(&value.to_ne_bytes());
    }
}

struct CefBrowserSettings {
    bytes: [u8; CEF_BROWSER_SETTINGS_SIZE],
}

impl CefBrowserSettings {
    fn windowless(frame_rate: i32) -> Self {
        let mut settings = Self {
            bytes: [0; CEF_BROWSER_SETTINGS_SIZE],
        };
        settings.put_usize(0, CEF_BROWSER_SETTINGS_SIZE);
        settings.put_i32(CEF_BROWSER_SETTINGS_FRAME_RATE_OFFSET, frame_rate);
        settings.put_u32(CEF_BROWSER_SETTINGS_BACKGROUND_COLOR_OFFSET, 0xFFFF_FFFF);
        settings
    }

    fn as_ptr(&self) -> *const c_void {
        self.bytes.as_ptr().cast()
    }

    fn put_usize(&mut self, offset: usize, value: usize) {
        self.bytes[offset..offset + std::mem::size_of::<usize>()]
            .copy_from_slice(&value.to_ne_bytes());
    }

    fn put_i32(&mut self, offset: usize, value: i32) {
        self.bytes[offset..offset + std::mem::size_of::<i32>()]
            .copy_from_slice(&value.to_ne_bytes());
    }

    fn put_u32(&mut self, offset: usize, value: u32) {
        self.bytes[offset..offset + std::mem::size_of::<u32>()]
            .copy_from_slice(&value.to_ne_bytes());
    }
}

struct CefStringOwned {
    text: String,
    _data: Vec<u16>,
    raw: CefString,
}

impl CefStringOwned {
    fn new(text: &str) -> Result<Self, CefBrowserError> {
        if text.encode_utf16().any(|unit| unit == 0) {
            return Err(CefBrowserError::BadUrl);
        }
        let data = text.encode_utf16().collect::<Vec<_>>();
        let raw = CefString {
            str_: data.as_ptr(),
            length: data.len(),
            dtor: 0,
        };
        Ok(Self {
            text: text.to_owned(),
            _data: data,
            raw,
        })
    }

    fn as_ptr(&self) -> *const c_void {
        (&self.raw as *const CefString).cast()
    }

    fn text(&self) -> &str {
        &self.text
    }
}

#[repr(C)]
struct CefString {
    str_: *const u16,
    length: usize,
    dtor: usize,
}

struct CefBrowserCallbacks {
    state: Box<CefBrowserState>,
    client: Box<CefCallbackBlock<CEF_CLIENT_SIZE>>,
    life_span: Box<CefCallbackBlock<CEF_LIFE_SPAN_HANDLER_SIZE>>,
    render: Box<CefCallbackBlock<CEF_RENDER_HANDLER_SIZE>>,
    request: Box<CefCallbackBlock<CEF_REQUEST_HANDLER_SIZE>>,
    resource_request: Box<CefCallbackBlock<CEF_RESOURCE_REQUEST_HANDLER_SIZE>>,
    print: Box<CefCallbackBlock<CEF_PRINT_HANDLER_SIZE>>,
}

impl CefBrowserCallbacks {
    fn new(
        width: u32,
        height: u32,
        stream: Option<&UnixStream>,
        string_userfree_free: CefStringUserfreeUtf16Free,
    ) -> Result<Self, CefBrowserError> {
        let state = Box::new(CefBrowserState::new(
            width,
            height,
            stream,
            string_userfree_free,
        )?);
        let mut callbacks = Self {
            state,
            client: Box::new(CefCallbackBlock::new(CEF_CLIENT_SIZE)),
            life_span: Box::new(CefCallbackBlock::new(CEF_LIFE_SPAN_HANDLER_SIZE)),
            render: Box::new(CefCallbackBlock::new(CEF_RENDER_HANDLER_SIZE)),
            request: Box::new(CefCallbackBlock::new(CEF_REQUEST_HANDLER_SIZE)),
            resource_request: Box::new(CefCallbackBlock::new(CEF_RESOURCE_REQUEST_HANDLER_SIZE)),
            print: Box::new(CefCallbackBlock::new(CEF_PRINT_HANDLER_SIZE)),
        };
        callbacks.install();
        Ok(callbacks)
    }

    fn install(&mut self) {
        self.client.put_fn(
            CEF_CLIENT_GET_LIFE_SPAN_HANDLER_OFFSET,
            fn_ptr(get_life_span_handler as *const ()),
        );
        self.client.put_fn(
            CEF_CLIENT_GET_RENDER_HANDLER_OFFSET,
            fn_ptr(get_render_handler as *const ()),
        );
        self.client.put_fn(
            CEF_CLIENT_GET_REQUEST_HANDLER_OFFSET,
            fn_ptr(get_request_handler as *const ()),
        );
        self.client.put_fn(
            CEF_CLIENT_GET_PRINT_HANDLER_OFFSET,
            fn_ptr(get_print_handler as *const ()),
        );
        self.life_span.put_fn(
            CEF_LIFE_SPAN_ON_AFTER_CREATED_OFFSET,
            fn_ptr(on_after_created as *const ()),
        );
        self.render.put_fn(
            CEF_RENDER_HANDLER_GET_VIEW_RECT_OFFSET,
            fn_ptr(get_view_rect as *const ()),
        );
        self.render.put_fn(
            CEF_RENDER_HANDLER_ON_PAINT_OFFSET,
            fn_ptr(on_paint as *const ()),
        );
        self.request.put_fn(
            CEF_REQUEST_HANDLER_GET_RESOURCE_REQUEST_HANDLER_OFFSET,
            fn_ptr(get_resource_request_handler as *const ()),
        );
        self.resource_request.put_fn(
            CEF_RESOURCE_REQUEST_HANDLER_ON_BEFORE_RESOURCE_LOAD_OFFSET,
            fn_ptr(on_before_resource_load as *const ()),
        );
        self.print.put_fn(
            CEF_PRINT_HANDLER_ON_PRINT_DIALOG_OFFSET,
            fn_ptr(on_print_dialog as *const ()),
        );
        self.print.put_fn(
            CEF_PRINT_HANDLER_ON_PRINT_JOB_OFFSET,
            fn_ptr(on_print_job as *const ()),
        );
        self.print.put_fn(
            CEF_PRINT_HANDLER_GET_PDF_PAPER_SIZE_OFFSET,
            fn_ptr(get_pdf_paper_size as *const ()),
        );

        let state = self.state.as_ref() as *const CefBrowserState as usize;
        self.state
            .print_handler_ptr
            .store(self.print.as_usize(), Ordering::SeqCst);
        let mut registry = registry().lock().expect("cef callback registry");
        registry.insert(self.client.as_usize(), state);
        registry.insert(self.life_span.as_usize(), state);
        registry.insert(self.render.as_usize(), state);
        registry.insert(self.request.as_usize(), state);
        registry.insert(self.resource_request.as_usize(), state);
        registry.insert(self.print.as_usize(), state);
    }

    fn client_ptr(&self) -> *mut c_void {
        self.client.as_mut_ptr()
    }

    fn created(&self) -> usize {
        self.state.created.load(Ordering::SeqCst)
    }

    fn paints(&self) -> usize {
        self.state.paints.load(Ordering::SeqCst)
    }

    fn navigations(&self) -> u64 {
        self.state.navigations()
    }

    fn last_paint_width(&self) -> i32 {
        self.state.last_paint_width.load(Ordering::SeqCst) as i32
    }

    fn last_paint_height(&self) -> i32 {
        self.state.last_paint_height.load(Ordering::SeqCst) as i32
    }

    fn resize(&self, width: u32, height: u32) {
        self.state.resize(width, height);
    }

    fn update_pointer(&self, x: f32, y: f32) -> (i32, i32) {
        self.state.update_pointer(x, y)
    }

    fn pointer_position(&self) -> (i32, i32) {
        self.state.pointer_position()
    }

    fn apply_resource_verdict(&self, id: u64, allow: bool) {
        self.state.apply_resource_verdict(id, allow);
    }

    fn retain_pdf_callback(&self) -> *mut c_void {
        self.state.retain_pdf_callback()
    }
}

impl Drop for CefBrowserCallbacks {
    fn drop(&mut self) {
        self.state.cancel_pending_resource_requests();
        let mut registry = registry().lock().expect("cef callback registry");
        registry.remove(&self.client.as_usize());
        registry.remove(&self.life_span.as_usize());
        registry.remove(&self.render.as_usize());
        registry.remove(&self.request.as_usize());
        registry.remove(&self.resource_request.as_usize());
        registry.remove(&self.print.as_usize());
        if let Ok(callbacks) = self.state.pdf_callbacks.lock() {
            for callback in callbacks.iter() {
                registry.remove(&callback.as_usize());
            }
        }
    }
}

struct CefBrowserState {
    width: AtomicI32,
    height: AtomicI32,
    created: AtomicUsize,
    paints: AtomicUsize,
    /// Monotonic count of main/sub-frame navigations, bumped from the resource
    /// handler's `is_navigation` flag (browser-8). Drives per-context shim
    /// re-injection without a wall-clock timer.
    nav_seq: AtomicU64,
    last_paint_width: AtomicUsize,
    last_paint_height: AtomicUsize,
    pointer_x: AtomicI32,
    pointer_y: AtomicI32,
    frame_sink: Mutex<Option<BrowserFrameSink>>,
    next_resource_request_id: AtomicU64,
    pending_resource_requests: Mutex<HashMap<u64, usize>>,
    pdf_callbacks: Mutex<Vec<Box<CefCallbackBlock<CEF_PDF_PRINT_CALLBACK_SIZE>>>>,
    print_handler_ptr: AtomicUsize,
    string_userfree_free: CefStringUserfreeUtf16Free,
}

impl CefBrowserState {
    fn new(
        width: u32,
        height: u32,
        stream: Option<&UnixStream>,
        string_userfree_free: CefStringUserfreeUtf16Free,
    ) -> Result<Self, CefBrowserError> {
        let frame_sink = stream
            .map(|stream| {
                let helper_stream = stream.try_clone()?;
                let sink = OffscreenFrameSink::attach(&helper_stream, width, height)?;
                Ok::<_, CefBrowserError>(BrowserFrameSink {
                    stream: helper_stream,
                    sink,
                })
            })
            .transpose()?;
        Ok(Self {
            width: AtomicI32::new(i32::try_from(width).unwrap_or(i32::MAX)),
            height: AtomicI32::new(i32::try_from(height).unwrap_or(i32::MAX)),
            created: AtomicUsize::new(0),
            paints: AtomicUsize::new(0),
            nav_seq: AtomicU64::new(0),
            last_paint_width: AtomicUsize::new(0),
            last_paint_height: AtomicUsize::new(0),
            pointer_x: AtomicI32::new(0),
            pointer_y: AtomicI32::new(0),
            frame_sink: Mutex::new(frame_sink),
            next_resource_request_id: AtomicU64::new(1),
            pending_resource_requests: Mutex::new(HashMap::new()),
            pdf_callbacks: Mutex::new(Vec::new()),
            print_handler_ptr: AtomicUsize::new(0),
            string_userfree_free,
        })
    }

    fn resize(&self, width: u32, height: u32) {
        self.width
            .store(i32::try_from(width).unwrap_or(i32::MAX), Ordering::SeqCst);
        self.height
            .store(i32::try_from(height).unwrap_or(i32::MAX), Ordering::SeqCst);
    }

    /// Record that a fresh document context is being loaded (browser-8). Called
    /// from the resource handler when CEF flags a request as a navigation.
    fn record_navigation(&self) {
        self.nav_seq.fetch_add(1, Ordering::SeqCst);
    }

    /// Current navigation generation observed by the pump loop.
    fn navigations(&self) -> u64 {
        self.nav_seq.load(Ordering::SeqCst)
    }

    fn update_pointer(&self, x: f32, y: f32) -> (i32, i32) {
        let x = f32_to_i32(x);
        let y = f32_to_i32(y);
        self.pointer_x.store(x, Ordering::SeqCst);
        self.pointer_y.store(y, Ordering::SeqCst);
        (x, y)
    }

    fn pointer_position(&self) -> (i32, i32) {
        (
            self.pointer_x.load(Ordering::SeqCst),
            self.pointer_y.load(Ordering::SeqCst),
        )
    }

    fn begin_resource_request(&self, url: String, callback: *mut c_void) -> c_int {
        if let Some((id, text)) = decode_page_text_beacon(&url) {
            self.publish_page_text(id, text);
            if !callback.is_null() {
                cancel_cef_callback(callback);
            }
            return RV_CANCEL;
        }
        if let Some((id, body)) = decode_page_scrape_beacon(&url) {
            self.publish_page_scrape(id, body);
            if !callback.is_null() {
                cancel_cef_callback(callback);
            }
            return RV_CANCEL;
        }
        if let Some(body) = decode_passkey_beacon(&url) {
            self.publish_passkey_request(body);
            if !callback.is_null() {
                cancel_cef_callback(callback);
            }
            return RV_CANCEL;
        }
        if callback.is_null() {
            return RV_CONTINUE;
        }
        let Some(id) = self
            .next_resource_request_id
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |id| id.checked_add(1))
            .ok()
        else {
            return RV_CONTINUE;
        };
        add_ref_cef(callback);
        let event = EventMsg::ResourceRequest {
            id,
            url,
            resource: RESOURCE_OTHER,
        };
        let sent = self
            .frame_sink
            .lock()
            .ok()
            .and_then(|guard| {
                guard.as_ref().and_then(|frame_sink| {
                    sock::send_frame(&frame_sink.stream, &event.encode()).ok()
                })
            })
            .is_some();
        if sent {
            if let Ok(mut pending) = self.pending_resource_requests.lock() {
                pending.insert(id, callback as usize);
                return RV_CONTINUE_ASYNC;
            }
        }
        cancel_cef_callback(callback);
        release_cef(callback);
        RV_CANCEL
    }

    fn apply_resource_verdict(&self, id: u64, allow: bool) {
        let callback = self
            .pending_resource_requests
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&id))
            .map(|ptr| ptr as *mut c_void);
        if let Some(callback) = callback {
            if allow {
                continue_cef_callback(callback);
            } else {
                cancel_cef_callback(callback);
            }
            release_cef(callback);
        }
    }

    fn retain_pdf_callback(&self) -> *mut c_void {
        let mut callback = Box::new(CefCallbackBlock::new(CEF_PDF_PRINT_CALLBACK_SIZE));
        callback.put_fn(
            CEF_PDF_PRINT_CALLBACK_ON_FINISHED_OFFSET,
            fn_ptr(on_pdf_print_finished as *const ()),
        );
        let ptr = callback.as_mut_ptr();
        if let Ok(mut callbacks) = self.pdf_callbacks.lock() {
            let state = self as *const CefBrowserState as usize;
            if let Ok(mut registry) = registry().lock() {
                registry.insert(callback.as_usize(), state);
            }
            callbacks.push(callback);
            ptr
        } else {
            ptr::null_mut()
        }
    }

    fn publish_pdf_finished(&self, path: String, ok: bool) {
        let ok = ok && pdf_file_looks_written(&path);
        let event = EventMsg::PdfSaved { path, ok };
        let _ = self.frame_sink.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .and_then(|frame_sink| sock::send_frame(&frame_sink.stream, &event.encode()).ok())
        });
    }

    fn publish_page_text(&self, id: u64, text: String) {
        let event = EventMsg::PageText { id, text };
        let _ = self.frame_sink.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .and_then(|frame_sink| sock::send_frame(&frame_sink.stream, &event.encode()).ok())
        });
    }

    fn publish_page_scrape(&self, id: u64, body: String) {
        let event = EventMsg::PageScrape { id, body };
        let _ = self.frame_sink.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .and_then(|frame_sink| sock::send_frame(&frame_sink.stream, &event.encode()).ok())
        });
    }

    fn publish_passkey_request(&self, body: String) {
        let event = EventMsg::PasskeyRequest { body };
        let _ = self.frame_sink.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .and_then(|frame_sink| sock::send_frame(&frame_sink.stream, &event.encode()).ok())
        });
    }

    fn cancel_pending_resource_requests(&self) {
        let callbacks = self
            .pending_resource_requests
            .lock()
            .map(|mut pending| pending.drain().map(|(_, ptr)| ptr).collect::<Vec<_>>())
            .unwrap_or_default();
        for callback in callbacks {
            let callback = callback as *mut c_void;
            cancel_cef_callback(callback);
            release_cef(callback);
        }
    }
}

struct CefMouseEvent {
    bytes: [u8; CEF_MOUSE_EVENT_SIZE],
}

impl CefMouseEvent {
    fn new(x: i32, y: i32, modifiers: c_int) -> Self {
        let mut event = Self {
            bytes: [0; CEF_MOUSE_EVENT_SIZE],
        };
        event.put_i32(CEF_MOUSE_EVENT_X_OFFSET, x);
        event.put_i32(CEF_MOUSE_EVENT_Y_OFFSET, y);
        event.put_i32(CEF_MOUSE_EVENT_MODIFIERS_OFFSET, modifiers);
        event
    }

    fn as_ptr(&self) -> *const c_void {
        self.bytes.as_ptr().cast()
    }

    fn put_i32(&mut self, offset: usize, value: i32) {
        self.bytes[offset..offset + std::mem::size_of::<i32>()]
            .copy_from_slice(&value.to_ne_bytes());
    }
}

struct CefKeyEvent {
    bytes: [u8; CEF_KEY_EVENT_SIZE],
}

impl CefKeyEvent {
    fn new(
        event_type: c_int,
        modifiers: c_int,
        windows_key_code: i32,
        native_key_code: i32,
        character: u16,
        unmodified_character: u16,
    ) -> Self {
        let mut event = Self {
            bytes: [0; CEF_KEY_EVENT_SIZE],
        };
        event.put_i32(CEF_KEY_EVENT_TYPE_OFFSET, event_type);
        event.put_i32(CEF_KEY_EVENT_MODIFIERS_OFFSET, modifiers);
        event.put_i32(CEF_KEY_EVENT_WINDOWS_KEY_CODE_OFFSET, windows_key_code);
        event.put_i32(CEF_KEY_EVENT_NATIVE_KEY_CODE_OFFSET, native_key_code);
        event.put_i32(CEF_KEY_EVENT_IS_SYSTEM_KEY_OFFSET, 0);
        event.put_u16(CEF_KEY_EVENT_CHARACTER_OFFSET, character);
        event.put_u16(
            CEF_KEY_EVENT_UNMODIFIED_CHARACTER_OFFSET,
            unmodified_character,
        );
        // Offscreen CEF does not get native toolkit focus metadata from a window.
        // The shell sends keyboard/text only after the page canvas owns focus, so
        // mark the event as editable-focused for Chromium's text-input path.
        event.put_i32(CEF_KEY_EVENT_FOCUS_ON_EDITABLE_FIELD_OFFSET, 1);
        event
    }

    fn as_ptr(&self) -> *const c_void {
        self.bytes.as_ptr().cast()
    }

    fn put_i32(&mut self, offset: usize, value: i32) {
        self.bytes[offset..offset + std::mem::size_of::<i32>()]
            .copy_from_slice(&value.to_ne_bytes());
    }

    fn put_u16(&mut self, offset: usize, value: u16) {
        self.bytes[offset..offset + std::mem::size_of::<u16>()]
            .copy_from_slice(&value.to_ne_bytes());
    }
}

struct BrowserFrameSink {
    stream: UnixStream,
    sink: OffscreenFrameSink,
}

#[repr(C, align(8))]
struct CefCallbackBlock<const N: usize> {
    bytes: [u8; N],
}

impl<const N: usize> CefCallbackBlock<N> {
    fn new(size: usize) -> Self {
        let mut block = Self { bytes: [0; N] };
        block.put_usize(BASE_SIZE_OFFSET, size);
        block.put_fn(BASE_ADD_REF_OFFSET, fn_ptr(add_ref as *const ()));
        block.put_fn(BASE_RELEASE_OFFSET, fn_ptr(release as *const ()));
        block.put_fn(BASE_HAS_ONE_REF_OFFSET, fn_ptr(has_one_ref as *const ()));
        block.put_fn(
            BASE_HAS_AT_LEAST_ONE_REF_OFFSET,
            fn_ptr(has_at_least_one_ref as *const ()),
        );
        block
    }

    fn as_mut_ptr(&self) -> *mut c_void {
        self.bytes.as_ptr().cast_mut().cast()
    }

    fn as_usize(&self) -> usize {
        self.as_mut_ptr() as usize
    }

    fn put_usize(&mut self, offset: usize, value: usize) {
        self.bytes[offset..offset + std::mem::size_of::<usize>()]
            .copy_from_slice(&value.to_ne_bytes());
    }

    fn put_fn(&mut self, offset: usize, value: usize) {
        self.put_usize(offset, value);
    }
}

fn fn_ptr(ptr: *const ()) -> usize {
    ptr as usize
}

#[repr(C)]
struct CefRect {
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CefSize {
    width: c_int,
    height: c_int,
}

unsafe extern "C" fn add_ref(_self: *mut c_void) {}

unsafe extern "C" fn release(_self: *mut c_void) -> c_int {
    0
}

unsafe extern "C" fn has_one_ref(_self: *mut c_void) -> c_int {
    0
}

unsafe extern "C" fn has_at_least_one_ref(_self: *mut c_void) -> c_int {
    1
}

unsafe extern "C" fn get_life_span_handler(self_: *mut c_void) -> *mut c_void {
    with_state(self_, |state| state.life_span_ptr()).unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn get_render_handler(self_: *mut c_void) -> *mut c_void {
    with_state(self_, |state| state.render_ptr()).unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn get_request_handler(self_: *mut c_void) -> *mut c_void {
    with_state(self_, |state| state.request_ptr()).unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn get_print_handler(self_: *mut c_void) -> *mut c_void {
    with_state(self_, |state| state.print_ptr()).unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn get_resource_request_handler(
    self_: *mut c_void,
    _browser: *mut c_void,
    _frame: *mut c_void,
    _request: *mut c_void,
    is_navigation: c_int,
    _is_download: c_int,
    _request_initiator: *const c_void,
    disable_default_handling: *mut c_int,
) -> *mut c_void {
    if !disable_default_handling.is_null() {
        // SAFETY: CEF supplied this out-parameter for the callback duration.
        unsafe {
            *disable_default_handling = 0;
        }
    }
    with_state(self_, |state| {
        if is_navigation != 0 {
            // browser-8: a real navigation opens a fresh JS context that needs
            // the WebRTC/passkey shims re-injected. Signal the pump loop instead
            // of re-running the shims on a blind 250 ms timer.
            state.record_navigation();
        }
        state.resource_request_ptr()
    })
    .unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn on_after_created(self_: *mut c_void, _browser: *mut c_void) {
    let _ = with_state(self_, |state| {
        state.created.fetch_add(1, Ordering::SeqCst);
    });
}

unsafe extern "C" fn get_view_rect(self_: *mut c_void, _browser: *mut c_void, rect: *mut CefRect) {
    if rect.is_null() {
        return;
    }
    let _ = with_state(self_, |state| {
        // SAFETY: CEF supplied a non-null output pointer for this callback.
        unsafe {
            (*rect).x = 0;
            (*rect).y = 0;
            (*rect).width = state.width.load(Ordering::SeqCst);
            (*rect).height = state.height.load(Ordering::SeqCst);
        }
    });
}

unsafe extern "C" fn on_paint(
    self_: *mut c_void,
    _browser: *mut c_void,
    paint_type: c_int,
    _dirty_rects_count: usize,
    _dirty_rects: *const CefRect,
    buffer: *const c_void,
    width: c_int,
    height: c_int,
) {
    if paint_type != PET_VIEW || buffer.is_null() || width <= 0 || height <= 0 {
        return;
    }
    let _ = with_state(self_, |state| {
        state.paints.fetch_add(1, Ordering::SeqCst);
        state
            .last_paint_width
            .store(usize::try_from(width).unwrap_or(0), Ordering::SeqCst);
        state
            .last_paint_height
            .store(usize::try_from(height).unwrap_or(0), Ordering::SeqCst);
        if let Ok(mut guard) = state.frame_sink.lock() {
            if let Some(frame_sink) = guard.as_mut() {
                let len = (width as usize)
                    .saturating_mul(height as usize)
                    .saturating_mul(4);
                // SAFETY: CEF documents `buffer` as `width * height * 4` bytes for
                // `PET_VIEW` BGRA paints and the pointer was checked non-null.
                let pixels = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), len) };
                let _ = frame_sink.sink.publish_bgra(
                    &frame_sink.stream,
                    u32::try_from(width).unwrap_or(0),
                    u32::try_from(height).unwrap_or(0),
                    pixels,
                );
            }
        }
    });
}

unsafe extern "C" fn on_before_resource_load(
    self_: *mut c_void,
    _browser: *mut c_void,
    _frame: *mut c_void,
    request: *mut c_void,
    callback: *mut c_void,
) -> c_int {
    with_state(self_, |state| {
        let Some(url) = request_url(request, state.string_userfree_free) else {
            return RV_CONTINUE;
        };
        state.begin_resource_request(url, callback)
    })
    .unwrap_or(RV_CONTINUE)
}

unsafe extern "C" fn on_print_dialog(
    _self: *mut c_void,
    _browser: *mut c_void,
    _has_selection: c_int,
    _callback: *mut c_void,
) -> c_int {
    0
}

unsafe extern "C" fn on_print_job(
    _self: *mut c_void,
    _browser: *mut c_void,
    _document_name: *const c_void,
    _pdf_file_path: *const c_void,
    _callback: *mut c_void,
) -> c_int {
    0
}

unsafe extern "C" fn get_pdf_paper_size(
    _self: *mut c_void,
    _browser: *mut c_void,
    device_units_per_inch: c_int,
) -> CefSize {
    let dpi = device_units_per_inch.max(1);
    CefSize {
        width: dpi.saturating_mul(85) / 10,
        height: dpi.saturating_mul(11),
    }
}

fn pdf_file_looks_written(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).is_ok() && magic == *b"%PDF"
}

unsafe extern "C" fn on_pdf_print_finished(self_: *mut c_void, path: *const c_void, ok: c_int) {
    let path = cef_string_to_string(path.cast::<CefString>());
    let _ = with_state(self_, |state| state.publish_pdf_finished(path, ok != 0));
}

fn with_state<T>(key: *mut c_void, f: impl FnOnce(&CefBrowserState) -> T) -> Option<T> {
    let state_ptr = {
        let registry = registry().lock().ok()?;
        *registry.get(&(key as usize))?
    };
    let state = state_ptr as *const CefBrowserState;
    if state.is_null() {
        return None;
    }
    // SAFETY: registry entries are installed from a live `CefBrowserCallbacks`
    // and removed only when the callback object drops after CEF shutdown.
    Some(f(unsafe { &*state }))
}

fn registry() -> &'static Mutex<HashMap<usize, usize>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl CefBrowserState {
    fn life_span_ptr(&self) -> *mut c_void {
        lookup_peer(self, CEF_LIFE_SPAN_HANDLER_SIZE)
    }

    fn render_ptr(&self) -> *mut c_void {
        lookup_peer(self, CEF_RENDER_HANDLER_SIZE)
    }

    fn request_ptr(&self) -> *mut c_void {
        lookup_peer(self, CEF_REQUEST_HANDLER_SIZE)
    }

    fn resource_request_ptr(&self) -> *mut c_void {
        lookup_peer(self, CEF_RESOURCE_REQUEST_HANDLER_SIZE)
    }

    fn print_ptr(&self) -> *mut c_void {
        self.print_handler_ptr.load(Ordering::SeqCst) as *mut c_void
    }
}

fn lookup_peer(state: &CefBrowserState, size: usize) -> *mut c_void {
    let state_ptr = state as *const CefBrowserState as usize;
    let registry = registry().lock().expect("cef callback registry");
    registry
        .iter()
        .find_map(|(key, value)| {
            if *value == state_ptr && callback_size(*key) == Some(size) {
                Some(*key as *mut c_void)
            } else {
                None
            }
        })
        .unwrap_or(ptr::null_mut())
}

fn callback_size(key: usize) -> Option<usize> {
    // SAFETY: `key` is a registered callback block pointer. All callback blocks
    // start with a `size_t size` field per `cef_base_ref_counted_t`.
    let size = unsafe { *(key as *const usize) };
    match size {
        CEF_CLIENT_SIZE
        | CEF_LIFE_SPAN_HANDLER_SIZE
        | CEF_RENDER_HANDLER_SIZE
        | CEF_REQUEST_HANDLER_SIZE
        | CEF_RESOURCE_REQUEST_HANDLER_SIZE
        | CEF_PDF_PRINT_CALLBACK_SIZE => Some(size),
        _ => None,
    }
}

fn notify_browser_view_ready(browser: *mut c_void) {
    let Some(host) = browser_host(browser) else {
        return;
    };
    call_host_void(host, CEF_BROWSER_HOST_WAS_RESIZED_OFFSET);
    call_host_paint_type(host, CEF_BROWSER_HOST_INVALIDATE_OFFSET, PET_VIEW);
}

fn close_browser(browser: *mut c_void) {
    let Some(host) = browser_host(browser) else {
        return;
    };
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_CLOSE_BROWSER_OFFSET) else {
        return;
    };
    // SAFETY: `callback` is read from `cef_browser_host_t::close_browser`, whose
    // pinned C signature is `void (*)(cef_browser_host_t*, int)`.
    let callback: unsafe extern "C" fn(*mut c_void, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from `cef_browser_t::get_host`; force close avoids
    // unload-dialog waits during socket teardown.
    unsafe { callback(host, 1) };
}

fn browser_host(browser: *mut c_void) -> Option<*mut c_void> {
    if browser.is_null() {
        return None;
    }
    let get_host = read_fn(browser, CEF_BROWSER_GET_HOST_OFFSET)?;
    // SAFETY: `get_host` is read from a live `cef_browser_t` function slot using
    // the offset verified from the pinned CEF 149 headers.
    let get_host: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(get_host) };
    // SAFETY: CEF returned `browser` from `cef_browser_host_create_browser_sync`.
    let host = unsafe { get_host(browser) };
    (!host.is_null()).then_some(host)
}

fn call_host_void(host: *mut c_void, offset: usize) {
    let Some(callback) = read_fn(host, offset) else {
        return;
    };
    // SAFETY: `callback` is read from a live `cef_browser_host_t` function slot
    // using a header-verified offset.
    let callback: unsafe extern "C" fn(*mut c_void) = unsafe { std::mem::transmute(callback) };
    // SAFETY: CEF returned `host` from `cef_browser_t::get_host`.
    unsafe { callback(host) };
}

fn set_host_focus(host: *mut c_void, focused: bool) {
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_SET_FOCUS_OFFSET) else {
        return;
    };
    // SAFETY: `callback` is read from `cef_browser_host_t::set_focus`, whose
    // pinned C signature is `(cef_browser_host_t*, int)`.
    let callback: unsafe extern "C" fn(*mut c_void, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF and the focus flag is the CEF boolean int.
    unsafe { callback(host, if focused { 1 } else { 0 }) };
}

fn set_audio_muted(browser: *mut c_void, muted: bool) {
    let Some(host) = browser_host(browser) else {
        return;
    };
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_SET_AUDIO_MUTED_OFFSET) else {
        return;
    };
    // SAFETY: `callback` is read from `cef_browser_host_t::set_audio_muted`,
    // whose pinned C signature is `(cef_browser_host_t*, int)`.
    let callback: unsafe extern "C" fn(*mut c_void, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF and the mute flag is the CEF boolean int.
    unsafe { callback(host, if muted { 1 } else { 0 }) };
}

fn print_page(browser: *mut c_void) {
    let Some(host) = browser_host(browser) else {
        return;
    };
    call_host_void(host, CEF_BROWSER_HOST_PRINT_OFFSET);
}

fn save_pdf(browser: *mut c_void, callbacks: &CefBrowserCallbacks, path: &str) {
    let Some(host) = browser_host(browser) else {
        return;
    };
    let Ok(path) = CefStringOwned::new(path) else {
        return;
    };
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_PRINT_TO_PDF_OFFSET) else {
        return;
    };
    let pdf_callback = callbacks.retain_pdf_callback();
    if pdf_callback.is_null() {
        return;
    }
    // SAFETY: `callback` is read from `cef_browser_host_t::print_to_pdf`, whose
    // pinned C signature is `(cef_browser_host_t*, const cef_string_t*,
    // const cef_pdf_print_settings_t*, cef_pdf_print_callback_t*)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void, *const c_void, *mut c_void) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF, `path` points to a live CefString for this
    // call, null settings asks CEF for defaults, and the callback is retained by
    // the browser state until shutdown.
    unsafe { callback(host, path.as_ptr(), ptr::null(), pdf_callback) };
}

fn call_host_paint_type(host: *mut c_void, offset: usize, paint_type: c_int) {
    let Some(callback) = read_fn(host, offset) else {
        return;
    };
    // SAFETY: `callback` is read from a live `cef_browser_host_t` function slot
    // using a header-verified offset.
    let callback: unsafe extern "C" fn(*mut c_void, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: CEF returned `host` from `cef_browser_t::get_host`; `PET_VIEW` is a
    // valid `cef_paint_element_type_t`.
    unsafe { callback(host, paint_type) };
}

fn call_browser_void(browser: *mut c_void, offset: usize) {
    let Some(callback) = read_fn(browser, offset) else {
        return;
    };
    // SAFETY: `callback` is read from a live `cef_browser_t` void method slot
    // using a header-verified offset.
    let callback: unsafe extern "C" fn(*mut c_void) = unsafe { std::mem::transmute(callback) };
    // SAFETY: CEF returned `browser` from `cef_browser_host_create_browser_sync`.
    unsafe { callback(browser) };
}

fn call_browser_bool(browser: *mut c_void, offset: usize) -> bool {
    let Some(callback) = read_fn(browser, offset) else {
        return false;
    };
    // SAFETY: `callback` is read from a live `cef_browser_t` int-returning method
    // slot using a header-verified offset.
    let callback: unsafe extern "C" fn(*mut c_void) -> c_int =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: CEF returned `browser` from `cef_browser_host_create_browser_sync`.
    unsafe { callback(browser) != 0 }
}

fn send_mouse_move(host: *mut c_void, x: i32, y: i32, modifiers: c_int, mouse_leave: bool) {
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_SEND_MOUSE_MOVE_EVENT_OFFSET) else {
        return;
    };
    let event = CefMouseEvent::new(x, y, modifiers);
    // SAFETY: `callback` is read from `cef_browser_host_t::send_mouse_move_event`,
    // whose pinned C signature is `(cef_browser_host_t*, const cef_mouse_event_t*, int)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF and `event` lives for the duration of the call.
    unsafe { callback(host, event.as_ptr(), if mouse_leave { 1 } else { 0 }) };
}

fn send_mouse_click(host: *mut c_void, x: i32, y: i32, button: PointerButton, pressed: bool) {
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_SEND_MOUSE_CLICK_EVENT_OFFSET) else {
        return;
    };
    let event = CefMouseEvent::new(x, y, mouse_button_event_flag(button));
    // SAFETY: `callback` is read from `cef_browser_host_t::send_mouse_click_event`,
    // whose pinned C signature is `(cef_browser_host_t*, const cef_mouse_event_t*,
    // cef_mouse_button_type_t, int, int)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void, c_int, c_int, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF and `event` lives for the duration of the call.
    unsafe {
        callback(
            host,
            event.as_ptr(),
            cef_mouse_button(button),
            if pressed { 0 } else { 1 },
            1,
        )
    };
}

fn send_mouse_wheel(host: *mut c_void, x: i32, y: i32, delta_x: f32, delta_y: f32) {
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_SEND_MOUSE_WHEEL_EVENT_OFFSET) else {
        return;
    };
    let event = CefMouseEvent::new(x, y, 0);
    // SAFETY: `callback` is read from `cef_browser_host_t::send_mouse_wheel_event`,
    // whose pinned C signature is `(cef_browser_host_t*, const cef_mouse_event_t*, int, int)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void, c_int, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF and `event` lives for the duration of the call.
    unsafe {
        callback(
            host,
            event.as_ptr(),
            f32_to_i32(delta_x),
            f32_to_i32(delta_y),
        )
    };
}

fn send_key(host: *mut c_void, key: KeyCode, pressed: bool, modifiers: Modifiers) {
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_SEND_KEY_EVENT_OFFSET) else {
        return;
    };
    let Some(code) = windows_key_code(key) else {
        return;
    };
    let event = CefKeyEvent::new(
        if pressed {
            KEYEVENT_RAWKEYDOWN
        } else {
            KEYEVENT_KEYUP
        },
        cef_modifiers(modifiers),
        code,
        code,
        0,
        0,
    );
    // SAFETY: `callback` is read from `cef_browser_host_t::send_key_event`,
    // whose pinned C signature is `(cef_browser_host_t*, const cef_key_event_t*)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF and `event` lives for the duration of the call.
    unsafe { callback(host, event.as_ptr()) };
}

fn send_char(host: *mut c_void, character: u16, modifiers: Modifiers) {
    let Some(callback) = read_fn(host, CEF_BROWSER_HOST_SEND_KEY_EVENT_OFFSET) else {
        return;
    };
    let event = CefKeyEvent::new(
        KEYEVENT_CHAR,
        cef_modifiers(modifiers),
        i32::from(character),
        i32::from(character),
        character,
        character,
    );
    // SAFETY: `callback` is read from `cef_browser_host_t::send_key_event`,
    // whose pinned C signature is `(cef_browser_host_t*, const cef_key_event_t*)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `host` came from CEF and `event` lives for the duration of the call.
    unsafe { callback(host, event.as_ptr()) };
}

fn cef_mouse_button(button: PointerButton) -> c_int {
    match button {
        PointerButton::Primary => MBT_LEFT,
        PointerButton::Secondary => MBT_RIGHT,
        PointerButton::Middle => MBT_MIDDLE,
    }
}

fn mouse_button_event_flag(button: PointerButton) -> c_int {
    match button {
        PointerButton::Primary => EVENTFLAG_LEFT_MOUSE_BUTTON,
        PointerButton::Secondary => EVENTFLAG_RIGHT_MOUSE_BUTTON,
        PointerButton::Middle => EVENTFLAG_MIDDLE_MOUSE_BUTTON,
    }
}

fn cef_modifiers(modifiers: Modifiers) -> c_int {
    let mut flags = 0;
    if modifiers.has(Modifiers::SHIFT) {
        flags |= EVENTFLAG_SHIFT_DOWN;
    }
    if modifiers.has(Modifiers::CTRL) {
        flags |= EVENTFLAG_CONTROL_DOWN;
    }
    if modifiers.has(Modifiers::ALT) {
        flags |= EVENTFLAG_ALT_DOWN;
    }
    if modifiers.has(Modifiers::COMMAND) {
        flags |= EVENTFLAG_COMMAND_DOWN;
    }
    flags
}

fn windows_key_code(key: KeyCode) -> Option<i32> {
    let code = match key {
        KeyCode::Enter => 13,
        KeyCode::Escape => 27,
        KeyCode::Backspace => 8,
        KeyCode::Tab => 9,
        KeyCode::Space => 32,
        KeyCode::Delete => 46,
        KeyCode::Insert => 45,
        KeyCode::Home => 36,
        KeyCode::End => 35,
        KeyCode::PageUp => 33,
        KeyCode::PageDown => 34,
        KeyCode::ArrowUp => 38,
        KeyCode::ArrowDown => 40,
        KeyCode::ArrowLeft => 37,
        KeyCode::ArrowRight => 39,
        KeyCode::A => 65,
        KeyCode::B => 66,
        KeyCode::C => 67,
        KeyCode::D => 68,
        KeyCode::E => 69,
        KeyCode::F => 70,
        KeyCode::G => 71,
        KeyCode::H => 72,
        KeyCode::I => 73,
        KeyCode::J => 74,
        KeyCode::K => 75,
        KeyCode::L => 76,
        KeyCode::M => 77,
        KeyCode::N => 78,
        KeyCode::O => 79,
        KeyCode::P => 80,
        KeyCode::Q => 81,
        KeyCode::R => 82,
        KeyCode::S => 83,
        KeyCode::T => 84,
        KeyCode::U => 85,
        KeyCode::V => 86,
        KeyCode::W => 87,
        KeyCode::X => 88,
        KeyCode::Y => 89,
        KeyCode::Z => 90,
        KeyCode::Num0 => 48,
        KeyCode::Num1 => 49,
        KeyCode::Num2 => 50,
        KeyCode::Num3 => 51,
        KeyCode::Num4 => 52,
        KeyCode::Num5 => 53,
        KeyCode::Num6 => 54,
        KeyCode::Num7 => 55,
        KeyCode::Num8 => 56,
        KeyCode::Num9 => 57,
        KeyCode::F1 => 112,
        KeyCode::F2 => 113,
        KeyCode::F3 => 114,
        KeyCode::F4 => 115,
        KeyCode::F5 => 116,
        KeyCode::F6 => 117,
        KeyCode::F7 => 118,
        KeyCode::F8 => 119,
        KeyCode::F9 => 120,
        KeyCode::F10 => 121,
        KeyCode::F11 => 122,
        KeyCode::F12 => 123,
    };
    Some(code)
}

fn f32_to_i32(value: f32) -> i32 {
    if !value.is_finite() {
        0
    } else if value >= i32::MAX as f32 {
        i32::MAX
    } else if value <= i32::MIN as f32 {
        i32::MIN
    } else {
        value.round() as i32
    }
}

fn load_url(browser: *mut c_void, url: &str) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    load_frame_url(frame, url);
}

fn load_frame_url(frame: *mut c_void, url: &str) {
    let Ok(url) = CefStringOwned::new(url) else {
        return;
    };
    let Some(callback) = read_fn(frame, CEF_FRAME_LOAD_URL_OFFSET) else {
        return;
    };
    // SAFETY: `callback` is read from `cef_frame_t::load_url`, whose pinned C
    // signature is `void (*)(cef_frame_t*, const cef_string_t*)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `frame` came from `cef_browser_t::get_main_frame`; `url` owns the
    // UTF-16 buffer for the duration of the call.
    unsafe { callback(frame, url.as_ptr()) };
}

fn apply_cosmetic_filters(browser: *mut c_void, css: &str) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    let script = cosmetic_filter_script(css);
    execute_java_script(frame, &script);
}

fn apply_force_dark(browser: *mut c_void, enabled: bool) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &force_dark_script(enabled));
}

fn apply_reader_mode(browser: *mut c_void, enabled: bool) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &reader_mode_script(enabled));
}

fn apply_user_scripts(browser: *mut c_void, enabled: bool, bundle: &str) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &userscript_library_script(enabled, bundle));
}

fn apply_user_agent(browser: *mut c_void, user_agent: &str) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &user_agent_override_script(user_agent));
}

fn apply_device_profile(
    browser: *mut c_void,
    profile: &str,
    width: u16,
    height: u16,
    scale_percent: u16,
    touch: bool,
) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(
        frame,
        &device_profile_script(profile, width, height, scale_percent, touch),
    );
}

fn apply_spellcheck_highlights(browser: *mut c_void, words: &[String]) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &spellcheck_highlight_script(words));
}

fn apply_spellcheck_correction(browser: *mut c_void, word: &str, replacement: &str) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &spellcheck_correction_script(word, replacement));
}

fn apply_spellcheck_correction_all(browser: *mut c_void, word: &str, replacement: &str) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &spellcheck_correction_all_script(word, replacement));
}

fn apply_spellcheck_correction_at(
    browser: *mut c_void,
    word: &str,
    replacement: &str,
    occurrence: u16,
) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(
        frame,
        &spellcheck_correction_at_script(word, replacement, occurrence),
    );
}

fn request_page_text(browser: *mut c_void, id: u64, max_bytes: u32) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    load_frame_url(
        frame,
        &format!("javascript:{}", page_text_beacon_script(id, max_bytes)),
    );
}

fn request_page_scrape(
    browser: *mut c_void,
    id: u64,
    max_bytes: u32,
    max_links: u16,
    max_headings: u16,
) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    load_frame_url(
        frame,
        &format!(
            "javascript:{}",
            page_scrape_beacon_script(id, max_bytes, max_links, max_headings)
        ),
    );
}

/// Inject the per-context security shims (WebRTC block + passkey bridge) into
/// the current document (browser-8). Called once per navigation generation and
/// through a fresh document's settle window — not on a wall-clock timer.
fn inject_context_shims(browser: *mut c_void) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, webrtc_block_script());
    execute_java_script(frame, &passkey_bridge_script());
}

/// Drain any page-initiated passkey ceremonies queued since the last tick. This
/// is a lightweight heartbeat (it just calls the installed
/// `__mdeBrowserPasskeyDrain` closure) rather than recompiling the multi-KB
/// bridge shim every 250 ms.
fn poll_passkey_drain(browser: *mut c_void) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, passkey_drain_script());
}

fn complete_passkey(browser: *mut c_void, body: &str) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &passkey_complete_script(body));
}

fn apply_page_zoom(browser: *mut c_void, percent: u16) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, &page_zoom_script(percent));
}

fn apply_find_in_page(browser: *mut c_void, query: &str, backwards: bool) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    if query.trim().is_empty() {
        clear_find_in_page(browser);
        return;
    }
    execute_java_script(frame, &find_in_page_script(query, backwards));
}

fn clear_find_in_page(browser: *mut c_void) {
    let Some(frame) = main_frame(browser) else {
        return;
    };
    execute_java_script(frame, clear_find_script());
}

fn execute_java_script(frame: *mut c_void, script: &str) {
    let Ok(script) = CefStringOwned::new(script) else {
        return;
    };
    let Ok(script_url) = CefStringOwned::new("mde://cosmetic-filters") else {
        return;
    };
    let Some(callback) = read_fn(frame, CEF_FRAME_EXECUTE_JAVA_SCRIPT_OFFSET) else {
        return;
    };
    // SAFETY: `callback` is read from `cef_frame_t::execute_java_script`, whose
    // pinned C signature is
    // `void (*)(cef_frame_t*, const cef_string_t*, const cef_string_t*, int)`.
    let callback: unsafe extern "C" fn(*mut c_void, *const c_void, *const c_void, c_int) =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: `frame` came from `cef_browser_t::get_main_frame`; both CEF strings
    // own their UTF-16 buffers for the duration of the call.
    unsafe { callback(frame, script.as_ptr(), script_url.as_ptr(), 1) };
}

fn cosmetic_filter_script(css: &str) -> String {
    let css = js_string_literal(css);
    format!(
        "(function(){{var id='mde-cef-cosmetic-style';var root=document.head||document.documentElement;if(!root)return;var el=document.getElementById(id);if({css}.length===0){{if(el)el.remove();return;}}if(!el){{el=document.createElement('style');el.id=id;root.appendChild(el);}}el.textContent={css};}})();"
    )
}

fn force_dark_script(enabled: bool) -> String {
    if !enabled {
        return "(function(){var id='mde-cef-force-dark-style';var el=document.getElementById(id);if(el)el.remove();document.documentElement.style.colorScheme='';})();".to_owned();
    }
    let css = r#"
:root { color-scheme: dark !important; background: #0f1419 !important; }
html, body { background: #0f1419 !important; color: #f2f4f8 !important; }
body, main, article, section, nav, aside, header, footer, div {
  background-color: color-mix(in srgb, currentColor 0%, #0f1419 100%) !important;
}
p, span, li, td, th, label, input, textarea, select, button, a, h1, h2, h3, h4, h5, h6 {
  color: #f2f4f8 !important;
}
a { color: #78a9ff !important; }
img, video, canvas, picture, svg, iframe { filter: none !important; }
input, textarea, select, button { background: #202830 !important; border-color: #525c66 !important; }
"#;
    let css = js_string_literal(css);
    format!(
        "(function(){{var id='mde-cef-force-dark-style';var root=document.head||document.documentElement;if(!root)return;var el=document.getElementById(id);if(!el){{el=document.createElement('style');el.id=id;root.appendChild(el);}}document.documentElement.style.colorScheme='dark';el.textContent={css};}})();"
    )
}

fn reader_mode_script(enabled: bool) -> String {
    if !enabled {
        return "(function(){var id='mde-cef-reader-style';var el=document.getElementById(id);if(el)el.remove();document.documentElement.classList.remove('mde-reader-mode');})();".to_owned();
    }
    let css = r#"
html.mde-reader-mode body {
  max-width: 76ch !important;
  margin: 0 auto !important;
  padding: 3rem 2rem !important;
  line-height: 1.65 !important;
  font-size: 18px !important;
  font-family: Inter, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif !important;
}
html.mde-reader-mode article, html.mde-reader-mode main {
  max-width: 76ch !important;
  margin-left: auto !important;
  margin-right: auto !important;
}
html.mde-reader-mode nav, html.mde-reader-mode aside, html.mde-reader-mode footer,
html.mde-reader-mode [role="navigation"], html.mde-reader-mode [aria-label*="advert"],
html.mde-reader-mode iframe, html.mde-reader-mode embed {
  display: none !important;
}
html.mde-reader-mode p, html.mde-reader-mode li {
  margin-block: 0.85em !important;
}
html.mde-reader-mode img, html.mde-reader-mode video {
  max-width: 100% !important;
  height: auto !important;
}
"#;
    let css = js_string_literal(css);
    format!(
        "(function(){{var id='mde-cef-reader-style';var root=document.head||document.documentElement;if(!root)return;var el=document.getElementById(id);if(!el){{el=document.createElement('style');el.id=id;root.appendChild(el);}}el.textContent={css};document.documentElement.classList.add('mde-reader-mode');}})();"
    )
}

fn user_agent_override_script(user_agent: &str) -> String {
    if user_agent.trim().is_empty() {
        return "(function(){delete window.__mdeUserAgentOverride;})();".to_owned();
    }
    let ua = js_string_literal(&clamp_utf8(user_agent, 512));
    format!(
        "(function(){{var ua={ua};window.__mdeUserAgentOverride=ua;try{{Object.defineProperty(Navigator.prototype,'userAgent',{{get:function(){{return window.__mdeUserAgentOverride||ua;}},configurable:true}});Object.defineProperty(Navigator.prototype,'appVersion',{{get:function(){{return window.__mdeUserAgentOverride||ua;}},configurable:true}});Object.defineProperty(Navigator.prototype,'platform',{{get:function(){{return /Android|Mobile|iPhone|iPad/.test(window.__mdeUserAgentOverride||ua)?'Linux armv8l':'Linux x86_64';}},configurable:true}});}}catch(_e){{}}}})();"
    )
}

fn device_profile_script(
    profile: &str,
    width: u16,
    height: u16,
    scale_percent: u16,
    touch: bool,
) -> String {
    if profile == "default" || width == 0 || height == 0 {
        return "(function(){delete window.__mdeDeviceProfile;try{delete window.innerWidth;delete window.innerHeight;delete window.devicePixelRatio;}catch(_e){}var meta=document.getElementById('mde-device-profile-viewport');if(meta)meta.remove();delete document.documentElement.dataset.mdeDeviceProfile;})();".to_owned();
    }
    let profile = js_string_literal(&clamp_utf8(profile, 32));
    let width = width.clamp(240, 7680);
    let height = height.clamp(240, 7680);
    let scale = scale_percent.clamp(50, 600);
    let touch_points = if touch { 5 } else { 0 };
    format!(
        "(function(){{var p={{profile:{profile},width:{width},height:{height},dpr:{scale}/100,touch:{touch},touchPoints:{touch_points}}};window.__mdeDeviceProfile=p;document.documentElement.dataset.mdeDeviceProfile=p.profile;var meta=document.getElementById('mde-device-profile-viewport');if(!meta){{meta=document.createElement('meta');meta.id='mde-device-profile-viewport';meta.name='viewport';(document.head||document.documentElement).appendChild(meta);}}meta.content='width='+p.width+', initial-scale=1';function def(o,n,g){{try{{Object.defineProperty(o,n,{{get:g,configurable:true}});}}catch(_e){{}}}}def(window,'innerWidth',function(){{return p.width;}});def(window,'innerHeight',function(){{return p.height;}});def(window,'devicePixelRatio',function(){{return p.dpr;}});if(window.Screen&&Screen.prototype){{def(Screen.prototype,'width',function(){{return p.width;}});def(Screen.prototype,'height',function(){{return p.height;}});def(Screen.prototype,'availWidth',function(){{return p.width;}});def(Screen.prototype,'availHeight',function(){{return p.height;}});}}if(window.Navigator&&Navigator.prototype){{def(Navigator.prototype,'maxTouchPoints',function(){{return p.touchPoints;}});}}}})();"
    )
}

fn page_zoom_script(percent: u16) -> String {
    let percent = percent.clamp(25, 500);
    format!("(function(){{document.documentElement.style.zoom='{percent}%';}})();")
}

fn find_in_page_script(query: &str, backwards: bool) -> String {
    let query = js_string_literal(query);
    let backwards = if backwards { "true" } else { "false" };
    format!("(function(){{window.find({query},false,{backwards},true,false,false,false);}})();")
}

const fn clear_find_script() -> &'static str {
    "(function(){var s=window.getSelection&&window.getSelection();if(s)s.removeAllRanges();})();"
}

fn userscript_library_script(enabled: bool, bundle: &str) -> String {
    if !enabled {
        return "(function(){var style=document.getElementById('mde-browser-userscript-style');if(style)style.remove();if(window.__mdeBrowserUserScriptsObserver){window.__mdeBrowserUserScriptsObserver.disconnect();window.__mdeBrowserUserScriptsObserver=null;}delete document.documentElement.dataset.mdeBrowserUserscripts;})();".to_owned();
    }
    format!(
        "(function(){{try{{document.documentElement.dataset.mdeBrowserUserscripts='true';\n{bundle}\n}}catch(err){{console.warn('mde userscript bundle failed',err);}}}})();"
    )
}

fn spellcheck_highlight_script(words: &[String]) -> String {
    let words: Vec<String> = words
        .iter()
        .filter_map(|word| {
            let trimmed = word.trim();
            if trimmed.len() < 2 || trimmed.len() > 64 {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
        .take(64)
        .collect();
    let words = js_string_array_literal(&words);
    r#"(function(){
var cls='mde-browser-spell-miss';
var old=document.querySelectorAll('span.'+cls);
for(var i=old.length-1;i>=0;i--){var n=old[i];n.replaceWith(document.createTextNode(n.textContent||''));}
if(!document.body){return;}
document.body.normalize();
var words=__WORDS__;
if(!words.length){delete document.documentElement.dataset.mdeBrowserSpellcheck;return;}
var style=document.getElementById('mde-browser-spellcheck-style');
if(!style){style=document.createElement('style');style.id='mde-browser-spellcheck-style';(document.head||document.documentElement).appendChild(style);}
style.textContent='span.'+cls+'{text-decoration: underline wavy #d13438; text-decoration-thickness: 1.5px; text-underline-offset: 0.12em;}';
var escaped=words.map(function(w){return String(w).replace(/[.*+?^${}()|[\]\\]/g,'\\$&');}).filter(Boolean);
if(!escaped.length){return;}
var re=new RegExp('\\b('+escaped.join('|')+')\\b','gi');
var walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT,{acceptNode:function(node){
  var p=node.parentElement;
  if(!p||p.closest('script,style,textarea,input,select,span.'+cls))return NodeFilter.FILTER_REJECT;
  re.lastIndex=0;
  return re.test(node.nodeValue||'')?NodeFilter.FILTER_ACCEPT:NodeFilter.FILTER_REJECT;
}});
var nodes=[];
while(nodes.length<256){var node=walker.nextNode();if(!node)break;nodes.push(node);}
for(var n=0;n<nodes.length;n++){
  var text=nodes[n].nodeValue||'';re.lastIndex=0;
  var frag=document.createDocumentFragment();var last=0;var m;
  while((m=re.exec(text))&&frag.childNodes.length<512){
    if(m.index>last)frag.appendChild(document.createTextNode(text.slice(last,m.index)));
    var span=document.createElement('span');span.className=cls;span.dataset.mdeBrowserSpellcheck='miss';span.textContent=m[0];frag.appendChild(span);
    last=m.index+m[0].length;
  }
  if(last<text.length)frag.appendChild(document.createTextNode(text.slice(last)));
  nodes[n].replaceWith(frag);
}
document.documentElement.dataset.mdeBrowserSpellcheck=String(words.length);
})();"#
        .replace("__WORDS__", &words)
}

fn spellcheck_correction_script(word: &str, replacement: &str) -> String {
    spellcheck_correction_script_with_target(word, replacement, Some(0))
}

fn spellcheck_correction_all_script(word: &str, replacement: &str) -> String {
    spellcheck_correction_script_with_target(word, replacement, None)
}

fn spellcheck_correction_at_script(word: &str, replacement: &str, occurrence: u16) -> String {
    spellcheck_correction_script_with_target(word, replacement, Some(occurrence))
}

fn spellcheck_correction_script_with_target(
    word: &str,
    replacement: &str,
    target_occurrence: Option<u16>,
) -> String {
    let word = word.trim();
    let replacement = replacement.trim();
    if word.is_empty() || replacement.is_empty() || word.len() > 64 || replacement.len() > 128 {
        return "()=>{}".to_owned();
    }
    let word = js_string_literal(word);
    let replacement = js_string_literal(replacement);
    let target_occurrence = target_occurrence.map_or(-1, i32::from);
    format!(
        r#"(function(){{
var word={word};
var replacement={replacement};
var targetOccurrence={target_occurrence};
var replaceAll=targetOccurrence<0;
var cls='mde-browser-spell-miss';
function same(value){{return String(value||'').toLocaleLowerCase()===word.toLocaleLowerCase();}}
var marks=document.querySelectorAll('span.'+cls);
var changed=0;var seen=0;var markMatches=0;
for(var i=0;i<marks.length;i++){{
  if(same(marks[i].textContent)){{
    markMatches++;
    if(!replaceAll&&seen!==targetOccurrence){{seen++;continue;}}
    marks[i].replaceWith(document.createTextNode(replacement));
    changed++;
    if(!replaceAll){{
      document.body&&document.body.normalize();
      return;
    }}
    seen++;
  }}
}}
if(markMatches>0&&!replaceAll)return;
if(changed>0){{
  document.body&&document.body.normalize();
  return;
}}
if(!document.body)return;
var escaped=word.replace(/[.*+?^${{}}()|[\]\\]/g,'\\$&');
var re=new RegExp('\\b'+escaped+'\\b','gi');
var walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT,{{acceptNode:function(node){{
  var p=node.parentElement;
  if(!p||p.closest('script,style,textarea,input,select'))return NodeFilter.FILTER_REJECT;
  re.lastIndex=0;
  return re.test(node.nodeValue||'')?NodeFilter.FILTER_ACCEPT:NodeFilter.FILTER_REJECT;
}}}});
var node;var total=0;
while((node=walker.nextNode())&&total<512){{
  var text=node.nodeValue||'';
  re.lastIndex=0;
  if(!replaceAll){{
    var m;
    while((m=re.exec(text))&&total<512){{
      if(total===targetOccurrence){{
        node.nodeValue=text.slice(0,m.index)+replacement+text.slice(m.index+m[0].length);
        return;
      }}
      total++;
    }}
    continue;
  }}
  var next=text.replace(re,function(m){{total++;return total<=512?replacement:m;}});
  if(next!==text)node.nodeValue=next;
}}
}})();"#
    )
}

fn page_text_beacon_script(id: u64, max_bytes: u32) -> String {
    let max_bytes = max_bytes.clamp(1, CEF_PAGE_TEXT_BEACON_MAX_BYTES);
    format!(
        "(function(){{try{{var cap={max_bytes};var root=document.body||document.documentElement;\
var text=root?String(root.innerText||root.textContent||''):'';\
text=text.replace(/\\s+/g,' ').trim();if(text.length>cap)text=text.slice(0,cap);\
var img=document.createElement('img');img.alt='';img.width=1;img.height=1;\
img.style.cssText='position:absolute;left:-9999px;top:-9999px;width:1px;height:1px';\
img.src='{}{}?text='+encodeURIComponent(text);\
(document.body||document.documentElement).appendChild(img);}}catch(err){{\
var fallback=document.createElement('img');fallback.alt='';fallback.width=1;fallback.height=1;\
fallback.style.cssText='position:absolute;left:-9999px;top:-9999px;width:1px;height:1px';\
fallback.src='{}{}?text=';(document.body||document.documentElement).appendChild(fallback);}}}})();",
        CEF_PAGE_TEXT_BEACON_PREFIX, id, CEF_PAGE_TEXT_BEACON_PREFIX, id
    )
}

fn page_scrape_beacon_script(id: u64, max_bytes: u32, max_links: u16, max_headings: u16) -> String {
    let max_bytes = max_bytes.clamp(1, CEF_PAGE_SCRAPE_BEACON_MAX_BYTES as u32);
    let max_links = max_links.min(128);
    let max_headings = max_headings.min(64);
    format!(
        r#"(function(){{try{{
var textCap=Math.min({max_bytes},16384),linkCap={max_links},headingCap={max_headings},articleCap=8192,bodyCap={body_cap};
function trim(v,n){{v=String(v||'').replace(/\s+/g,' ').trim();return v.length>n?v.slice(0,n):v;}}
function visible(el){{try{{if(!el||!el.getClientRects||!el.getClientRects().length)return false;var s=getComputedStyle(el);return s.visibility!=='hidden'&&s.display!=='none';}}catch(_){{return true;}}}}
var root=document.body||document.documentElement;
var raw=root?String(root.innerText||root.textContent||''):'';
var normalized=trim(raw,textCap);
var articleNode=null,articleSelector='';
var candidates=document.querySelectorAll?document.querySelectorAll('article,main,[role=main]'):[];
for(var c=0;c<candidates.length;c++){{if(visible(candidates[c])){{articleNode=candidates[c];articleSelector=(articleNode.tagName||'').toLowerCase();if(articleNode.getAttribute&&articleNode.getAttribute('role'))articleSelector+='[role='+articleNode.getAttribute('role')+']';break;}}}}
var articleRaw=articleNode?String(articleNode.innerText||articleNode.textContent||''):'';
var articleText=trim(articleRaw,articleCap);
var links=[];
var anchors=document.querySelectorAll?document.querySelectorAll('a[href]'):[];
for(var i=0;i<anchors.length&&links.length<linkCap;i++){{var a=anchors[i];if(!visible(a))continue;var href=trim(a.href||a.getAttribute('href')||'',2048);if(!href)continue;links.push({{url:href,text:trim(a.innerText||a.textContent||a.getAttribute('aria-label')||'',160),rel:trim(a.getAttribute('rel')||'',80),target:trim(a.getAttribute('target')||'',40)}});}}
var headings=[];
var hs=document.querySelectorAll?document.querySelectorAll('h1,h2,h3,h4,h5,h6'):[];
for(var h=0;h<hs.length&&headings.length<headingCap;h++){{var el=hs[h];if(!visible(el))continue;var label=trim(el.innerText||el.textContent||'',240);if(!label)continue;headings.push({{level:Number(String(el.tagName||'H0').slice(1))||0,text:label}});}}
var canonicalEl=document.querySelector?document.querySelector('link[rel~="canonical"][href]'):null;
var descriptionEl=document.querySelector?document.querySelector('meta[name="description" i][content],meta[property="og:description"][content]'):null;
function payload(){{return {{text:normalized,text_truncated:trim(raw,2147483647).length>textCap,article_text:articleText,article_text_truncated:trim(articleRaw,2147483647).length>articleCap,article_selector:articleSelector,canonical_url:canonicalEl?trim(canonicalEl.href||canonicalEl.getAttribute('href')||'',2048):'',meta_description:descriptionEl?trim(descriptionEl.getAttribute('content')||'',512):'',document_lang:trim((document.documentElement&&document.documentElement.lang)||'',64),links:links,headings:headings}};}}
var body=JSON.stringify(payload());
if(body.length>bodyCap){{links=links.slice(0,32);headings=headings.slice(0,16);normalized=trim(normalized,8192);articleText=trim(articleText,4096);body=JSON.stringify(payload());}}
if(body.length>bodyCap){{links=[];headings=[];normalized=trim(normalized,4096);articleText=trim(articleText,2048);body=JSON.stringify(payload());}}
var img=document.createElement('img');img.alt='';img.width=1;img.height=1;img.style.cssText='position:absolute;left:-9999px;top:-9999px;width:1px;height:1px';img.src='{prefix}{id}?body='+encodeURIComponent(body);(document.body||document.documentElement).appendChild(img);
}}catch(err){{var fallback=document.createElement('img');fallback.alt='';fallback.width=1;fallback.height=1;fallback.style.cssText='position:absolute;left:-9999px;top:-9999px;width:1px;height:1px';fallback.src='{prefix}{id}?body=';(document.body||document.documentElement).appendChild(fallback);}}}})();"#,
        body_cap = CEF_PAGE_SCRAPE_BEACON_MAX_BYTES,
        prefix = CEF_PAGE_SCRAPE_BEACON_PREFIX,
    )
}

/// Best-effort renderer-level removal of the JS-reachable WebRTC surface.
///
/// `chromium_privacy_switches()` (`cef_init.rs`) cannot fully disable WebRTC
/// at the command-line level: `--disable-webrtc` is not a real Chromium
/// switch (verified against the live `content_switches.cc`/
/// `chrome_switches.cc` upstream — Chromium silently no-ops unrecognized `--`
/// switches rather than erroring), and the only genuine kill switch is the
/// build-time GN flag `enable_webrtc=false`, unavailable on this crate's
/// vendored prebuilt CEF binary. This deletes the JS constructors/entry
/// points instead, matching the CEF community's own recommended technique
/// (remove the interfaces from the renderer's global scope at script-inject
/// time) and this codebase's existing shim-injection pattern
/// (`passkey_bridge_script`). Injected once per navigation generation (and
/// re-applied through a fresh document's settle window — see `ShimInjector` /
/// `inject_context_shims`) rather than on a blind 250 ms timer; the installed
/// `MutationObserver` keeps late subframes covered within a stable document.
/// This is a baseline privacy default, not a per-tab, user-toggleable feature.
///
/// This is defense-in-depth, not an airtight guarantee: this ABI has no
/// `OnContextCreated`-equivalent early-injection hook, so a page's own inline
/// script can still run before the first injection lands, and a fresh
/// document commit (e.g. an in-page navigation) gets an unpatched JS context
/// until the navigation-driven re-injection re-applies this. `--force-webrtc-ip
/// -handling-policy=disable_non_proxied_udp` (kept in
/// `chromium_privacy_switches()`, verified real) is the second layer: even a
/// same-tick `RTCPeerConnection` that gets past this script still cannot leak a
/// raw local IP over non-proxied UDP.
const fn webrtc_block_script() -> &'static str {
    // browser-3: the removal is applied to EVERY reachable frame, not just the
    // main frame. `strip(w)` deletes the JS-reachable WebRTC surface on a target
    // window; `sweep(w)` recurses through `w.frames` so a child (or nested)
    // same-origin iframe — the trivial main-frame-only bypass — is covered too.
    // A `MutationObserver` re-sweeps on DOM mutation so a *newly inserted* iframe
    // is patched as soon as it appears, between the 250ms poll ticks. Cross-origin
    // subframes are unreachable from JS by same-origin policy (property access on
    // them throws and is swallowed) — the `--force-webrtc-ip-handling-policy`
    // switch remains the backstop for that residual, see this file's cef_init
    // companion. A native `CefPermissionHandler`/ICE-layer deny would be airtight
    // but the pinned CEF 149 ABI exposes no permission-handler or frame-enumeration
    // vtable offset verified from the farm headers, so it is not attempted here.
    "(function(){function strip(w){try{delete w.RTCPeerConnection;}catch(_e){}try{delete w.webkitRTCPeerConnection;}catch(_e){}try{delete w.RTCDataChannel;}catch(_e){}try{delete w.RTCSessionDescription;}catch(_e){}try{delete w.RTCIceCandidate;}catch(_e){}try{if(w.MediaDevices&&w.MediaDevices.prototype){delete w.MediaDevices.prototype.getUserMedia;delete w.MediaDevices.prototype.getDisplayMedia;}}catch(_e){}try{if(w.navigator&&w.navigator.mediaDevices){delete w.navigator.mediaDevices.getUserMedia;delete w.navigator.mediaDevices.getDisplayMedia;}}catch(_e){}try{delete w.navigator.getUserMedia;}catch(_e){}try{delete w.navigator.webkitGetUserMedia;}catch(_e){}try{delete w.navigator.mozGetUserMedia;}catch(_e){}}function sweep(w){try{strip(w);}catch(_e){}var kids=null;try{kids=w.frames;}catch(_e){kids=null;}if(kids){for(var i=0;i<kids.length;i++){var cw=null;try{cw=kids[i];}catch(_e){cw=null;}if(cw&&cw!==w){try{sweep(cw);}catch(_e){}}}}}sweep(window);try{if(!window.__mdeWebrtcBlockObserver&&window.MutationObserver&&document&&document.documentElement){window.__mdeWebrtcBlockObserver=new MutationObserver(function(){try{sweep(window);}catch(_e){}});window.__mdeWebrtcBlockObserver.observe(document.documentElement,{childList:true,subtree:true});}}catch(_e){}})();"
}

fn passkey_bridge_script() -> String {
    format!(
        r#"(function(){{
try{{
  if(!window.__mdeBrowserPasskeyQueue)window.__mdeBrowserPasskeyQueue=[];
  if(!window.__mdeBrowserPasskeyPending)window.__mdeBrowserPasskeyPending={{}};
  if(!window.__mdeBrowserPasskeyComplete){{
    window.__mdeBrowserPasskeyComplete=function(event){{
      try{{
        event=event||{{}};
        var id=String(event.client_request_id||'');
        var pending=window.__mdeBrowserPasskeyPending&&window.__mdeBrowserPasskeyPending[id];
        if(!pending)return false;
        delete window.__mdeBrowserPasskeyPending[id];
        function ab(v){{
          try{{
            v=String(v||'').replace(/-/g,'+').replace(/_/g,'/');
            while(v.length%4)v+='=';
            var s=atob(v),out=new Uint8Array(s.length);
            for(var i=0;i<s.length;i++)out[i]=s.charCodeAt(i);
            return out.buffer;
          }}catch(_){{return new ArrayBuffer(0);}}
        }}
        if(event.error||event.state==='error'){{
          pending.reject(new DOMException(String(event.error||'Passkey ceremony failed'),'NotAllowedError'));
          return true;
        }}
        function setProto(obj,ctor){{try{{if(ctor&&ctor.prototype)Object.setPrototypeOf(obj,ctor.prototype);}}catch(_){{}}return obj;}}
        function b64(v){{return String(v||'');}}
        var credentialId=String(event.credential_id_b64url||'');
        var response={{}};
        if(event.op==='browser_passkey_assertion'||event.ceremony==='get'){{
          response.authenticatorData=ab(event.authenticator_data_b64url);
          response.clientDataJSON=ab(event.client_data_json_b64url);
          response.signature=ab(event.signature_b64url);
          response.userHandle=ab(event.user_handle_b64url);
          response.toJSON=function(){{return {{authenticatorData:b64(event.authenticator_data_b64url),clientDataJSON:b64(event.client_data_json_b64url),signature:b64(event.signature_b64url),userHandle:b64(event.user_handle_b64url)}};}};
          setProto(response,window.AuthenticatorAssertionResponse);
        }}else{{
          response.clientDataJSON=ab(event.client_data_json_b64url);
          response.attestationObject=ab(event.attestation_object_b64url);
          response.getPublicKey=function(){{return ab(event.public_key_spki_der_b64url||event.public_key_sec1_b64url);}};
          response.getPublicKeyAlgorithm=function(){{return Number(event.cose_alg||-7);}};
          response.getTransports=function(){{return ['internal'];}};
          response.getAuthenticatorData=function(){{return ab(event.authenticator_data_b64url);}};
          response.toJSON=function(){{return {{attestationObject:b64(event.attestation_object_b64url),clientDataJSON:b64(event.client_data_json_b64url),publicKey:b64(event.public_key_spki_der_b64url||event.public_key_sec1_b64url),publicKeyAlgorithm:Number(event.cose_alg||-7),authenticatorData:b64(event.authenticator_data_b64url),transports:['internal']}};}};
          setProto(response,window.AuthenticatorAttestationResponse);
        }}
        var credential={{id:credentialId,rawId:ab(credentialId),type:'public-key',authenticatorAttachment:'platform',response:response}};
        credential.getClientExtensionResults=function(){{return {{}};}};
        credential.toJSON=function(){{return {{id:credentialId,rawId:credentialId,type:'public-key',authenticatorAttachment:'platform',response:response.toJSON?response.toJSON():{{}},clientExtensionResults:{{}}}};}};
        pending.resolve(setProto(credential,window.PublicKeyCredential));
        return true;
      }}catch(err){{return false;}}
    }};
  }}
  function emit(item){{
    try{{
      var img=document.createElement('img');img.alt='';img.width=1;img.height=1;
      img.style.cssText='position:absolute;left:-9999px;top:-9999px;width:1px;height:1px';
      img.src='{prefix}?body='+encodeURIComponent(JSON.stringify(item).slice(0,8192));
      (document.body||document.documentElement).appendChild(img);
    }}catch(_){{}}
  }}
  if(!window.__mdeBrowserPasskeyBridgeInstalled){{
    window.__mdeBrowserPasskeyBridgeInstalled=true;
    window.__mdeBrowserPasskeySeq=window.__mdeBrowserPasskeySeq||0;
    try{{
      if(!window.PublicKeyCredential)window.PublicKeyCredential=function PublicKeyCredential(){{}};
      if(!window.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable)window.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable=function(){{return Promise.resolve(true);}};
      if(!window.PublicKeyCredential.isConditionalMediationAvailable)window.PublicKeyCredential.isConditionalMediationAvailable=function(){{return Promise.resolve(false);}};
      if(!window.AuthenticatorAttestationResponse)window.AuthenticatorAttestationResponse=function AuthenticatorAttestationResponse(){{}};
      if(!window.AuthenticatorAssertionResponse)window.AuthenticatorAssertionResponse=function AuthenticatorAssertionResponse(){{}};
    }}catch(_){{}}
    function trim(v,n){{v=String(v||'').trim();return v.length>n?v.slice(0,n):v;}}
    function b64url(value){{
      try{{
        if(value==null)return '';
        if(typeof value==='string')return value.replace(/=+$/,'').replace(/\+/g,'-').replace(/\//g,'_');
        var bytes=null;
        if(value instanceof ArrayBuffer)bytes=new Uint8Array(value);
        else if(ArrayBuffer.isView(value))bytes=new Uint8Array(value.buffer,value.byteOffset,value.byteLength);
        if(!bytes)return '';
        var s='',max=Math.min(bytes.length,1536);
        for(var i=0;i<max;i++)s+=String.fromCharCode(bytes[i]);
        return btoa(s).replace(/=+$/,'').replace(/\+/g,'-').replace(/\//g,'_');
      }}catch(_){{return '';}}
    }}
    function hasUserGesture(){{
      try{{
        var ua=navigator.userActivation;
        if(ua&&typeof ua.isActive==='boolean')return ua.isActive;
      }}catch(_){{}}
      return false;
    }}
    function ceremony(kind,options){{
      var pk=(options&&options.publicKey)||{{}};
      var rp=(pk.rp&&pk.rp.id)||location.hostname;
      var out={{ceremony:kind,origin:String(location.href||''),rp_id:trim(rp,253),challenge_b64url:b64url(pk.challenge)}};
      if(kind==='create'&&pk.user){{
        out.user_handle_b64url=b64url(pk.user.id);
        out.user_name=trim(pk.user.displayName||pk.user.name||'',256);
      }}
      if(kind==='get'&&Array.isArray(pk.allowCredentials)){{
        out.allow_credentials=pk.allowCredentials.slice(0,64).map(function(c){{return b64url(c&&c.id);}}).filter(Boolean);
      }}
      if(typeof pk.timeout==='number')out.timeout_ms=Math.max(0,Math.floor(pk.timeout));
      out.user_present=hasUserGesture();
      return out;
    }}
    function enqueue(kind,options){{
      var item=ceremony(kind,options);
      if(!item.challenge_b64url)return Promise.reject(new DOMException('Passkey challenge missing','NotAllowedError'));
      if(!item.user_present)return Promise.reject(new DOMException('Passkey ceremony requires a user gesture','NotAllowedError'));
      item.client_request_id='mde-pk-'+Date.now().toString(36)+'-'+(++window.__mdeBrowserPasskeySeq).toString(36);
      var q=window.__mdeBrowserPasskeyQueue;
      q.push(item);
      while(q.length>16)q.shift();
      return new Promise(function(resolve,reject){{window.__mdeBrowserPasskeyPending[item.client_request_id]={{resolve:resolve,reject:reject,ceremony:kind}};}});
    }}
    var creds=navigator.credentials||(navigator.credentials={{}});
    var origCreate=(typeof creds.create==='function')?creds.create.bind(creds):null;
    var origGet=(typeof creds.get==='function')?creds.get.bind(creds):null;
    creds.create=function(options){{
      if(options&&options.publicKey)return enqueue('create',options);
      if(origCreate)return origCreate(options);
      return Promise.reject(new DOMException('Unsupported credential type','NotSupportedError'));
    }};
    creds.get=function(options){{
      if(options&&options.publicKey)return enqueue('get',options);
      if(origGet)return origGet(options);
      return Promise.reject(new DOMException('Unsupported credential type','NotSupportedError'));
    }};
    window.__mdeBrowserPasskeyDrain=function(){{try{{var dq=window.__mdeBrowserPasskeyQueue;if(dq){{for(var n=0;n<4&&dq.length;n++)emit(dq.shift());}}}}catch(_){{}}}};
  }}
  var q=window.__mdeBrowserPasskeyQueue;
  for(var n=0;n<4&&q.length;n++)emit(q.shift());
}}catch(_){{}}
}})();"#,
        prefix = CEF_PASSKEY_BEACON_PREFIX
    )
}

fn passkey_complete_script(body: &str) -> String {
    let body = js_string_literal(body);
    format!(
        "(function(){{try{{var event=JSON.parse({body});if(window.__mdeBrowserPasskeyComplete)window.__mdeBrowserPasskeyComplete(event);}}catch(_){{}}}})();"
    )
}

/// browser-8: the lightweight passkey heartbeat. It only calls the drain closure
/// installed once by [`passkey_bridge_script`], so the multi-KB bridge shim is no
/// longer recompiled/re-executed every 250 ms — but page-initiated ceremonies are
/// still delivered promptly. A no-op until the bridge has been installed for the
/// current document (`__mdeBrowserPasskeyDrain` undefined), which is exactly when
/// there is nothing to drain.
const fn passkey_drain_script() -> &'static str {
    "(function(){try{if(window.__mdeBrowserPasskeyDrain)window.__mdeBrowserPasskeyDrain();}catch(_){}})();"
}

fn decode_page_text_beacon(url: &str) -> Option<(u64, String)> {
    let rest = url
        .strip_prefix(CEF_PAGE_TEXT_BEACON_PREFIX)
        .or_else(|| url.strip_prefix(CEF_PAGE_TEXT_BEACON_LEGACY_PREFIX))?;
    let (id, query) = rest.split_once('?').unwrap_or((rest, ""));
    let id = id.parse::<u64>().ok()?;
    let text = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("text="))
        .unwrap_or_default();
    Some((
        id,
        clamp_utf8(
            &percent_decode(text),
            CEF_PAGE_TEXT_BEACON_MAX_BYTES as usize,
        ),
    ))
}

fn decode_page_scrape_beacon(url: &str) -> Option<(u64, String)> {
    let rest = url.strip_prefix(CEF_PAGE_SCRAPE_BEACON_PREFIX)?;
    let (id, query) = rest.split_once('?').unwrap_or((rest, ""));
    let id = id.parse::<u64>().ok()?;
    let body = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("body="))
        .unwrap_or_default();
    Some((
        id,
        clamp_utf8(&percent_decode(body), CEF_PAGE_SCRAPE_BEACON_MAX_BYTES),
    ))
}

fn decode_passkey_beacon(url: &str) -> Option<String> {
    let query = url.strip_prefix(CEF_PASSKEY_BEACON_PREFIX)?;
    let query = query.strip_prefix('?').unwrap_or(query);
    let body = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("body="))
        .unwrap_or_default();
    let body = clamp_utf8(&percent_decode(body), CEF_PASSKEY_BEACON_MAX_BYTES);
    body.trim_start().starts_with('{').then_some(body)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn clamp_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn js_string_literal(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn js_string_array_literal(values: &[String]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&js_string_literal(value));
    }
    out.push(']');
    out
}

fn main_frame(browser: *mut c_void) -> Option<*mut c_void> {
    let get_main_frame = read_fn(browser, CEF_BROWSER_GET_MAIN_FRAME_OFFSET)?;
    // SAFETY: `get_main_frame` is read from a live `cef_browser_t` function slot
    // using the offset verified from the pinned CEF 149 headers.
    let get_main_frame: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(get_main_frame) };
    // SAFETY: CEF returned `browser` from `cef_browser_host_create_browser_sync`.
    let frame = unsafe { get_main_frame(browser) };
    (!frame.is_null()).then_some(frame)
}

fn request_url(
    request: *mut c_void,
    string_userfree_free: CefStringUserfreeUtf16Free,
) -> Option<String> {
    let get_url = read_fn(request, CEF_REQUEST_GET_URL_OFFSET)?;
    // SAFETY: `get_url` is read from `cef_request_t::get_url`, whose pinned C
    // signature is `cef_string_userfree_t (*)(cef_request_t*)`.
    let get_url: unsafe extern "C" fn(*mut c_void) -> *mut CefString =
        unsafe { std::mem::transmute(get_url) };
    // SAFETY: CEF supplied a live `cef_request_t` for the callback duration.
    let raw = unsafe { get_url(request) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: CEF returned a non-null userfree UTF-16 string. Copy before
    // freeing with the matching libcef symbol.
    let text = unsafe {
        let value = if (*raw).str_.is_null() || (*raw).length == 0 {
            String::new()
        } else {
            String::from_utf16_lossy(std::slice::from_raw_parts((*raw).str_, (*raw).length))
        };
        string_userfree_free(raw.cast());
        value
    };
    (!text.is_empty()).then_some(text)
}

fn cef_string_to_string(raw: *const CefString) -> String {
    if raw.is_null() {
        return String::new();
    }
    // SAFETY: callers pass a CEF-owned string pointer that is live for the
    // callback duration. Copy the UTF-16 contents immediately.
    unsafe {
        if (*raw).str_.is_null() || (*raw).length == 0 {
            String::new()
        } else {
            String::from_utf16_lossy(std::slice::from_raw_parts((*raw).str_, (*raw).length))
        }
    }
}

fn add_ref_cef(object: *mut c_void) {
    call_ref_counted_void(object, BASE_ADD_REF_OFFSET);
}

fn release_cef(object: *mut c_void) {
    call_ref_counted_int(object, BASE_RELEASE_OFFSET);
}

fn continue_cef_callback(callback: *mut c_void) {
    call_ref_counted_void(callback, CEF_CALLBACK_CONT_OFFSET);
}

fn cancel_cef_callback(callback: *mut c_void) {
    call_ref_counted_void(callback, CEF_CALLBACK_CANCEL_OFFSET);
}

fn call_ref_counted_void(object: *mut c_void, offset: usize) {
    let Some(callback) = read_fn(object, offset) else {
        return;
    };
    // SAFETY: `callback` is read from a CEF ref-counted object function slot
    // using a header-verified offset.
    let callback: unsafe extern "C" fn(*mut c_void) = unsafe { std::mem::transmute(callback) };
    // SAFETY: caller passes a live CEF ref-counted object pointer.
    unsafe { callback(object) };
}

fn call_ref_counted_int(object: *mut c_void, offset: usize) {
    let Some(callback) = read_fn(object, offset) else {
        return;
    };
    // SAFETY: `callback` is read from a CEF ref-counted object function slot
    // using a header-verified offset.
    let callback: unsafe extern "C" fn(*mut c_void) -> c_int =
        unsafe { std::mem::transmute(callback) };
    // SAFETY: caller passes a live CEF ref-counted object pointer.
    let _ = unsafe { callback(object) };
}

fn read_fn(base: *mut c_void, offset: usize) -> Option<usize> {
    if base.is_null() {
        return None;
    }
    // SAFETY: caller supplies a live CEF struct pointer and a function-pointer
    // offset verified from the pinned CEF 149 headers.
    let value = unsafe { *((base.cast::<u8>()).add(offset).cast::<usize>()) };
    (value != 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_pdf_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
            "mde-web-cef-{tag}-{}-{nanos}.pdf",
            std::process::id()
        ))
    }

    #[test]
    fn callback_layout_matches_pinned_cef_headers() {
        assert_eq!(CEF_BASE_REF_COUNTED_SIZE, 40);
        assert_eq!(CEF_CLIENT_SIZE, 192);
        assert_eq!(CEF_CLIENT_GET_LIFE_SPAN_HANDLER_OFFSET, 144);
        assert_eq!(CEF_CLIENT_GET_PRINT_HANDLER_OFFSET, 160);
        assert_eq!(CEF_CLIENT_GET_RENDER_HANDLER_OFFSET, 168);
        assert_eq!(CEF_CLIENT_GET_REQUEST_HANDLER_OFFSET, 176);
        assert_eq!(CEF_LIFE_SPAN_HANDLER_SIZE, 88);
        assert_eq!(CEF_LIFE_SPAN_ON_AFTER_CREATED_OFFSET, 64);
        assert_eq!(CEF_RENDER_HANDLER_SIZE, 176);
        assert_eq!(CEF_RENDER_HANDLER_GET_VIEW_RECT_OFFSET, 56);
        assert_eq!(CEF_RENDER_HANDLER_ON_PAINT_OFFSET, 96);
        assert_eq!(CEF_REQUEST_HANDLER_SIZE, 128);
        assert_eq!(CEF_REQUEST_HANDLER_GET_RESOURCE_REQUEST_HANDLER_OFFSET, 56);
        assert_eq!(CEF_RESOURCE_REQUEST_HANDLER_SIZE, 104);
        assert_eq!(
            CEF_RESOURCE_REQUEST_HANDLER_ON_BEFORE_RESOURCE_LOAD_OFFSET,
            48
        );
        assert_eq!(CEF_PRINT_HANDLER_SIZE, 88);
        assert_eq!(CEF_PRINT_HANDLER_ON_PRINT_DIALOG_OFFSET, 56);
        assert_eq!(CEF_PRINT_HANDLER_ON_PRINT_JOB_OFFSET, 64);
        assert_eq!(CEF_PRINT_HANDLER_GET_PDF_PAPER_SIZE_OFFSET, 80);
        assert_eq!(CEF_BROWSER_SIZE, 208);
        assert_eq!(CEF_BROWSER_GET_HOST_OFFSET, 48);
        assert_eq!(CEF_BROWSER_CAN_GO_BACK_OFFSET, 56);
        assert_eq!(CEF_BROWSER_GO_BACK_OFFSET, 64);
        assert_eq!(CEF_BROWSER_CAN_GO_FORWARD_OFFSET, 72);
        assert_eq!(CEF_BROWSER_GO_FORWARD_OFFSET, 80);
        assert_eq!(CEF_BROWSER_RELOAD_OFFSET, 96);
        assert_eq!(CEF_BROWSER_STOP_LOAD_OFFSET, 112);
        assert_eq!(CEF_BROWSER_GET_MAIN_FRAME_OFFSET, 152);
        assert_eq!(CEF_BROWSER_HOST_SIZE, 592);
        assert_eq!(CEF_BROWSER_HOST_CLOSE_BROWSER_OFFSET, 48);
        assert_eq!(CEF_BROWSER_HOST_SET_FOCUS_OFFSET, 72);
        assert_eq!(CEF_BROWSER_HOST_WAS_RESIZED_OFFSET, 304);
        assert_eq!(CEF_BROWSER_HOST_INVALIDATE_OFFSET, 328);
        assert_eq!(CEF_BROWSER_HOST_SEND_KEY_EVENT_OFFSET, 344);
        assert_eq!(CEF_BROWSER_HOST_SEND_MOUSE_CLICK_EVENT_OFFSET, 352);
        assert_eq!(CEF_BROWSER_HOST_SEND_MOUSE_MOVE_EVENT_OFFSET, 360);
        assert_eq!(CEF_BROWSER_HOST_SEND_MOUSE_WHEEL_EVENT_OFFSET, 368);
        assert_eq!(CEF_BROWSER_HOST_PRINT_OFFSET, 504);
        assert_eq!(CEF_BROWSER_HOST_PRINT_TO_PDF_OFFSET, 512);
        assert_eq!(CEF_BROWSER_HOST_SET_AUDIO_MUTED_OFFSET, 520);
        assert_eq!(CEF_BROWSER_HOST_IS_AUDIO_MUTED_OFFSET, 528);
        assert_eq!(CEF_FRAME_SIZE, 248);
        assert_eq!(CEF_FRAME_LOAD_URL_OFFSET, 144);
        assert_eq!(CEF_FRAME_EXECUTE_JAVA_SCRIPT_OFFSET, 152);
        assert_eq!(CEF_REQUEST_SIZE, 216);
        assert_eq!(CEF_REQUEST_GET_URL_OFFSET, 48);
        assert_eq!(CEF_CALLBACK_SIZE, 56);
        assert_eq!(CEF_CALLBACK_CONT_OFFSET, 40);
        assert_eq!(CEF_CALLBACK_CANCEL_OFFSET, 48);
        assert_eq!(CEF_PDF_PRINT_CALLBACK_SIZE, 48);
        assert_eq!(CEF_PDF_PRINT_CALLBACK_ON_FINISHED_OFFSET, 40);
        assert_eq!(CEF_MOUSE_EVENT_SIZE, 12);
        assert_eq!(CEF_MOUSE_EVENT_X_OFFSET, 0);
        assert_eq!(CEF_MOUSE_EVENT_Y_OFFSET, 4);
        assert_eq!(CEF_MOUSE_EVENT_MODIFIERS_OFFSET, 8);
        assert_eq!(CEF_KEY_EVENT_SIZE, 40);
        assert_eq!(CEF_KEY_EVENT_TYPE_OFFSET, 8);
        assert_eq!(CEF_KEY_EVENT_MODIFIERS_OFFSET, 12);
        assert_eq!(CEF_KEY_EVENT_WINDOWS_KEY_CODE_OFFSET, 16);
        assert_eq!(CEF_KEY_EVENT_NATIVE_KEY_CODE_OFFSET, 20);
        assert_eq!(CEF_KEY_EVENT_IS_SYSTEM_KEY_OFFSET, 24);
        assert_eq!(CEF_KEY_EVENT_CHARACTER_OFFSET, 28);
        assert_eq!(CEF_KEY_EVENT_UNMODIFIED_CHARACTER_OFFSET, 30);
        assert_eq!(CEF_KEY_EVENT_FOCUS_ON_EDITABLE_FIELD_OFFSET, 32);
        assert_eq!(size_of::<CefRect>(), CEF_RECT_SIZE);
        assert_eq!(size_of::<CefSize>(), 8);
        assert_eq!(align_of::<CefCallbackBlock<CEF_CLIENT_SIZE>>(), 8);
    }

    #[test]
    fn window_info_sets_windowless_bounds_from_header_offsets() {
        let info = CefWindowInfo::windowless(800, 600);
        assert_eq!(read_usize(&info.bytes, 0), CEF_WINDOW_INFO_SIZE);
        assert_eq!(
            read_i32(&info.bytes, CEF_WINDOW_INFO_BOUNDS_OFFSET + 8),
            800
        );
        assert_eq!(
            read_i32(&info.bytes, CEF_WINDOW_INFO_BOUNDS_OFFSET + 12),
            600
        );
        assert_eq!(read_i32(&info.bytes, CEF_WINDOW_INFO_WINDOWLESS_OFFSET), 1);
        assert_eq!(
            read_i32(&info.bytes, CEF_WINDOW_INFO_SHARED_TEXTURE_OFFSET),
            0
        );
        assert_eq!(
            read_i32(&info.bytes, CEF_WINDOW_INFO_RUNTIME_STYLE_OFFSET),
            CEF_RUNTIME_STYLE_ALLOY
        );
    }

    #[test]
    fn browser_settings_set_size_rate_and_background() {
        let settings = CefBrowserSettings::windowless(15);
        assert_eq!(read_usize(&settings.bytes, 0), CEF_BROWSER_SETTINGS_SIZE);
        assert_eq!(
            read_i32(&settings.bytes, CEF_BROWSER_SETTINGS_FRAME_RATE_OFFSET),
            15
        );
        assert_eq!(
            read_u32(
                &settings.bytes,
                CEF_BROWSER_SETTINGS_BACKGROUND_COLOR_OFFSET
            ),
            0xFFFF_FFFF
        );
    }

    #[test]
    fn mouse_event_sets_header_pinned_fields() {
        let event = CefMouseEvent::new(12, 34, EVENTFLAG_LEFT_MOUSE_BUTTON);
        assert_eq!(read_i32(&event.bytes, CEF_MOUSE_EVENT_X_OFFSET), 12);
        assert_eq!(read_i32(&event.bytes, CEF_MOUSE_EVENT_Y_OFFSET), 34);
        assert_eq!(
            read_i32(&event.bytes, CEF_MOUSE_EVENT_MODIFIERS_OFFSET),
            EVENTFLAG_LEFT_MOUSE_BUTTON
        );
    }

    #[test]
    fn key_event_sets_header_pinned_fields() {
        let event = CefKeyEvent::new(
            KEYEVENT_CHAR,
            EVENTFLAG_SHIFT_DOWN,
            65,
            65,
            b'A' as u16,
            b'A' as u16,
        );
        assert_eq!(
            read_i32(&event.bytes, CEF_KEY_EVENT_TYPE_OFFSET),
            KEYEVENT_CHAR
        );
        assert_eq!(
            read_i32(&event.bytes, CEF_KEY_EVENT_MODIFIERS_OFFSET),
            EVENTFLAG_SHIFT_DOWN
        );
        assert_eq!(
            read_i32(&event.bytes, CEF_KEY_EVENT_WINDOWS_KEY_CODE_OFFSET),
            65
        );
        assert_eq!(
            read_i32(&event.bytes, CEF_KEY_EVENT_NATIVE_KEY_CODE_OFFSET),
            65
        );
        assert_eq!(
            read_u16(&event.bytes, CEF_KEY_EVENT_CHARACTER_OFFSET),
            b'A' as u16
        );
        assert_eq!(
            read_u16(&event.bytes, CEF_KEY_EVENT_UNMODIFIED_CHARACTER_OFFSET),
            b'A' as u16
        );
        assert_eq!(
            read_i32(&event.bytes, CEF_KEY_EVENT_FOCUS_ON_EDITABLE_FIELD_OFFSET),
            1
        );
    }

    static FOCUS_CALLS: AtomicI32 = AtomicI32::new(0);
    static FOCUS_LAST: AtomicI32 = AtomicI32::new(0);
    static STOP_LOAD_CALLS: AtomicI32 = AtomicI32::new(0);
    static AUDIO_MUTED_CALLS: AtomicI32 = AtomicI32::new(0);
    static AUDIO_MUTED_LAST: AtomicI32 = AtomicI32::new(0);
    static TEST_HOST_PTR: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn test_browser_host(_browser: *mut c_void) -> *mut c_void {
        TEST_HOST_PTR.load(AtomicOrdering::SeqCst) as *mut c_void
    }

    unsafe extern "C" fn record_focus(_host: *mut c_void, focused: c_int) {
        FOCUS_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        FOCUS_LAST.store(focused, AtomicOrdering::SeqCst);
    }

    #[test]
    fn set_host_focus_uses_header_pinned_slot() {
        FOCUS_CALLS.store(0, AtomicOrdering::SeqCst);
        FOCUS_LAST.store(0, AtomicOrdering::SeqCst);
        let mut host = vec![0u8; CEF_BROWSER_HOST_SIZE];
        host[CEF_BROWSER_HOST_SET_FOCUS_OFFSET
            ..CEF_BROWSER_HOST_SET_FOCUS_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(record_focus as *const () as usize).to_ne_bytes());

        set_host_focus(host.as_mut_ptr().cast(), true);

        assert_eq!(FOCUS_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(FOCUS_LAST.load(AtomicOrdering::SeqCst), 1);
    }

    unsafe extern "C" fn record_stop_load(_browser: *mut c_void) {
        STOP_LOAD_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
    }

    #[test]
    fn stop_control_uses_cef_stop_load_slot() {
        STOP_LOAD_CALLS.store(0, AtomicOrdering::SeqCst);
        let mut browser = vec![0u8; CEF_BROWSER_SIZE];
        browser[CEF_BROWSER_STOP_LOAD_OFFSET
            ..CEF_BROWSER_STOP_LOAD_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(record_stop_load as *const () as usize).to_ne_bytes());
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");

        apply_control_frame(browser.as_mut_ptr().cast(), &callbacks, &ControlMsg::Stop);

        assert_eq!(STOP_LOAD_CALLS.load(AtomicOrdering::SeqCst), 1);
    }

    unsafe extern "C" fn record_audio_muted(_host: *mut c_void, muted: c_int) {
        AUDIO_MUTED_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        AUDIO_MUTED_LAST.store(muted, AtomicOrdering::SeqCst);
    }

    #[test]
    fn audio_mute_control_uses_cef_host_audio_slot() {
        AUDIO_MUTED_CALLS.store(0, AtomicOrdering::SeqCst);
        AUDIO_MUTED_LAST.store(0, AtomicOrdering::SeqCst);

        let mut host = vec![0u8; CEF_BROWSER_HOST_SIZE];
        host[CEF_BROWSER_HOST_SET_AUDIO_MUTED_OFFSET
            ..CEF_BROWSER_HOST_SET_AUDIO_MUTED_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(record_audio_muted as *const () as usize).to_ne_bytes());
        TEST_HOST_PTR.store(host.as_mut_ptr() as usize, AtomicOrdering::SeqCst);

        let mut browser = vec![0u8; CEF_BROWSER_SIZE];
        browser[CEF_BROWSER_GET_HOST_OFFSET
            ..CEF_BROWSER_GET_HOST_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(test_browser_host as *const () as usize).to_ne_bytes());
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");

        apply_control_frame(
            browser.as_mut_ptr().cast(),
            &callbacks,
            &ControlMsg::SetAudioMuted { muted: true },
        );

        assert_eq!(AUDIO_MUTED_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(AUDIO_MUTED_LAST.load(AtomicOrdering::SeqCst), 1);

        apply_control_frame(
            browser.as_mut_ptr().cast(),
            &callbacks,
            &ControlMsg::SetAudioMuted { muted: false },
        );

        assert_eq!(AUDIO_MUTED_CALLS.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(AUDIO_MUTED_LAST.load(AtomicOrdering::SeqCst), 0);
    }

    static PRINT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static PDF_CALLS: AtomicUsize = AtomicUsize::new(0);
    static PDF_PATH_LEN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn record_print(_host: *mut c_void) {
        PRINT_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
    }

    unsafe extern "C" fn record_print_to_pdf(
        _host: *mut c_void,
        path: *const c_void,
        settings: *const c_void,
        callback: *mut c_void,
    ) {
        PDF_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        assert!(
            settings.is_null(),
            "default PDF settings are passed as null"
        );
        assert!(!callback.is_null(), "PDF completion callback is retained");
        let path = path.cast::<CefString>();
        assert!(!path.is_null(), "PDF path is a CEF string");
        // SAFETY: the test passes a live CefStringOwned for this call.
        let len = unsafe { (*path).length };
        PDF_PATH_LEN.store(len, AtomicOrdering::SeqCst);
    }

    #[test]
    fn print_controls_use_cef_host_print_slots() {
        PRINT_CALLS.store(0, AtomicOrdering::SeqCst);
        PDF_CALLS.store(0, AtomicOrdering::SeqCst);
        PDF_PATH_LEN.store(0, AtomicOrdering::SeqCst);

        let mut host = vec![0u8; CEF_BROWSER_HOST_SIZE];
        host[CEF_BROWSER_HOST_PRINT_OFFSET
            ..CEF_BROWSER_HOST_PRINT_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(record_print as *const () as usize).to_ne_bytes());
        host[CEF_BROWSER_HOST_PRINT_TO_PDF_OFFSET
            ..CEF_BROWSER_HOST_PRINT_TO_PDF_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(record_print_to_pdf as *const () as usize).to_ne_bytes());
        TEST_HOST_PTR.store(host.as_mut_ptr() as usize, AtomicOrdering::SeqCst);

        let mut browser = vec![0u8; CEF_BROWSER_SIZE];
        browser[CEF_BROWSER_GET_HOST_OFFSET
            ..CEF_BROWSER_GET_HOST_OFFSET + std::mem::size_of::<usize>()]
            .copy_from_slice(&(test_browser_host as *const () as usize).to_ne_bytes());
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");

        apply_control_frame(
            browser.as_mut_ptr().cast(),
            &callbacks,
            &ControlMsg::PrintPage,
        );
        assert_eq!(PRINT_CALLS.load(AtomicOrdering::SeqCst), 1);

        apply_control_frame(
            browser.as_mut_ptr().cast(),
            &callbacks,
            &ControlMsg::SavePdf {
                path: "/tmp/mde-browser-page.pdf".to_owned(),
            },
        );
        assert_eq!(PDF_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            PDF_PATH_LEN.load(AtomicOrdering::SeqCst),
            "/tmp/mde-browser-page.pdf".encode_utf16().count()
        );
    }

    #[test]
    fn print_handler_returns_letter_paper_and_cancels_interactive_jobs() {
        assert_eq!(
            unsafe { get_pdf_paper_size(ptr::null_mut(), ptr::null_mut(), 72) },
            CefSize {
                width: 612,
                height: 792
            }
        );
        assert_eq!(
            unsafe { on_print_dialog(ptr::null_mut(), ptr::null_mut(), 0, ptr::null_mut()) },
            0
        );
        assert_eq!(
            unsafe {
                on_print_job(
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut(),
                )
            },
            0
        );
    }

    #[test]
    fn input_mapping_helpers_cover_wire_keys_and_modifiers() {
        assert_eq!(windows_key_code(KeyCode::Enter), Some(13));
        assert_eq!(windows_key_code(KeyCode::A), Some(65));
        assert_eq!(windows_key_code(KeyCode::Num9), Some(57));
        assert_eq!(windows_key_code(KeyCode::F12), Some(123));
        assert_eq!(
            cef_modifiers(Modifiers(
                Modifiers::CTRL | Modifiers::SHIFT | Modifiers::COMMAND
            )),
            EVENTFLAG_CONTROL_DOWN | EVENTFLAG_SHIFT_DOWN | EVENTFLAG_COMMAND_DOWN
        );
        assert_eq!(f32_to_i32(12.4), 12);
        assert_eq!(f32_to_i32(12.5), 13);
        assert_eq!(f32_to_i32(f32::NAN), 0);
    }

    #[test]
    fn cosmetic_filter_script_installs_and_clears_the_style_element() {
        let script = cosmetic_filter_script(r#"#ad, .sponsor::before { content: "\"x\""; }"#);
        assert!(script.contains("mde-cef-cosmetic-style"));
        assert!(script.contains("document.createElement('style')"));
        assert!(script.contains(r#"#ad, .sponsor::before { content: \"\\\"x\\\"\"; }"#));

        let clear = cosmetic_filter_script("");
        assert!(clear.contains("if(el)el.remove();return;"));
    }

    #[test]
    fn page_zoom_and_find_scripts_are_bounded_and_escaped() {
        assert!(page_zoom_script(125).contains("zoom='125%'"));
        assert!(page_zoom_script(5).contains("zoom='25%'"));
        assert!(page_zoom_script(900).contains("zoom='500%'"));

        let forward = find_in_page_script("mesh \"ops\"", false);
        assert!(forward.contains(r#"window.find("mesh \"ops\"",false,false"#));
        let backward = find_in_page_script("mesh", true);
        assert!(backward.contains(r#"window.find("mesh",false,true"#));
        assert!(clear_find_script().contains("removeAllRanges"));
    }

    #[test]
    fn force_dark_script_installs_and_clears_bounded_style() {
        let enable = force_dark_script(true);
        assert!(enable.contains("mde-cef-force-dark-style"));
        assert!(enable.contains("colorScheme='dark'"));
        assert!(enable.contains("img, video, canvas"));
        assert!(
            !enable.contains("<script"),
            "force-dark is injected as style text only"
        );

        let disable = force_dark_script(false);
        assert!(disable.contains("if(el)el.remove()"));
        assert!(disable.contains("colorScheme=''"));
    }

    #[test]
    fn reader_mode_script_installs_and_clears_bounded_style() {
        let enable = reader_mode_script(true);
        assert!(enable.contains("mde-cef-reader-style"));
        assert!(enable.contains("mde-reader-mode"));
        assert!(enable.contains("max-width: 76ch"));
        assert!(
            !enable.contains("<script"),
            "reader mode is injected as style text only"
        );

        let disable = reader_mode_script(false);
        assert!(disable.contains("if(el)el.remove()"));
        assert!(disable.contains("classList.remove"));
    }

    #[test]
    fn user_agent_override_script_installs_and_clears_page_visible_ua() {
        let enable = user_agent_override_script("Mozilla/5.0 MDE-Test");
        assert!(enable.contains("Navigator.prototype"));
        assert!(enable.contains("userAgent"));
        assert!(enable.contains("MDE-Test"));
        assert!(
            !enable.contains("</script>"),
            "UA override is injected as bounded script text only"
        );

        let disable = user_agent_override_script("");
        assert!(disable.contains("__mdeUserAgentOverride"));
        assert!(disable.contains("delete"));
    }

    #[test]
    fn device_profile_script_installs_and_clears_page_visible_device_metadata() {
        let enable = device_profile_script("phone", 390, 844, 300, true);
        assert!(enable.contains("mdeDeviceProfile"));
        assert!(enable.contains("innerWidth"));
        assert!(enable.contains("maxTouchPoints"));
        assert!(enable.contains("mde-device-profile-viewport"));
        assert!(enable.contains("width:390"));
        assert!(
            !enable.contains("</script>"),
            "device profile is injected as bounded script text only"
        );

        let disable = device_profile_script("default", 0, 0, 100, false);
        assert!(disable.contains("__mdeDeviceProfile"));
        assert!(disable.contains("delete"));
    }

    #[test]
    fn userscript_library_script_runs_and_cleans_the_bundle() {
        let enable = userscript_library_script(
            true,
            "document.documentElement.dataset.curatedUserscript='npr';",
        );
        assert!(enable.contains("mdeBrowserUserscripts"));
        assert!(enable.contains("curatedUserscript='npr'"));
        assert!(enable.contains("try{"));

        let disable = userscript_library_script(false, "");
        assert!(disable.contains("mde-browser-userscript-style"));
        assert!(disable.contains("__mdeBrowserUserScriptsObserver"));
        assert!(disable.contains("delete document.documentElement.dataset.mdeBrowserUserscripts"));
    }

    #[test]
    fn spellcheck_highlight_script_marks_and_clears_words() {
        let script =
            spellcheck_highlight_script(&["wrold".to_owned(), "mesh?".to_owned(), "x".to_owned()]);
        assert!(script.contains("mde-browser-spell-miss"));
        assert!(script.contains("mde-browser-spellcheck-style"));
        assert!(script.contains("\"wrold\""));
        assert!(script.contains("\"mesh?\""));
        assert!(!script.contains("\"x\""));

        let clear = spellcheck_highlight_script(&[]);
        assert!(clear.contains("delete document.documentElement.dataset.mdeBrowserSpellcheck"));
    }

    #[test]
    fn spellcheck_correction_script_replaces_mark_or_text() {
        let script = spellcheck_correction_script("wrold", "world");
        assert!(script.contains("mde-browser-spell-miss"));
        assert!(script.contains(r#"word="wrold""#));
        assert!(script.contains(r#"replacement="world""#));
        assert!(script.contains("replaceWith(document.createTextNode(replacement))"));
        assert!(script.contains("createTreeWalker"));
        assert!(script.contains("var targetOccurrence=0"));
        assert!(script.contains("var replaceAll=targetOccurrence<0"));

        let all = spellcheck_correction_all_script("wrold", "world");
        assert!(all.contains("var targetOccurrence=-1"));
        assert!(all.contains("while((node=walker.nextNode())&&total<512)"));
        assert!(all.contains("text.replace(re,function(m)"));

        let indexed = spellcheck_correction_at_script("wrold", "world", 3);
        assert!(indexed.contains("var targetOccurrence=3"));
        assert!(indexed.contains("if(total===targetOccurrence)"));
        assert!(indexed.contains("if(markMatches>0&&!replaceAll)return"));

        assert_eq!(spellcheck_correction_script("", "world"), "()=>{}");
        assert_eq!(spellcheck_correction_script("wrold", ""), "()=>{}");
        assert_eq!(spellcheck_correction_all_script("", "world"), "()=>{}");
        assert_eq!(spellcheck_correction_at_script("", "world", 1), "()=>{}");
    }

    #[test]
    fn page_text_beacon_script_is_bounded_and_decodable() {
        let script = page_text_beacon_script(42, 200_000);
        assert!(script.contains("cap=8192"));
        assert!(script.contains("innerText||root.textContent"));
        assert!(script.contains("encodeURIComponent(text)"));
        assert!(script.contains("https://mde-page-text.invalid/capture/42?text="));

        assert_eq!(
            decode_page_text_beacon(
                "https://mde-page-text.invalid/capture/42?text=hello%20w%C3%B8rld"
            ),
            Some((42, "hello wørld".to_owned()))
        );
        assert_eq!(
            decode_page_text_beacon("mde-page-text://capture/42?text=hello%20w%C3%B8rld"),
            Some((42, "hello wørld".to_owned()))
        );
        assert_eq!(percent_decode("%zz+ok"), "%zz+ok");
        assert_eq!(clamp_utf8("abé", 3), "ab");
        assert_eq!(decode_page_text_beacon("https://example.com/"), None);
    }

    #[test]
    fn page_scrape_beacon_script_is_bounded_and_decodable() {
        let script = page_scrape_beacon_script(43, 200_000, 400, 300);
        assert!(script.contains("textCap=Math.min(32768,16384)"));
        assert!(script.contains("articleCap=8192"));
        assert!(script.contains("linkCap=128"));
        assert!(script.contains("headingCap=64"));
        assert!(script.contains("querySelectorAll('a[href]')"));
        assert!(script.contains("querySelectorAll('h1,h2,h3,h4,h5,h6')"));
        assert!(script.contains("querySelectorAll('article,main,[role=main]')"));
        assert!(script.contains("link[rel~=\"canonical\"][href]"));
        assert!(script.contains("meta[name=\"description\" i][content]"));
        assert!(script.contains("links=links.slice(0,32)"));
        assert!(script.contains("https://mde-page-scrape.invalid/capture/43?body="));

        assert_eq!(
            decode_page_scrape_beacon(
                "https://mde-page-scrape.invalid/capture/43?body=%7B%22text%22%3A%22hello%22%7D"
            ),
            Some((43, r#"{"text":"hello"}"#.to_owned()))
        );
        assert_eq!(decode_page_scrape_beacon("https://example.com/"), None);
    }

    #[test]
    fn passkey_bridge_script_uses_bounded_beacon_metadata() {
        let script = passkey_bridge_script();
        assert!(script.contains("navigator.credentials"));
        assert!(script.contains("creds.create=function"));
        assert!(script.contains("creds.get=function"));
        // browser-4: capture the originals so non-publicKey requests fall
        // through instead of being hijacked into a passkey ceremony.
        assert!(script.contains("origCreate"));
        assert!(script.contains("origGet"));
        // security-2(a): only dispatch behind a real user gesture (transient
        // activation), and thread the presence signal to the daemon.
        assert!(script.contains("hasUserGesture"));
        assert!(script.contains("navigator.userActivation"));
        assert!(script.contains("user_present"));
        assert!(script.contains(CEF_PASSKEY_BEACON_PREFIX));
        assert!(script.contains("challenge_b64url"));
        assert!(script.contains("allow_credentials"));
        assert!(script.contains("client_request_id"));
        assert!(script.contains("__mdeBrowserPasskeyComplete"));
        assert!(script.contains("pending.resolve"));
        assert!(script.contains("public_key_spki_der_b64url"));
        assert!(script.contains("getClientExtensionResults"));
        assert!(script.contains("isUserVerifyingPlatformAuthenticatorAvailable"));
        assert!(script.contains("isConditionalMediationAvailable"));
        assert!(script.contains("Object.setPrototypeOf"));
        assert!(script.contains("getTransports"));
        assert!(script.contains("toJSON"));

        let complete = passkey_complete_script(r#"{"client_request_id":"pk-1"}"#);
        assert!(complete.contains("__mdeBrowserPasskeyComplete"));
        assert!(complete.contains("JSON.parse"));

        let body = decode_passkey_beacon(
            "https://mde-passkey.invalid/request/?body=%7B%22ceremony%22%3A%22get%22%7D",
        )
        .expect("passkey beacon");
        assert_eq!(body, r#"{"ceremony":"get"}"#);
        assert_eq!(decode_passkey_beacon("https://example.test/"), None);
    }

    #[test]
    fn passkey_shim_feature_detects_credential_type_and_gates_on_a_gesture() {
        // browser-4: a plain `navigator.credentials.get({password:true})`
        // (password-manager autofill), `{federated:...}`, or WebOTP `{otp:...}`
        // request has no `publicKey` member, so the shim must NOT convert it into
        // a passkey ceremony — it calls through to the captured original instead.
        let script = passkey_bridge_script();
        // The publicKey guard precedes the passkey `enqueue`, and the else-branch
        // is a call-through to the original implementation.
        assert!(script.contains("if(options&&options.publicKey)return enqueue('get',options)"));
        assert!(script.contains("if(options&&options.publicKey)return enqueue('create',options)"));
        assert!(script.contains("if(origGet)return origGet(options)"));
        assert!(script.contains("if(origCreate)return origCreate(options)"));
        // The `enqueue` path (publicKey only) is the sole caller of the passkey
        // beacon queue, so a non-publicKey get can never reach it. Prove the
        // guard sits before the queue push, not after.
        let get_guard = script
            .find("if(options&&options.publicKey)return enqueue('get',options)")
            .expect("get guard present");
        let enqueue_impl = script
            .find("function enqueue(kind,options)")
            .expect("enqueue defined");
        assert!(
            enqueue_impl < get_guard,
            "enqueue is defined before the guarded call site"
        );

        // security-2(a): the ceremony only dispatches with transient activation,
        // and a gesture-less call is rejected rather than auto-signed.
        assert!(script.contains("if(ua&&typeof ua.isActive==='boolean')return ua.isActive"));
        assert!(script.contains("out.user_present=hasUserGesture()"));
        assert!(script.contains(
            "if(!item.user_present)return Promise.reject(new DOMException('Passkey ceremony requires a user gesture','NotAllowedError'))"
        ));
    }

    #[test]
    fn webrtc_block_script_removes_the_reachable_webrtc_surface() {
        // `--disable-webrtc` (cef_init.rs) is confirmed non-functional (see
        // its doc comment); this renderer-level shim is the real mitigation,
        // so pin exactly which JS-reachable entry points it removes.
        let script = webrtc_block_script();
        assert!(script.contains("delete w.RTCPeerConnection"));
        assert!(script.contains("delete w.webkitRTCPeerConnection"));
        assert!(script.contains("delete w.RTCDataChannel"));
        assert!(script.contains("delete w.RTCSessionDescription"));
        assert!(script.contains("delete w.RTCIceCandidate"));
        assert!(script.contains("w.MediaDevices.prototype.getUserMedia"));
        assert!(script.contains("w.MediaDevices.prototype.getDisplayMedia"));
        assert!(script.contains("w.navigator.mediaDevices.getUserMedia"));
        assert!(script.contains("w.navigator.mediaDevices.getDisplayMedia"));
        assert!(script.contains("delete w.navigator.getUserMedia"));
        assert!(script.contains("delete w.navigator.webkitGetUserMedia"));
        assert!(script.contains("delete w.navigator.mozGetUserMedia"));
        assert!(
            !script.contains("<script"),
            "webrtc block runs as a direct IIFE, not injected markup"
        );
        // Every deletion is individually try/catch-guarded so one already-gone
        // global (e.g. re-running after the page itself deleted something)
        // cannot abort the rest of the shim.
        assert_eq!(script.matches("catch(_e){}").count(), 14);
    }

    #[test]
    fn webrtc_block_script_covers_subframes_not_just_the_main_frame() {
        // browser-3: a page's trivial bypass was a child iframe (its own
        // unpatched JS context). The shim now recurses through `frames` and
        // re-sweeps on DOM mutation, so a same-origin child/nested/late-inserted
        // iframe is stripped too — not only `get_main_frame`.
        let script = webrtc_block_script();
        // The strip logic is parameterised over a target window `w` and applied
        // to every reachable frame, rather than hard-coded to `window`.
        assert!(script.contains("function strip(w)"));
        assert!(script.contains("function sweep(w)"));
        // Recurse across child frames.
        assert!(script.contains("w.frames"));
        assert!(script.contains("sweep(cw)"));
        // Cover frames inserted after the first pass.
        assert!(script.contains("MutationObserver"));
        assert!(script.contains("childList:true,subtree:true"));
        // The entry point sweeps from the top window (which recurses down).
        assert!(script.contains("sweep(window)"));
    }

    #[test]
    fn pump_interval_backs_off_when_idle_but_stays_fast_while_active() {
        // perf-6: awaiting the first paint is always active regardless of the
        // idle clock, so initial load latency is never regressed.
        assert_eq!(pump_interval(Duration::from_secs(30), true), PUMP_ACTIVE);
        // Recent activity (paint/frame/nav within the grace window) stays fast.
        assert_eq!(pump_interval(Duration::ZERO, false), PUMP_ACTIVE);
        assert_eq!(pump_interval(PUMP_IDLE_AFTER / 2, false), PUMP_ACTIVE);
        // Sustained quiet backs off so an idle tab stops spinning at 125 Hz.
        assert_eq!(pump_interval(PUMP_IDLE_AFTER, false), PUMP_IDLE);
        assert_eq!(pump_interval(Duration::from_secs(5), false), PUMP_IDLE);
        // The idle interval is a real, substantial back-off from the active spin.
        assert!(PUMP_IDLE >= PUMP_ACTIVE * 10);
    }

    #[test]
    fn shim_injector_injects_once_per_navigation_not_on_a_timer() {
        // browser-8: the shims re-inject when the navigation generation advances,
        // never on a bare wall-clock timer once the context is stable.
        let mut injector = ShimInjector::new();
        let t0 = Instant::now();
        // First sight of a generation always injects once.
        assert!(injector.should_inject(0, false, t0));
        // Same generation, stable (not settling): no re-injection even much later.
        assert!(!injector.should_inject(0, false, t0 + Duration::from_secs(10)));
        assert!(!injector.should_inject(0, false, t0 + Duration::from_secs(60)));
        // A real navigation (generation advanced) injects again.
        assert!(injector.should_inject(1, false, t0 + Duration::from_secs(61)));
        assert!(!injector.should_inject(1, false, t0 + Duration::from_secs(62)));
    }

    #[test]
    fn shim_injector_reinjects_through_a_settling_document_then_stops() {
        // A freshly-navigated document that is still settling gets a bounded
        // re-inject (covers a slow commit under an ABI with no load callback),
        // but the cadence is throttled to SETTLE_INTERVAL, not every tick.
        let mut injector = ShimInjector::new();
        let t0 = Instant::now();
        // A new generation injects once.
        assert!(injector.should_inject(2, true, t0));
        // Immediately again while settling: throttled, not a per-tick spin.
        assert!(!injector.should_inject(2, true, t0 + Duration::from_millis(10)));
        assert!(!injector.should_inject(2, true, t0 + ShimInjector::SETTLE_INTERVAL / 2));
        // Once the settle interval elapses, re-inject to cover the commit.
        assert!(injector.should_inject(2, true, t0 + ShimInjector::SETTLE_INTERVAL));
        // When the same generation stops settling, injection stops entirely.
        assert!(!injector.should_inject(2, false, t0 + Duration::from_secs(30)));
    }

    #[test]
    fn passkey_bridge_exposes_reusable_drain_and_heartbeat_is_lightweight() {
        // browser-8: the heavy bridge shim installs a reusable drain closure once
        // per context; the heartbeat script just invokes it instead of
        // recompiling the whole shim every 250 ms.
        let install = passkey_bridge_script();
        assert!(install.contains("window.__mdeBrowserPasskeyDrain=function"));
        assert!(install.contains("__mdeBrowserPasskeyQueue"));

        let drain = passkey_drain_script();
        assert!(drain.contains("window.__mdeBrowserPasskeyDrain"));
        // The heartbeat is only the invocation, not the shim: it must not carry
        // the credential-override installation that only needs to happen once.
        assert!(!drain.contains("creds.get=function"));
        assert!(!drain.contains("navigator.credentials"));
        // Materially smaller than the shim it replaces on the timer path.
        assert!(drain.len() * 4 < install.len());
    }

    #[test]
    fn wait_for_readable_returns_promptly_when_the_fd_is_readable() {
        use std::io::Write;
        let (a, mut b) = UnixStream::pair().expect("socketpair");
        a.set_nonblocking(true).expect("nonblocking");
        b.write_all(b"x").expect("write");
        // A readable fd must not block for the full timeout: poll returns at once.
        let started = Instant::now();
        wait_for_readable(a.as_raw_fd(), Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn record_navigation_advances_the_generation_seen_by_the_pump() {
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");
        assert_eq!(callbacks.navigations(), 0);
        callbacks.state.record_navigation();
        callbacks.state.record_navigation();
        assert_eq!(callbacks.navigations(), 2);
    }

    #[test]
    fn js_string_literal_escapes_script_sensitive_characters() {
        assert_eq!(js_string_literal("a\"b\\c\n\r\t"), r#""a\"b\\c\n\r\t""#);
        assert_eq!(js_string_literal("nul\u{1}"), r#""nul\u0001""#);
    }

    #[test]
    fn callback_registry_returns_lifespan_and_render_peers() {
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");
        let life = unsafe { get_life_span_handler(callbacks.client_ptr()) };
        let render = unsafe { get_render_handler(callbacks.client_ptr()) };
        let request = unsafe { get_request_handler(callbacks.client_ptr()) };
        assert!(!life.is_null());
        assert!(!render.is_null());
        assert!(!request.is_null());
        assert_ne!(life, render);
        assert_ne!(render, request);
    }

    #[test]
    fn callbacks_record_created_and_view_paint() {
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");
        let life = unsafe { get_life_span_handler(callbacks.client_ptr()) };
        let render = unsafe { get_render_handler(callbacks.client_ptr()) };
        unsafe { on_after_created(life, ptr::null_mut()) };
        let mut rect = CefRect {
            x: -1,
            y: -1,
            width: 0,
            height: 0,
        };
        unsafe { get_view_rect(render, ptr::null_mut(), &mut rect) };
        assert_eq!((rect.x, rect.y, rect.width, rect.height), (0, 0, 320, 200));
        let pixel = 0_u32;
        unsafe {
            on_paint(
                render,
                ptr::null_mut(),
                PET_VIEW,
                0,
                ptr::null(),
                (&pixel as *const u32).cast(),
                320,
                200,
            )
        };
        assert_eq!(callbacks.created(), 1);
        assert_eq!(callbacks.paints(), 1);
        assert_eq!(callbacks.last_paint_width(), 320);
        assert_eq!(callbacks.last_paint_height(), 200);
    }

    #[test]
    fn resize_control_changes_the_next_view_rect() {
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");
        let render = unsafe { get_render_handler(callbacks.client_ptr()) };
        callbacks.resize(640, 480);
        let mut rect = CefRect {
            x: -1,
            y: -1,
            width: 0,
            height: 0,
        };
        unsafe { get_view_rect(render, ptr::null_mut(), &mut rect) };
        assert_eq!((rect.x, rect.y, rect.width, rect.height), (0, 0, 640, 480));
    }

    #[test]
    fn callbacks_publish_paint_to_the_bookmarks_frame_sink() {
        use crate::sock::{recv, RecvOutcome};
        use crate::wire::{take_frame, EventMsg};

        let (helper, shell) = UnixStream::pair().expect("socketpair");
        let callbacks =
            CefBrowserCallbacks::new(2, 2, Some(&helper), noop_userfree_free).expect("callbacks");

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("attach recv") else {
            panic!("expected attach")
        };
        assert_eq!(fds.len(), 1);
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        assert_eq!(
            EventMsg::decode(&payload).expect("event"),
            EventMsg::AttachFrame
        );

        let render = unsafe { get_render_handler(callbacks.client_ptr()) };
        unsafe {
            on_paint(
                render,
                ptr::null_mut(),
                PET_VIEW,
                0,
                ptr::null(),
                [0x7a_u8; 2 * 2 * 4].as_ptr().cast(),
                2,
                2,
            )
        };

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("paint recv") else {
            panic!("expected paint")
        };
        assert!(fds.is_empty());
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        let EventMsg::PaintReady { seq } = EventMsg::decode(&payload).expect("event") else {
            panic!("expected paint ready")
        };
        assert_eq!(seq % 2, 0);
        assert_eq!(callbacks.paints(), 1);
    }

    #[test]
    fn pdf_completion_callback_publishes_helper_event() {
        use crate::sock::{recv, RecvOutcome};
        use crate::wire::{take_frame, EventMsg};

        let (helper, shell) = UnixStream::pair().expect("socketpair");
        let callbacks =
            CefBrowserCallbacks::new(2, 2, Some(&helper), noop_userfree_free).expect("callbacks");

        let RecvOutcome::Data { .. } = recv(&shell).expect("attach recv") else {
            panic!("expected attach")
        };
        let callback = callbacks.retain_pdf_callback();
        let path = unique_test_pdf_path("cef-finished-ok");
        std::fs::write(&path, b"%PDF-1.7\n% test\n").expect("pdf fixture");
        let path_text = path.to_string_lossy().into_owned();
        let cef_path = CefStringOwned::new(&path_text).expect("pdf path");

        unsafe { on_pdf_print_finished(callback, cef_path.as_ptr(), 1) };

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("pdf recv") else {
            panic!("expected pdf event")
        };
        assert!(fds.is_empty());
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        assert_eq!(
            EventMsg::decode(&payload).expect("event"),
            EventMsg::PdfSaved {
                path: path_text,
                ok: true,
            }
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pdf_completion_callback_rejects_missing_pdf_output() {
        use crate::sock::{recv, RecvOutcome};
        use crate::wire::{take_frame, EventMsg};

        let (helper, shell) = UnixStream::pair().expect("socketpair");
        let callbacks =
            CefBrowserCallbacks::new(2, 2, Some(&helper), noop_userfree_free).expect("callbacks");
        let RecvOutcome::Data { .. } = recv(&shell).expect("attach recv") else {
            panic!("expected attach")
        };
        let callback = callbacks.retain_pdf_callback();
        let path = unique_test_pdf_path("cef-finished-missing");
        let path_text = path.to_string_lossy().into_owned();
        let cef_path = CefStringOwned::new(&path_text).expect("pdf path");

        unsafe { on_pdf_print_finished(callback, cef_path.as_ptr(), 1) };

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("pdf recv") else {
            panic!("expected pdf event")
        };
        assert!(fds.is_empty());
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        assert_eq!(
            EventMsg::decode(&payload).expect("event"),
            EventMsg::PdfSaved {
                path: path_text,
                ok: false,
            }
        );
    }

    #[test]
    fn request_handler_registry_returns_resource_request_peer() {
        let callbacks =
            CefBrowserCallbacks::new(320, 200, None, noop_userfree_free).expect("callbacks");
        let request = unsafe { get_request_handler(callbacks.client_ptr()) };
        let mut disable_default_handling = -1;
        let resource_request = unsafe {
            get_resource_request_handler(
                request,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                0,
                ptr::null(),
                &mut disable_default_handling,
            )
        };
        assert_eq!(disable_default_handling, 0);
        assert!(!resource_request.is_null());
    }

    #[test]
    fn resource_verdict_transport_uses_async_cef_callback() {
        use crate::sock::{recv, RecvOutcome};
        use crate::wire::{take_frame, EventMsg};

        let (helper, shell) = UnixStream::pair().expect("socketpair");
        let callbacks =
            CefBrowserCallbacks::new(2, 2, Some(&helper), noop_userfree_free).expect("callbacks");
        let RecvOutcome::Data { .. } = recv(&shell).expect("attach recv") else {
            panic!("expected attach")
        };

        let cef_callback = TestCefCallback::new();
        let rv = callbacks.state.begin_resource_request(
            "https://www.google-analytics.com/collect".to_owned(),
            cef_callback.as_mut_ptr(),
        );
        assert_eq!(rv, RV_CONTINUE_ASYNC);

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("resource request recv") else {
            panic!("expected resource request")
        };
        assert!(fds.is_empty());
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        assert_eq!(
            EventMsg::decode(&payload).expect("event"),
            EventMsg::ResourceRequest {
                id: 1,
                url: "https://www.google-analytics.com/collect".to_owned(),
                resource: RESOURCE_OTHER,
            }
        );

        callbacks.apply_resource_verdict(1, false);
        assert_eq!(cef_callback.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(cef_callback.continued.load(Ordering::SeqCst), 0);
        assert_eq!(cef_callback.add_refs.load(Ordering::SeqCst), 1);
        assert_eq!(cef_callback.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn page_text_beacon_is_intercepted_and_published_without_adfilter_roundtrip() {
        use crate::sock::{recv, RecvOutcome};
        use crate::wire::{take_frame, EventMsg};

        let (helper, shell) = UnixStream::pair().expect("socketpair");
        let callbacks =
            CefBrowserCallbacks::new(2, 2, Some(&helper), noop_userfree_free).expect("callbacks");
        let RecvOutcome::Data { .. } = recv(&shell).expect("attach recv") else {
            panic!("expected attach")
        };

        let cef_callback = TestCefCallback::new();
        let rv = callbacks.state.begin_resource_request(
            "mde-page-text://capture/77?text=hello%20page".to_owned(),
            cef_callback.as_mut_ptr(),
        );
        assert_eq!(rv, RV_CANCEL);

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("page text recv") else {
            panic!("expected page text")
        };
        assert!(fds.is_empty());
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        assert_eq!(
            EventMsg::decode(&payload).expect("event"),
            EventMsg::PageText {
                id: 77,
                text: "hello page".to_owned(),
            }
        );
        assert_eq!(cef_callback.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(cef_callback.continued.load(Ordering::SeqCst), 0);
        assert_eq!(cef_callback.add_refs.load(Ordering::SeqCst), 0);
        assert_eq!(cef_callback.releases.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn page_scrape_beacon_is_intercepted_and_published_without_adfilter_roundtrip() {
        use crate::sock::{recv, RecvOutcome};
        use crate::wire::{take_frame, EventMsg};

        let (helper, shell) = UnixStream::pair().expect("socketpair");
        let callbacks =
            CefBrowserCallbacks::new(2, 2, Some(&helper), noop_userfree_free).expect("callbacks");
        let RecvOutcome::Data { .. } = recv(&shell).expect("attach recv") else {
            panic!("expected attach")
        };

        let cef_callback = TestCefCallback::new();
        let rv = callbacks.state.begin_resource_request(
            "https://mde-page-scrape.invalid/capture/88?body=%7B%22text%22%3A%22hello%22%2C%22links%22%3A%5B%5D%7D".to_owned(),
            cef_callback.as_mut_ptr(),
        );
        assert_eq!(rv, RV_CANCEL);

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("page scrape recv") else {
            panic!("expected page scrape")
        };
        assert!(fds.is_empty());
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        assert_eq!(
            EventMsg::decode(&payload).expect("event"),
            EventMsg::PageScrape {
                id: 88,
                body: r#"{"text":"hello","links":[]}"#.to_owned(),
            }
        );
        assert_eq!(cef_callback.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(cef_callback.continued.load(Ordering::SeqCst), 0);
        assert_eq!(cef_callback.add_refs.load(Ordering::SeqCst), 0);
        assert_eq!(cef_callback.releases.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn passkey_beacon_is_intercepted_and_published_without_adfilter_roundtrip() {
        use crate::sock::{recv, RecvOutcome};
        use crate::wire::{take_frame, EventMsg};

        let (helper, shell) = UnixStream::pair().expect("socketpair");
        let callbacks =
            CefBrowserCallbacks::new(2, 2, Some(&helper), noop_userfree_free).expect("callbacks");
        let RecvOutcome::Data { .. } = recv(&shell).expect("attach recv") else {
            panic!("expected attach")
        };

        let cef_callback = TestCefCallback::new();
        let rv = callbacks.state.begin_resource_request(
            "https://mde-passkey.invalid/request/?body=%7B%22ceremony%22%3A%22get%22%7D".to_owned(),
            cef_callback.as_mut_ptr(),
        );
        assert_eq!(rv, RV_CANCEL);

        let RecvOutcome::Data { bytes, fds } = recv(&shell).expect("passkey recv") else {
            panic!("expected passkey event")
        };
        assert!(fds.is_empty());
        let mut bytes = bytes;
        let payload = take_frame(&mut bytes).expect("frame").expect("payload");
        assert_eq!(
            EventMsg::decode(&payload).expect("event"),
            EventMsg::PasskeyRequest {
                body: r#"{"ceremony":"get"}"#.to_owned(),
            }
        );
        assert_eq!(cef_callback.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(cef_callback.continued.load(Ordering::SeqCst), 0);
        assert_eq!(cef_callback.add_refs.load(Ordering::SeqCst), 0);
        assert_eq!(cef_callback.releases.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn text_probe_status_and_event_drain_are_operator_readable() {
        let probe = CefTextProbe {
            browser: CefBrowserProbe {
                url: "https://example.test/".to_owned(),
                width: 1024,
                height: 768,
                created: 1,
                paints: 3,
                last_paint_width: 1024,
                last_paint_height: 768,
            },
            expected: "mde-extension-smoke-ok".to_owned(),
            text_bytes: 128,
        };
        let line = probe.status_line();
        assert!(line.contains("CEF_TEXT_PROBE_READY"));
        assert!(line.contains("https://example.test/"));
        assert!(line.contains("marker_bytes=22"));
        assert!(line.contains("text_bytes=128"));

        let mut bytes = wire::frame(
            &EventMsg::PageText {
                id: 99,
                text: "ignored".to_owned(),
            }
            .encode(),
        );
        bytes.extend(wire::frame(
            &EventMsg::PageText {
                id: 42,
                text: "mde-extension-smoke-ok".to_owned(),
            }
            .encode(),
        ));
        assert_eq!(
            drain_page_text_events(&mut bytes, 42),
            Some("mde-extension-smoke-ok".to_owned())
        );
        assert!(bytes.is_empty());

        let err = CefBrowserError::TextProbeMissing {
            created: 1,
            paints: 2,
            text_bytes: 12,
        }
        .to_string();
        assert!(err.contains("CEF text marker"));
        assert!(err.contains("text_bytes=12"));
    }

    #[test]
    fn browser_probe_status_line_is_operator_readable() {
        let line = CefBrowserProbe {
            url: "https://example.com/".to_owned(),
            width: 800,
            height: 600,
            created: 1,
            paints: 2,
            last_paint_width: 800,
            last_paint_height: 600,
        }
        .status_line();
        assert!(line.contains("CEF_BROWSER_PAINT_READY"));
        assert!(line.contains("https://example.com/"));
        assert!(line.contains("last_paint=800x600"));
    }

    fn read_usize(bytes: &[u8], offset: usize) -> usize {
        let mut data = [0; std::mem::size_of::<usize>()];
        data.copy_from_slice(&bytes[offset..offset + std::mem::size_of::<usize>()]);
        usize::from_ne_bytes(data)
    }

    fn read_i32(bytes: &[u8], offset: usize) -> i32 {
        let mut data = [0; std::mem::size_of::<i32>()];
        data.copy_from_slice(&bytes[offset..offset + std::mem::size_of::<i32>()]);
        i32::from_ne_bytes(data)
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        let mut data = [0; std::mem::size_of::<u32>()];
        data.copy_from_slice(&bytes[offset..offset + std::mem::size_of::<u32>()]);
        u32::from_ne_bytes(data)
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        let mut data = [0; std::mem::size_of::<u16>()];
        data.copy_from_slice(&bytes[offset..offset + std::mem::size_of::<u16>()]);
        u16::from_ne_bytes(data)
    }

    unsafe extern "C" fn noop_userfree_free(_string: *mut c_void) {}

    struct TestCefCallback {
        block: CefCallbackBlock<CEF_CALLBACK_SIZE>,
        add_refs: Box<AtomicUsize>,
        releases: Box<AtomicUsize>,
        continued: Box<AtomicUsize>,
        cancelled: Box<AtomicUsize>,
    }

    impl TestCefCallback {
        fn new() -> Box<Self> {
            let add_refs = Box::new(AtomicUsize::new(0));
            let releases = Box::new(AtomicUsize::new(0));
            let continued = Box::new(AtomicUsize::new(0));
            let cancelled = Box::new(AtomicUsize::new(0));
            let mut block = CefCallbackBlock::new(CEF_CALLBACK_SIZE);
            block.put_fn(BASE_ADD_REF_OFFSET, fn_ptr(test_add_ref as *const ()));
            block.put_fn(BASE_RELEASE_OFFSET, fn_ptr(test_release as *const ()));
            block.put_fn(CEF_CALLBACK_CONT_OFFSET, fn_ptr(test_cont as *const ()));
            block.put_fn(CEF_CALLBACK_CANCEL_OFFSET, fn_ptr(test_cancel as *const ()));
            let value = Box::new(Self {
                block,
                add_refs,
                releases,
                continued,
                cancelled,
            });
            value.install_state_pointers();
            value
        }

        fn as_mut_ptr(&self) -> *mut c_void {
            self.block.as_mut_ptr()
        }

        fn install_state_pointers(&self) {
            let state = TestCefCallbackState {
                add_refs: self.add_refs.as_ref() as *const AtomicUsize as usize,
                releases: self.releases.as_ref() as *const AtomicUsize as usize,
                continued: self.continued.as_ref() as *const AtomicUsize as usize,
                cancelled: self.cancelled.as_ref() as *const AtomicUsize as usize,
            };
            test_callback_registry()
                .lock()
                .expect("test callback registry")
                .insert(self.as_mut_ptr() as usize, state);
        }
    }

    impl Drop for TestCefCallback {
        fn drop(&mut self) {
            let _ = test_callback_registry()
                .lock()
                .map(|mut registry| registry.remove(&(self.as_mut_ptr() as usize)));
        }
    }

    #[derive(Clone, Copy)]
    struct TestCefCallbackState {
        add_refs: usize,
        releases: usize,
        continued: usize,
        cancelled: usize,
    }

    fn test_callback_registry() -> &'static Mutex<HashMap<usize, TestCefCallbackState>> {
        static REGISTRY: OnceLock<Mutex<HashMap<usize, TestCefCallbackState>>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
    }

    unsafe extern "C" fn test_add_ref(self_: *mut c_void) {
        test_counter(self_, |state| state.add_refs).fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn test_release(self_: *mut c_void) -> c_int {
        test_counter(self_, |state| state.releases).fetch_add(1, Ordering::SeqCst);
        0
    }

    unsafe extern "C" fn test_cont(self_: *mut c_void) {
        test_counter(self_, |state| state.continued).fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn test_cancel(self_: *mut c_void) {
        test_counter(self_, |state| state.cancelled).fetch_add(1, Ordering::SeqCst);
    }

    fn test_counter(
        self_: *mut c_void,
        select: impl FnOnce(TestCefCallbackState) -> usize,
    ) -> &'static AtomicUsize {
        let state = test_callback_registry()
            .lock()
            .expect("test callback registry")
            .get(&(self_ as usize))
            .copied()
            .expect("registered test callback");
        let ptr = select(state);
        unsafe { &*(ptr as *const AtomicUsize) }
    }
}
