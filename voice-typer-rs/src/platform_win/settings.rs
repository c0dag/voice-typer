//! Native Win32 settings dialog. No external GUI deps.
//!
//! Layout (≈460×400):
//!   Hotkey         [_______________________] (ex: f9, mouse4, mouse5)
//!   Deepgram Key   [••••••••••••••••••••••••]
//!   Modelo         [▼ nova-3                ]
//!   Idioma         [pt-BR  ]  (BCP-47)
//!   Microfone      [▼ (padrão do sistema)   ]
//!   ☑ Colar texto automaticamente após transcrever
//!                                       [Cancelar] [Salvar]
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use anyhow::Result;
use parking_lot::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT, HFONT, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, BST_CHECKED, BST_UNCHECKED, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW, IsDialogMessageW, LoadCursorW,
    PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow,
    TranslateMessage, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CBS_HASSTRINGS, CS_HREDRAW,
    CS_VREDRAW, ES_AUTOHSCROLL, ES_PASSWORD, HMENU, IDC_ARROW, MSG, SHOW_WINDOW_CMD, SM_CXSCREEN,
    SM_CYSCREEN, SWP_NOSIZE, SW_SHOWNORMAL, WINDOW_STYLE, WM_COMMAND, WM_DESTROY, WM_SETFONT,
    WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};

use crate::audio::Recorder;
use crate::config::Config;

const W: i32 = 620;
// Height is sized to sit the bottom buttons (at H-80) just under the last
// control, with a small margin. The content ends near y=280.
const H: i32 = 380;
const PAD: i32 = 18;
const ROW_H: i32 = 36;
const LABEL_W: i32 = 140;
const FIELD_X: i32 = PAD + LABEL_W + 10;
const FIELD_W: i32 = W - FIELD_X - PAD - 18;

const ID_HOTKEY: i32 = 1001;
const ID_PROXY_TOKEN: i32 = 1011;
const ID_DEVICE: i32 = 1005;
const ID_PASTE: i32 = 1006;
const ID_SHOW_POPUP: i32 = 1007;
const ID_STREAMING: i32 = 1009;
const ID_OVERLAY: i32 = 1010;
const ID_SAVE: i32 = 1100;
const ID_CANCEL: i32 = 1101;

struct Ctx {
    window: HWND,
    hotkey: HWND,
    proxy_token: HWND,
    device: HWND,
    paste: HWND,
    show_popup: HWND,
    streaming: HWND,
    overlay: HWND,
    devices: Vec<String>,
    notify_tx: Sender<()>,
}

unsafe impl Send for Ctx {}
unsafe impl Sync for Ctx {}

static CTX: OnceLock<Mutex<Option<Ctx>>> = OnceLock::new();

fn ctx_slot() -> &'static Mutex<Option<Ctx>> {
    CTX.get_or_init(|| Mutex::new(None))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn ui_font() -> HFONT {
    HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
}

pub fn spawn(notify_tx: Sender<()>) {
    {
        let g = ctx_slot().lock();
        if let Some(c) = g.as_ref() {
            unsafe {
                let _ = ShowWindow(c.window, SHOW_WINDOW_CMD(SW_SHOWNORMAL.0));
                let _ = SetForegroundWindow(c.window);
            }
            return;
        }
    }
    std::thread::Builder::new()
        .name("settings".into())
        .spawn(move || {
            if let Err(e) = unsafe { run(notify_tx) } {
                log::error!("settings: {e:?}");
            }
        })
        .ok();
}

