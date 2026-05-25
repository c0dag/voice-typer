//! Global keyboard + mouse hooks on a dedicated thread with a message pump.
//!
//! Sends `HotkeyEvent::Press` and `Release` to the main thread via an mpsc channel.
//! Supports any single key (parsed by name), the L/R/M/X1/X2 mouse buttons,
//! and modifier combos like `ctrl+x`, `alt+space`, `ctrl+shift+mouse4`.
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::thread;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_F1, VK_PAUSE};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    XBUTTON1, XBUTTON2,
};

#[derive(Debug, Clone, Copy)]
pub enum HotkeyEvent {
    Press,
    Release,
}

// =====================================================================
// Encoding of the configured hotkey into a single u32 atomic, so the
// hook callbacks (C-style functions) can read it lock-free.
//
//   bits  0..15  : trigger value (VK code, or mouse button index 1..=5)
//   bits 16..23  : trigger kind   (0 = disabled, 1 = key, 2 = mouse)
//   bits 24..27  : required modifiers (CTRL=1, ALT=2, SHIFT=4, WIN=8)
//   bits 28..31  : reserved
// =====================================================================
const KIND_NONE: u32 = 0;
const KIND_KEY: u32 = 1;
const KIND_MOUSE: u32 = 2;

pub const MOD_CTRL: u32 = 1 << 0;
pub const MOD_ALT: u32 = 1 << 1;
pub const MOD_SHIFT: u32 = 1 << 2;
pub const MOD_WIN: u32 = 1 << 3;

const fn pack(mods: u32, kind: u32, val: u32) -> u32 {
    ((mods & 0xF) << 24) | ((kind & 0xFF) << 16) | (val & 0xFFFF)
}
const fn unpack_kind(t: u32) -> u32 {
    (t >> 16) & 0xFF
}
const fn unpack_val(t: u32) -> u32 {
    t & 0xFFFF
}
const fn unpack_mods(t: u32) -> u32 {
    (t >> 24) & 0xF
}

static HOTKEY_TOKEN: AtomicU32 = AtomicU32::new(0);
/// Track currently-pressed state (debounces autorepeat KEYDOWN).
static IS_DOWN: AtomicU8 = AtomicU8::new(0);
/// Live modifier state (independent of the configured trigger).
static HELD_MODS: AtomicU8 = AtomicU8::new(0);

/// Channel sender, set once at startup.
static SENDER: OnceLock<parking_lot::Mutex<Option<Sender<HotkeyEvent>>>> = OnceLock::new();

// =====================================================================
// Public parser (returns the i32 token for set_hotkey).
// Public API stays i32 to keep callers stable; we just transmute u32->i32.
// =====================================================================
pub fn parse_hotkey(s: &str) -> Option<i32> {
    let mut mods: u32 = 0;
    let mut kind: u32 = KIND_NONE;
    let mut val: u32 = 0;

    for raw in s.split('+') {
        let tok = raw.trim().to_lowercase();
        if tok.is_empty() {
            continue;
        }
        match tok.as_str() {
            "ctrl" | "control" => {
                mods |= MOD_CTRL;
                continue;
            }
            "alt" | "menu" => {
                mods |= MOD_ALT;
                continue;
            }
            "shift" => {
                mods |= MOD_SHIFT;
                continue;
            }
            "win" | "meta" | "cmd" | "super" => {
                mods |= MOD_WIN;
                continue;
            }
            _ => {}
        }
        // Trigger token
        if kind != KIND_NONE {
            // already have a trigger, second non-modifier is invalid
            return None;
        }
        if let Some(btn) = parse_mouse(&tok) {
            kind = KIND_MOUSE;
            val = btn as u32;
        } else if let Some(vk) = parse_vk(&tok) {
            kind = KIND_KEY;
            val = vk.0 as u32;
        } else {
            return None;
        }
    }
    if kind == KIND_NONE {
        return None;
    }
    Some(pack(mods, kind, val) as i32)
}

fn parse_mouse(s: &str) -> Option<u8> {
    Some(match s {
        "mouse1" | "lmb" | "leftclick" | "left" => 1,
        "mouse2" | "rmb" | "rightclick" | "right" => 2,
        "mouse3" | "mmb" | "middle" => 3,
        "mouse4" | "x1" | "back" => 4,
        "mouse5" | "x2" | "forward" => 5,
        _ => return None,
    })
}

