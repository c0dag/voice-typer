//! Always-on-top floating popup with PER-PIXEL ALPHA via UpdateLayeredWindow.
//!
//! The window has WS_EX_LAYERED + WS_EX_TOOLWINDOW + WS_EX_TOPMOST. Its content
//! is set entirely via UpdateLayeredWindow with a premultiplied-BGRA bitmap, so
//! antialiased edges blend smoothly with whatever is behind on the desktop.
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP,
    HDC, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadCursorW, MSG, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetWindowPos, ShowWindow, TrackPopupMenu, TranslateMessage,
    UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, HMENU, IDC_ARROW, MF_SEPARATOR, MF_STRING,
    SHOW_WINDOW_CMD, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNA, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, ULW_ALPHA, WM_APP, WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_RBUTTONUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::render::{rendered_bgra, PopupState, POPUP_SIZE};

const WM_APP_SET_STATE: u32 = WM_APP + 1;
const WM_APP_QUIT: u32 = WM_APP + 2;

const ID_SETTINGS: usize = 1001;
const ID_QUIT: usize = 1002;

/// Channel for menu actions back to the main thread.
pub enum PopupAction {
    OpenSettings,
    Quit,
}

static ACTION_TX: OnceLock<parking_lot::Mutex<Option<std::sync::mpsc::Sender<PopupAction>>>> =
    OnceLock::new();

/// Cached HWND so other threads can send commands.
static POPUP_HWND: AtomicI32 = AtomicI32::new(0);

/// Whether the floating indicator should be rendered. False = window stays
/// fully transparent regardless of state changes. Toggled at startup from
/// Config.show_popup and re-applied when Settings is saved.
static SHOW_POPUP: AtomicBool = AtomicBool::new(true);

pub fn set_show_popup(b: bool) {
    SHOW_POPUP.store(b, Ordering::Relaxed);
    // Re-render with the current state so the change is immediate.
    set_state_async(crate::render::PopupState::Idle);
}

#[derive(Default)]
struct DragState {
    dragging: bool,
    grab_offset: (i32, i32),
}

static DRAG: parking_lot::Mutex<DragState> = parking_lot::Mutex::new(DragState {
    dragging: false,
    grab_offset: (0, 0),
});

pub fn set_state_async(state: PopupState) {
    let raw = POPUP_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as isize as *mut _);
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            hwnd,
            WM_APP_SET_STATE,
            WPARAM(state as usize),
            LPARAM(0),
        );
    }
}

pub fn quit_async() {
    let raw = POPUP_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as isize as *mut _);
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            hwnd,
            WM_APP_QUIT,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// Run the popup on this thread. Blocks until WM_DESTROY.
pub fn run(action_tx: std::sync::mpsc::Sender<PopupAction>) -> anyhow::Result<()> {
    ACTION_TX.get_or_init(|| parking_lot::Mutex::new(None));
    *ACTION_TX.get().unwrap().lock() = Some(action_tx);

    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class_name = wide("VoiceTyperPopup");
        let cursor = LoadCursorW(None, IDC_ARROW)?;

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            hCursor: cursor,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let _atom = RegisterClassW(&wc);

        // Position bottom-right with margin
        let (sw, sh) = screen_size();
        let x = sw - POPUP_SIZE as i32 - 24;
        let y = sh - POPUP_SIZE as i32 - 80;

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("VoiceTyper").as_ptr()),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            POPUP_SIZE as i32,
            POPUP_SIZE as i32,
            None,
            None,
            hinstance,
            None,
        )?;
        POPUP_HWND.store(hwnd.0 as i32, Ordering::Relaxed);

        // Initial paint
        update_layered(hwnd, PopupState::Idle);
        let _ = ShowWindow(hwnd, SHOW_WINDOW_CMD(SW_SHOWNA.0));

        // Message pump
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn update_layered(hwnd: HWND, state: PopupState) {
    let w = POPUP_SIZE as i32;
    let h = POPUP_SIZE as i32;
    // If the user has disabled the popup, render an all-zero (fully transparent)
    // bitmap so the layered window is invisible. The window itself stays alive
    // so the message pump keeps draining and right-click on the (invisible)
    // surface still won't trigger anything — the user uses the tray icon.
    let pixels = if SHOW_POPUP.load(Ordering::Relaxed) {
        rendered_bgra(state)
    } else {
        vec![0u8; (w * h * 4) as usize]
    };

    let hdc_screen: HDC = GetDC(None);
    let hdc_mem: HDC = CreateCompatibleDC(hdc_screen);

    let mut bi = BITMAPINFO::default();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // negative = top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB.0;

    let mut bits_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let hbitmap: HBITMAP = match CreateDIBSection(
        hdc_screen,
        &bi,
        DIB_RGB_COLORS,
        &mut bits_ptr,
        None,
        0,
    ) {
        Ok(b) => b,
        Err(_) => {
            DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);
            return;
        }
    };
    if !bits_ptr.is_null() {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits_ptr as *mut u8, pixels.len());
    }

    let h_old = SelectObject(hdc_mem, hbitmap);

    // Get current window position so UpdateLayeredWindow doesn't re-position
    let mut rect = RECT::default();
    let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect);

    let pt_dst = POINT { x: rect.left, y: rect.top };
    let sz = windows::Win32::Foundation::SIZE { cx: w, cy: h };
    let pt_src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = UpdateLayeredWindow(
        hwnd,
        hdc_screen,
        Some(&pt_dst),
        Some(&sz),
        hdc_mem,
        Some(&pt_src),
        windows::Win32::Foundation::COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );

    SelectObject(hdc_mem, h_old);
    let _ = DeleteObject(hbitmap);
    DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_LBUTTONDOWN => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut rect = RECT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect);
                let mut d = DRAG.lock();
                d.dragging = true;
                d.grab_offset = (pt.x - rect.left, pt.y - rect.top);
                drop(d);
                let _ = SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let d = DRAG.lock();
                if d.dragging {
                    let off = d.grab_offset;
                    drop(d);
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        pt.x - off.0,
                        pt.y - off.1,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                DRAG.lock().dragging = false;
                let _ = ReleaseCapture();
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_context_menu(hwnd);
                LRESULT(0)
            }
            x if x == WM_APP_SET_STATE => {
                let state = PopupState::from_u8(w.0 as u8);
                update_layered(hwnd, state);
                LRESULT(0)
            }
            x if x == WM_APP_QUIT => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    let menu: HMENU = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };
    let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, PCWSTR(wide("Settings…").as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(wide("Quit").as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd); // required for TrackPopupMenu to dismiss properly

    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);

    let action = match cmd.0 as usize {
        ID_SETTINGS => Some(PopupAction::OpenSettings),
        ID_QUIT => Some(PopupAction::Quit),
        _ => None,
    };
    if let Some(a) = action {
        if let Some(slot) = ACTION_TX.get() {
            if let Some(tx) = slot.lock().as_ref() {
                let _ = tx.send(a);
            }
        }
    }
}
