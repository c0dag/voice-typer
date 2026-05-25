//! Live transcript overlay: a wide, semi-transparent banner near the bottom of
//! the primary monitor that shows the streaming transcript as the user speaks.
//!
//! Only visible during a streaming session. Uses WS_EX_LAYERED with a constant
//! source-alpha (via SetLayeredWindowAttributes) so we can paint with normal
//! GDI calls (no premultiplied alpha bitmap juggling). Click-through enabled so
//! the overlay doesn't steal focus from whatever app is receiving the eventual
//! paste.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::OnceLock;
use std::thread;

use parking_lot::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, ANTIALIASED_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT, DT_CENTER, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, FW_NORMAL,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW,
    GetSystemMetrics, LoadCursorW, PostMessageW, RegisterClassW, SetLayeredWindowAttributes,
    SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, IDC_ARROW, LWA_ALPHA, MSG,
    SHOW_WINDOW_CMD, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNA, WM_APP, WM_DESTROY, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const WIDTH: i32 = 860;
const HEIGHT: i32 = 96;
const MARGIN_BOTTOM: i32 = 96;

const WM_APP_SET_TEXT: u32 = WM_APP + 10;
const WM_APP_SHOW: u32 = WM_APP + 11;
const WM_APP_HIDE: u32 = WM_APP + 12;

static OVERLAY_HWND: AtomicI32 = AtomicI32::new(0);
static ENABLED: AtomicBool = AtomicBool::new(true);
static CURRENT_TEXT: OnceLock<Mutex<Vec<u16>>> = OnceLock::new();

fn text_slot() -> &'static Mutex<Vec<u16>> {
    CURRENT_TEXT.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn set_enabled(b: bool) {
    ENABLED.store(b, Ordering::Relaxed);
    if !b {
        hide();
    }
}

pub fn show() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let raw = OVERLAY_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as isize as *mut _);
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_SHOW, WPARAM(0), LPARAM(0));
    }
}

pub fn hide() {
    let raw = OVERLAY_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as isize as *mut _);
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_HIDE, WPARAM(0), LPARAM(0));
    }
}

/// Replace the displayed text. We keep up to ~400 trailing chars and let the
/// painter (with DT_WORDBREAK) handle wrapping into two visible lines. When
/// the wrapped layout overflows the available height, the painter shifts the
/// rect up so the BOTTOM (most recent words) is always visible — feels like
/// the text rolls upward as the user keeps speaking.
pub fn set_text(s: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let s = s.trim();
    let max_chars: usize = 400;
    let display = if s.chars().count() > max_chars {
        let suffix: String = s
            .chars()
            .rev()
            .take(max_chars)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // Start at a word boundary so we don't display a half-word at the start.
        match suffix.find(' ') {
            Some(idx) if idx < 30 => suffix[idx + 1..].to_string(),
            _ => suffix,
        }
    } else {
        s.to_string()
    };
    let wide: Vec<u16> = display.encode_utf16().chain(std::iter::once(0)).collect();
    *text_slot().lock() = wide;
    let raw = OVERLAY_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as isize as *mut _);
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_SET_TEXT, WPARAM(0), LPARAM(0));
    }
}

/// Spawn the overlay window on its own thread (separate Win32 message pump).
pub fn spawn() {
    thread::Builder::new()
        .name("overlay".into())
        .spawn(|| unsafe { run() })
        .ok();
}

unsafe fn run() {
    let hinstance = GetModuleHandleW(None).expect("module handle");
    let class_name = wide("VoiceTyperOverlay");
    let cursor = LoadCursorW(None, IDC_ARROW).expect("cursor");

    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        hCursor: cursor,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    let _atom = RegisterClassW(&wc);

    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let x = (sw - WIDTH) / 2;
    let y = sh - HEIGHT - MARGIN_BOTTOM;

    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        PCWSTR(class_name.as_ptr()),
        PCWSTR(wide("VoiceTyper Live").as_ptr()),
        WS_POPUP,
        x,
        y,
        WIDTH,
        HEIGHT,
        None,
        None,
        hinstance,
        None,
    )
    .expect("overlay create");

    // Whole-window alpha (~85% opaque). Pixels with alpha are blended uniformly.
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 220, LWA_ALPHA);
    OVERLAY_HWND.store(hwnd.0 as i32, Ordering::Relaxed);

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);

                // Dark background fill
                let bg = CreateSolidBrush(COLORREF(0x00181818));
                FillRect(hdc, &rect, bg);
                let _ = DeleteObject(bg);

                // Font: 22px sans
                let font = CreateFontW(
                    -22,
                    0,
                    0,
                    0,
                    FW_NORMAL.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET.0 as u32,
                    OUT_DEFAULT_PRECIS.0 as u32,
                    CLIP_DEFAULT_PRECIS.0 as u32,
                    ANTIALIASED_QUALITY.0 as u32,
                    (FF_DONTCARE.0 as u32) | (DEFAULT_PITCH.0 as u32),
                    PCWSTR(wide("Segoe UI").as_ptr()),
                );
                let old = SelectObject(hdc, font);
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(0x00EFEFEF));

                // Inset for padding
                let mut visible_rect = rect;
                visible_rect.left += 20;
                visible_rect.right -= 20;
                visible_rect.top += 8;
                visible_rect.bottom -= 8;

                let mut text_buf = text_slot().lock().clone();
                let mut is_placeholder = false;
                if text_buf.is_empty() {
                    text_buf = wide("Listening…");
                    is_placeholder = true;
                }

                if is_placeholder {
                    // Center the placeholder vertically + horizontally
                    DrawTextW(
                        hdc,
                        &mut text_buf,
                        &mut visible_rect,
                        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
                    );
                } else {
                    // Measure wrapped height (tall virtual rect, narrowed to actual width)
                    let mut measure_rect = visible_rect;
                    measure_rect.bottom = measure_rect.top + 9999;
                    let mut measure_buf = text_buf.clone();
                    DrawTextW(
                        hdc,
                        &mut measure_buf,
                        &mut measure_rect,
                        DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX | DT_LEFT,
                    );
                    let text_h = measure_rect.bottom - measure_rect.top;
                    let visible_h = visible_rect.bottom - visible_rect.top;

                    // If text overflows, shift the draw rect UP so the bottom
                    // (most recent words) sits inside the visible window.
                    let mut draw_rect = visible_rect;
                    if text_h > visible_h {
                        draw_rect.top -= text_h - visible_h;
                    }
                    DrawTextW(
                        hdc,
                        &mut text_buf,
                        &mut draw_rect,
                        DT_WORDBREAK | DT_NOPREFIX | DT_LEFT,
                    );
                }

                SelectObject(hdc, old);
                let _ = DeleteObject(font);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            x if x == WM_APP_SET_TEXT => {
                let _ = InvalidateRect(hwnd, None, true);
                LRESULT(0)
            }
            x if x == WM_APP_SHOW => {
                let _ = ShowWindow(hwnd, SHOW_WINDOW_CMD(SW_SHOWNA.0));
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                let _ = InvalidateRect(hwnd, None, true);
                LRESULT(0)
            }
            x if x == WM_APP_HIDE => {
                let _ = ShowWindow(hwnd, SHOW_WINDOW_CMD(SW_HIDE.0));
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}