unsafe fn run(notify_tx: Sender<()>) -> Result<()> {
    let cc = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_STANDARD_CLASSES,
    };
    let _ = InitCommonControlsEx(&cc);

    let hinstance = GetModuleHandleW(None)?;
    let class_name = wide("VoiceTyperSettings");
    let cursor = LoadCursorW(None, IDC_ARROW)?;

    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        hCursor: cursor,
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(GetStockObject(WHITE_BRUSH).0),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);

    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let x = (sw - W) / 2;
    let y = (sh - H) / 2;

    let title = wide("Voice Typer · Configurações");
    let window = CreateWindowExW(
        Default::default(),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        x,
        y,
        W,
        H,
        None,
        None,
        hinstance,
        None,
    )?;

    let cfg = Config::load();

    let label = |y: i32, text: &str| -> HWND {
        let t = wide(text);
        let h = CreateWindowExW(
            Default::default(),
            PCWSTR(wide("STATIC").as_ptr()),
            PCWSTR(t.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            PAD,
            y + 6,
            LABEL_W,
            22,
            window,
            None,
            hinstance,
            None,
        )
        .unwrap();
        SendMessageW(h, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
        h
    };

    let hint = |x: i32, y: i32, w: i32, text: &str| -> HWND {
        let t = wide(text);
        let h = CreateWindowExW(
            Default::default(),
            PCWSTR(wide("STATIC").as_ptr()),
            PCWSTR(t.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            x,
            y,
            w,
            16,
            window,
            None,
            hinstance,
            None,
        )
        .unwrap();
        SendMessageW(h, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
        h
    };

    let mut row_y = PAD;

    // Hotkey
    label(row_y, "Hotkey");
    let hotkey_text = wide(&cfg.hotkey);
    let hotkey = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("EDIT").as_ptr()),
        PCWSTR(hotkey_text.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        FIELD_X,
        row_y,
        FIELD_W,
        24,
        window,
        HMENU(ID_HOTKEY as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(hotkey, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
    row_y += 26;
    hint(
        FIELD_X,
        row_y,
        FIELD_W,
        "Qualquer tecla do teclado · mouse1 a mouse5 · ou combo com Ctrl/Alt/Shift/Win",
    );
    row_y += 16;
    hint(
        FIELD_X,
        row_y,
        FIELD_W,
        "Ex: f9 · pause · ctrl+x · alt+space · ctrl+shift+f9 · ctrl+mouse4",
    );
    row_y += 18;

    // Proxy token (password style)
    label(row_y, "Token");
    let proxy_token_text = wide(&cfg.proxy_token);
    let proxy_token = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("EDIT").as_ptr()),
        PCWSTR(proxy_token_text.as_ptr()),
        WS_CHILD
            | WS_VISIBLE
            | WS_BORDER
            | WS_TABSTOP
            | WINDOW_STYLE(ES_AUTOHSCROLL as u32)
            | WINDOW_STYLE(ES_PASSWORD as u32),
        FIELD_X,
        row_y,
        FIELD_W,
        24,
        window,
        HMENU(ID_PROXY_TOKEN as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(proxy_token, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
    row_y += 26;
    hint(FIELD_X, row_y, FIELD_W, "Paste from the proxy's dashboard. One device per token.");
    row_y += 18;

    // Microphone
    label(row_y, "Microfone");
    let device = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("COMBOBOX").as_ptr()),
        PCWSTR::null(),
        WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WS_TABSTOP
            | WINDOW_STYLE(CBS_DROPDOWNLIST as u32 | CBS_HASSTRINGS as u32),
        FIELD_X,
        row_y,
        FIELD_W,
        220,
        window,
        HMENU(ID_DEVICE as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(device, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));

    let mut devices: Vec<String> = Vec::new();
    devices.push(String::new());
    let default_w = wide("(padrão do sistema)");
    SendMessageW(
        device,
        CB_ADDSTRING,
        WPARAM(0),
        LPARAM(default_w.as_ptr() as isize),
    );
    let mut selected_idx = 0i32;
    for (i, name) in Recorder::list_input_devices().into_iter().enumerate() {
        let label = wide(&name);
        SendMessageW(
            device,
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(label.as_ptr() as isize),
        );
        devices.push(name.clone());
        if !cfg.device_name.is_empty() && name.contains(&cfg.device_name) {
            selected_idx = (i + 1) as i32;
        }
    }
    SendMessageW(
        device,
        CB_SETCURSEL,
        WPARAM(selected_idx as usize),
        LPARAM(0),
    );
    row_y += ROW_H + 4;

    // Paste checkbox
    let paste_text = wide("Colar texto automaticamente após transcrever (Ctrl+V)");
    let paste = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("BUTTON").as_ptr()),
        PCWSTR(paste_text.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        PAD,
        row_y,
        W - 2 * PAD - 18,
        24,
        window,
        HMENU(ID_PASTE as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(paste, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
    SendMessageW(
        paste,
        BM_SETCHECK,
        WPARAM(if cfg.paste_after_transcribe {
            BST_CHECKED.0 as usize
        } else {
            BST_UNCHECKED.0 as usize
        }),
        LPARAM(0),
    );
    row_y += 30;

    // Show-popup checkbox
    let sp_text = wide("Mostrar popup flutuante (indicador no canto da tela)");
    let show_popup = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("BUTTON").as_ptr()),
        PCWSTR(sp_text.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        PAD,
        row_y,
        W - 2 * PAD - 18,
        24,
        window,
        HMENU(ID_SHOW_POPUP as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(show_popup, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
    SendMessageW(
        show_popup,
        BM_SETCHECK,
        WPARAM(if cfg.show_popup {
            BST_CHECKED.0 as usize
        } else {
            BST_UNCHECKED.0 as usize
        }),
        LPARAM(0),
    );
    row_y += 30;

    // Streaming-mode checkbox
    let st_text = wide("Modo streaming (transcrição em tempo real — texto aparece enquanto fala)");
    let streaming = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("BUTTON").as_ptr()),
        PCWSTR(st_text.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        PAD,
        row_y,
        W - 2 * PAD - 18,
        24,
        window,
        HMENU(ID_STREAMING as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(streaming, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
    SendMessageW(
        streaming,
        BM_SETCHECK,
        WPARAM(if cfg.streaming_mode { BST_CHECKED.0 as usize } else { BST_UNCHECKED.0 as usize }),
        LPARAM(0),
    );
    row_y += 28;

    // Live overlay checkbox
    let ov_text = wide("Mostrar overlay de preview ao vivo (durante streaming)");
    let overlay_cb = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("BUTTON").as_ptr()),
        PCWSTR(ov_text.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        PAD,
        row_y,
        W - 2 * PAD - 18,
        24,
        window,
        HMENU(ID_OVERLAY as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(overlay_cb, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));
    SendMessageW(
        overlay_cb,
        BM_SETCHECK,
        WPARAM(if cfg.show_live_overlay { BST_CHECKED.0 as usize } else { BST_UNCHECKED.0 as usize }),
        LPARAM(0),
    );
    row_y += 30;

    // Bottom buttons
    let cancel_text = wide("Cancelar");
    let _cancel = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("BUTTON").as_ptr()),
        PCWSTR(cancel_text.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        W - 2 * PAD - 18 - 200,
        H - 80,
        90,
        28,
        window,
        HMENU(ID_CANCEL as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(_cancel, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));

    let save_text = wide("Salvar");
    let _save = CreateWindowExW(
        Default::default(),
        PCWSTR(wide("BUTTON").as_ptr()),
        PCWSTR(save_text.as_ptr()),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
        W - 2 * PAD - 18 - 100,
        H - 80,
        90,
        28,
        window,
        HMENU(ID_SAVE as *mut _),
        hinstance,
        None,
    )?;
    SendMessageW(_save, WM_SETFONT, WPARAM(ui_font().0 as usize), LPARAM(1));

    *ctx_slot().lock() = Some(Ctx {
        window,
        hotkey,
        proxy_token,
        device,
        paste,
        show_popup,
        streaming,
        overlay: overlay_cb,
        devices,
        notify_tx,
    });

    let _ = ShowWindow(window, SHOW_WINDOW_CMD(SW_SHOWNORMAL.0));
    let _ = SetWindowPos(window, None, 0, 0, 0, 0, SWP_NOSIZE);
    let _ = SetForegroundWindow(window);

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        if !IsDialogMessageW(window, &msg).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe fn read_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n as usize])
}

unsafe fn save_and_close() {
    let mut g = ctx_slot().lock();
    let Some(c) = g.as_ref() else { return };

    let mut cfg = Config::load();
    cfg.hotkey = read_text(c.hotkey).trim().to_lowercase();
    if cfg.hotkey.is_empty() {
        cfg.hotkey = "f9".into();
    }
    cfg.proxy_token = read_text(c.proxy_token).trim().to_string();

    let dev_sel = SendMessageW(c.device, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
    cfg.device_name = if dev_sel <= 0 {
        String::new()
    } else {
        c.devices
            .get(dev_sel as usize)
            .cloned()
            .unwrap_or_default()
    };

    cfg.paste_after_transcribe =
        SendMessageW(c.paste, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED.0 as isize;
    cfg.show_popup =
        SendMessageW(c.show_popup, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED.0 as isize;
    cfg.streaming_mode =
        SendMessageW(c.streaming, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED.0 as isize;
    cfg.show_live_overlay =
        SendMessageW(c.overlay, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED.0 as isize;

    if let Err(e) = cfg.save() {
        log::error!("settings save: {e}");
    } else {
        log::info!("settings saved");
        let _ = c.notify_tx.send(());
    }

    let window = c.window;
    *g = None;
    drop(g);
    let _ = DestroyWindow(window);
}

unsafe fn close() {
    let mut g = ctx_slot().lock();
    if let Some(c) = g.as_ref() {
        let h = c.window;
        *g = None;
        drop(g);
        let _ = DestroyWindow(h);
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (w.0 & 0xFFFF) as i32;
                match id {
                    ID_SAVE => {
                        save_and_close();
                        return LRESULT(0);
                    }
                    ID_CANCEL => {
                        close();
                        return LRESULT(0);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                {
                    let mut g = ctx_slot().lock();
                    *g = None;
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}