fn parse_vk(s: &str) -> Option<VIRTUAL_KEY> {
    // Function keys F1..F24
    if let Some(rest) = s.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u16>() {
            if (1..=24).contains(&n) {
                return Some(VIRTUAL_KEY(VK_F1.0 + (n - 1)));
            }
        }
    }
    // Numpad: numpad0..numpad9
    if let Some(rest) = s.strip_prefix("numpad") {
        if let Ok(n) = rest.parse::<u16>() {
            if n <= 9 {
                return Some(VIRTUAL_KEY(0x60 + n));
            }
        }
    }
    Some(match s {
        "pause" => VK_PAUSE,
        "space" | "spacebar" => VIRTUAL_KEY(0x20),
        "tab" => VIRTUAL_KEY(0x09),
        "esc" | "escape" => VIRTUAL_KEY(0x1B),
        "enter" | "return" => VIRTUAL_KEY(0x0D),
        "ins" | "insert" => VIRTUAL_KEY(0x2D),
        "del" | "delete" => VIRTUAL_KEY(0x2E),
        "home" => VIRTUAL_KEY(0x24),
        "end" => VIRTUAL_KEY(0x23),
        "pgup" | "pageup" => VIRTUAL_KEY(0x21),
        "pgdn" | "pagedown" => VIRTUAL_KEY(0x22),
        "left" | "arrowleft" => VIRTUAL_KEY(0x25),
        "up" | "arrowup" => VIRTUAL_KEY(0x26),
        "right" | "arrowright" => VIRTUAL_KEY(0x27),
        "down" | "arrowdown" => VIRTUAL_KEY(0x28),
        "backspace" | "back" => VIRTUAL_KEY(0x08),
        "capslock" | "caps" => VIRTUAL_KEY(0x14),
        "numlock" => VIRTUAL_KEY(0x90),
        "scrolllock" | "scroll" => VIRTUAL_KEY(0x91),
        "printscreen" | "prtsc" | "print" => VIRTUAL_KEY(0x2C),
        "minus" | "-" => VIRTUAL_KEY(0xBD),
        "equals" | "=" | "plus" => VIRTUAL_KEY(0xBB),
        "comma" | "," => VIRTUAL_KEY(0xBC),
        "period" | "." => VIRTUAL_KEY(0xBE),
        "slash" | "/" => VIRTUAL_KEY(0xBF),
        "backtick" | "tilde" | "`" => VIRTUAL_KEY(0xC0),
        "lbracket" | "[" => VIRTUAL_KEY(0xDB),
        "rbracket" | "]" => VIRTUAL_KEY(0xDD),
        "backslash" | "\\" => VIRTUAL_KEY(0xDC),
        "semicolon" | ";" => VIRTUAL_KEY(0xBA),
        "quote" | "apostrophe" | "'" => VIRTUAL_KEY(0xDE),
        // Single A-Z or 0-9
        c if c.len() == 1 => {
            let ch = c.chars().next().unwrap();
            if ch.is_ascii_alphabetic() {
                return Some(VIRTUAL_KEY(ch.to_ascii_uppercase() as u16));
            }
            if ch.is_ascii_digit() {
                return Some(VIRTUAL_KEY(ch as u16));
            }
            return None;
        }
        _ => return None,
    })
}

pub fn set_hotkey(token: i32) {
    HOTKEY_TOKEN.store(token as u32, Ordering::SeqCst);
    IS_DOWN.store(0, Ordering::SeqCst);
}

pub fn spawn(tx: Sender<HotkeyEvent>) {
    SENDER.get_or_init(|| parking_lot::Mutex::new(None));
    *SENDER.get().unwrap().lock() = Some(tx);

    thread::Builder::new()
        .name("hotkey-hooks".into())
        .spawn(|| unsafe { hook_thread() })
        .expect("spawn hook thread");
}

unsafe fn hook_thread() {
    let hmod: windows::Win32::Foundation::HMODULE =
        GetModuleHandleW(None).unwrap_or_default();
    let hinst: windows::Win32::Foundation::HINSTANCE = hmod.into();
    let h_kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), hinst, 0);
    let h_mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinst, 0);

    if h_kb.is_ok() {
        log::info!("WH_KEYBOARD_LL installed");
    }
    if h_mouse.is_ok() {
        log::info!("WH_MOUSE_LL installed");
    }

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    if let Ok(h) = h_kb {
        let _ = UnhookWindowsHookEx(h);
    }
    if let Ok(h) = h_mouse {
        let _ = UnhookWindowsHookEx(h);
    }
}

