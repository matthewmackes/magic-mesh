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

    let mut first_paint = None;
    let started = Instant::now();
    let mut rbuf = Vec::new();
    loop {
        abi.do_message_loop_work();
        match sock::recv(stream) {
            Ok(RecvOutcome::Data { bytes, .. }) => {
                rbuf.extend_from_slice(&bytes);
                drain_control_frames(&mut rbuf, browser, &callbacks);
            }
            Ok(RecvOutcome::WouldBlock) => {}
            Ok(RecvOutcome::Eof) => break,
            Err(_) => break,
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
        if first_paint.is_none() && started.elapsed() > Duration::from_secs(15) {
            abi.shutdown();
            return Err(CefBrowserError::TimedOut {
                created: callbacks.created(),
                paints: callbacks.paints(),
            });
        }
        thread::sleep(Duration::from_millis(8));
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
        ControlMsg::PrintPage => print_page(browser),
        ControlMsg::SavePdf { path } => save_pdf(browser, callbacks, path),
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
    _is_navigation: c_int,
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
    with_state(self_, |state| state.resource_request_ptr()).unwrap_or(ptr::null_mut())
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