fn modifier_bit(vk: u16) -> u8 {
    match vk {
        // Control: VK_CONTROL=0x11, VK_LCONTROL=0xA2, VK_RCONTROL=0xA3
        0x11 | 0xA2 | 0xA3 => MOD_CTRL as u8,
        // Alt / Menu: VK_MENU=0x12, VK_LMENU=0xA4, VK_RMENU=0xA5
        0x12 | 0xA4 | 0xA5 => MOD_ALT as u8,
        // Shift: VK_SHIFT=0x10, VK_LSHIFT=0xA0, VK_RSHIFT=0xA1
        0x10 | 0xA0 | 0xA1 => MOD_SHIFT as u8,
        // Win: VK_LWIN=0x5B, VK_RWIN=0x5C
        0x5B | 0x5C => MOD_WIN as u8,
        _ => 0,
    }
}

extern "system" fn kb_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { *(l.0 as *const KBDLLHOOKSTRUCT) };
        let m = w.0 as u32;
        let vk = info.vkCode as u16;
        let is_down = m == WM_KEYDOWN || m == WM_SYSKEYDOWN;
        let is_up = m == WM_KEYUP || m == WM_SYSKEYUP;

        // 1. Update live modifier state
        let mb = modifier_bit(vk);
        if mb != 0 {
            if is_down {
                HELD_MODS.fetch_or(mb, Ordering::SeqCst);
            } else if is_up {
                HELD_MODS.fetch_and(!mb, Ordering::SeqCst);
            }
        }

        // 2. Trigger detection
        let token = HOTKEY_TOKEN.load(Ordering::Relaxed);
        if unpack_kind(token) == KIND_KEY {
            let target_vk = unpack_val(token) as u16;
            if vk == target_vk {
                let req_mods = unpack_mods(token) as u8;
                let held = HELD_MODS.load(Ordering::Relaxed);
                let mods_match = (held & req_mods) == req_mods;
                if is_down && mods_match {
                    if IS_DOWN.swap(1, Ordering::SeqCst) == 0 {
                        send(HotkeyEvent::Press);
                    }
                } else if is_up {
                    if IS_DOWN.swap(0, Ordering::SeqCst) == 1 {
                        send(HotkeyEvent::Release);
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}

extern "system" fn mouse_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let token = HOTKEY_TOKEN.load(Ordering::Relaxed);
        if unpack_kind(token) == KIND_MOUSE {
            let want_btn = unpack_val(token) as u8;
            let info = unsafe { *(l.0 as *const MSLLHOOKSTRUCT) };
            let m = w.0 as u32;
            let (btn, is_down) = match m {
                WM_LBUTTONDOWN => (1, true),
                WM_LBUTTONUP => (1, false),
                WM_RBUTTONDOWN => (2, true),
                WM_RBUTTONUP => (2, false),
                WM_MBUTTONDOWN => (3, true),
                WM_MBUTTONUP => (3, false),
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let x: u32 = ((info.mouseData >> 16) & 0xFFFF) as u32;
                    let b = if x == XBUTTON1 as u32 {
                        4
                    } else if x == XBUTTON2 as u32 {
                        5
                    } else {
                        0
                    };
                    (b, m == WM_XBUTTONDOWN)
                }
                _ => (0, false),
            };
            if btn != 0 && btn == want_btn {
                let req_mods = unpack_mods(token) as u8;
                let held = HELD_MODS.load(Ordering::Relaxed);
                let mods_match = (held & req_mods) == req_mods;
                if is_down && mods_match {
                    if IS_DOWN.swap(1, Ordering::SeqCst) == 0 {
                        send(HotkeyEvent::Press);
                    }
                } else if !is_down {
                    if IS_DOWN.swap(0, Ordering::SeqCst) == 1 {
                        send(HotkeyEvent::Release);
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}

fn send(ev: HotkeyEvent) {
    if let Some(slot) = SENDER.get() {
        if let Some(tx) = slot.lock().as_ref() {
            let _ = tx.send(ev);
        }
    }
}
