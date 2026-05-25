//! Global hotkey on macOS via Carbon `RegisterEventHotKey`.
//!
//! Unlike a `CGEventTap`, `RegisterEventHotKey` registers a *single* key
//! combination with the system and is only notified for THAT combo — it never
//! sees any other keystroke. Because it's so narrowly scoped, macOS requires
//! **no** privacy permission at all (no Input Monitoring, no Accessibility), and
//! the registered key is consumed before it reaches the focused app, so a held
//! function key never beeps. (The only permission the app needs is Microphone,
//! plus Accessibility *only* for the optional auto-paste in `paste.rs`.)
//!
//! Hotkey events are delivered to the application event target on the **main
//! thread**, pumped by NSApplication's run loop. So registration must happen on
//! that thread: `install_main_thread` is called once from the popup module, and
//! the popup's main-thread timer calls `poll_reregister` to pick up hotkey
//! changes made in Settings. Worker threads only ever call `set_hotkey` (which
//! just stores the desired combo) and receive `HotkeyEvent`s over the channel.
//!
//! Token format mirrors the Windows side:
//!   bits  0..15  : trigger value (kVK_* for key, mouse button index 1..=5)
//!   bits 16..23  : trigger kind   (0 = disabled, 1 = key, 2 = mouse)
//!   bits 24..27  : required modifiers (CTRL=1, ALT=2, SHIFT=4, CMD=8)
//!
//! Mouse-button triggers are *not* supported on macOS (they'd require the very
//! Input-Monitoring tap we're avoiding); configuring one logs a warning.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub enum HotkeyEvent {
    Press,
    Release,
}

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

/// Desired hotkey (set from any thread).
static HOTKEY_TOKEN: AtomicU32 = AtomicU32::new(0);
/// Currently registered hotkey; `u32::MAX` means "nothing registered yet".
static REGISTERED_TOKEN: AtomicU32 = AtomicU32::new(u32::MAX);
/// Debounce so a held key (or any key-repeat) only fires one Press until release.
static IS_DOWN: AtomicU8 = AtomicU8::new(0);
/// The live `EventHotKeyRef`, so we can unregister before re-registering.
static HOTKEY_REF: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
/// Whether the application-level Carbon event handler has been installed.
static HANDLER_INSTALLED: AtomicU8 = AtomicU8::new(0);

static SENDER: OnceLock<parking_lot::Mutex<Option<Sender<HotkeyEvent>>>> = OnceLock::new();

// --- Carbon FFI (Carbon.framework) ---------------------------------------

type OSStatus = i32;
type OSType = u32;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerUPP =
    unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: OSType,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: OSType,
    id: u32,
}

const K_EVENT_CLASS_KEYBOARD: OSType = 0x6b65_7962; // 'keyb'
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_HOT_KEY_RELEASED: u32 = 6;

// Carbon modifier-key masks (Events.h).
const CONTROL_KEY: u32 = 0x1000;
const OPTION_KEY: u32 = 0x0800;
const SHIFT_KEY: u32 = 0x0200;
const CMD_KEY: u32 = 0x0100;

#[allow(non_snake_case)]
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerUPP,
        num_types: usize, // ItemCount (unsigned long)
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        hot_key_id: EventHotKeyID,
        target: EventTargetRef,
        options: u32, // OptionBits
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OSStatus;
    fn GetEventKind(event: EventRef) -> u32;
}

/// Carbon delivers pressed/released here on the main thread. We only register
/// one hotkey at a time, so any event we receive is ours.
unsafe extern "C" fn hotkey_handler(
    _call_ref: EventHandlerCallRef,
    event: EventRef,
    _user_data: *mut c_void,
) -> OSStatus {
    match GetEventKind(event) {
        K_EVENT_HOT_KEY_PRESSED => {
            if IS_DOWN.swap(1, Ordering::SeqCst) == 0 {
                send(HotkeyEvent::Press);
            }
        }
        K_EVENT_HOT_KEY_RELEASED => {
            if IS_DOWN.swap(0, Ordering::SeqCst) == 1 {
                send(HotkeyEvent::Release);
            }
        }
        _ => {}
    }
    0 // noErr
}

// --- Hotkey parsing (shared format with Windows) --------------------------

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
            "alt" | "option" | "opt" | "menu" => {
                mods |= MOD_ALT;
                continue;
            }
            "shift" => {
                mods |= MOD_SHIFT;
                continue;
            }
            "cmd" | "command" | "win" | "meta" | "super" => {
                mods |= MOD_WIN;
                continue;
            }
            _ => {}
        }
        if kind != KIND_NONE {
            return None;
        }
        if let Some(btn) = parse_mouse(&tok) {
            kind = KIND_MOUSE;
            val = btn as u32;
        } else if let Some(vk) = parse_vk(&tok) {
            kind = KIND_KEY;
            val = vk as u32;
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

fn parse_vk(s: &str) -> Option<u16> {
    if let Some(rest) = s.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u16>() {
            return f_key(n);
        }
    }
    Some(match s {
        "a" => 0x00, "s" => 0x01, "d" => 0x02, "f" => 0x03,
        "h" => 0x04, "g" => 0x05, "z" => 0x06, "x" => 0x07,
        "c" => 0x08, "v" => 0x09, "b" => 0x0B, "q" => 0x0C,
        "w" => 0x0D, "e" => 0x0E, "r" => 0x0F, "y" => 0x10,
        "t" => 0x11, "1" => 0x12, "2" => 0x13, "3" => 0x14,
        "4" => 0x15, "6" => 0x16, "5" => 0x17, "9" => 0x19,
        "7" => 0x1A, "8" => 0x1C, "0" => 0x1D, "o" => 0x1F,
        "u" => 0x20, "i" => 0x22, "p" => 0x23, "l" => 0x25,
        "j" => 0x26, "k" => 0x28, "n" => 0x2D, "m" => 0x2E,
        "space" | "spacebar" => 0x31,
        "tab" => 0x30,
        "return" | "enter" => 0x24,
        "escape" | "esc" => 0x35,
        "delete" | "backspace" => 0x33,
        "forward_delete" | "del" => 0x75,
        "left" | "leftarrow" => 0x7B,
        "right" | "rightarrow" => 0x7C,
        "down" | "downarrow" => 0x7D,
        "up" | "uparrow" => 0x7E,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        "pause" => 0x71,
        _ => return None,
    })
}

fn f_key(n: u16) -> Option<u16> {
    Some(match n {
        1 => 0x7A, 2 => 0x78, 3 => 0x63, 4 => 0x76,
        5 => 0x60, 6 => 0x61, 7 => 0x62, 8 => 0x64,
        9 => 0x65, 10 => 0x6D, 11 => 0x67, 12 => 0x6F,
        13 => 0x69, 14 => 0x6B, 15 => 0x71, 16 => 0x6A,
        17 => 0x40, 18 => 0x4F, 19 => 0x50, 20 => 0x5A,
        _ => return None,
    })
}

fn to_carbon_mods(mods: u32) -> u32 {
    let mut m = 0;
    if mods & MOD_CTRL != 0 {
        m |= CONTROL_KEY;
    }
    if mods & MOD_ALT != 0 {
        m |= OPTION_KEY;
    }
    if mods & MOD_SHIFT != 0 {
        m |= SHIFT_KEY;
    }
    if mods & MOD_WIN != 0 {
        m |= CMD_KEY;
    }
    m
}

// --- Public API -----------------------------------------------------------

pub fn set_hotkey(token: i32) {
    HOTKEY_TOKEN.store(token as u32, Ordering::SeqCst);
}

/// Store the channel the Carbon handler sends Press/Release on. Does NOT spawn a
/// thread (Carbon delivers on the main thread); call `install_main_thread` from
/// the popup's run loop to actually start receiving events.
pub fn spawn(tx: Sender<HotkeyEvent>) {
    SENDER.get_or_init(|| parking_lot::Mutex::new(None));
    *SENDER.get().unwrap().lock() = Some(tx);
}

/// Install the application Carbon event handler and register the current hotkey.
/// MUST be called on the main thread (the one running NSApplication).
pub fn install_main_thread() {
    install_handler_once();
    reregister();
}

/// Re-register if the desired hotkey changed (e.g. via Settings). MUST be called
/// on the main thread — the popup timer calls this every tick.
pub fn poll_reregister() {
    if HOTKEY_TOKEN.load(Ordering::SeqCst) != REGISTERED_TOKEN.load(Ordering::SeqCst) {
        reregister();
    }
}

fn install_handler_once() {
    if HANDLER_INSTALLED.swap(1, Ordering::SeqCst) == 1 {
        return;
    }
    let specs = [
        EventTypeSpec {
            event_class: K_EVENT_CLASS_KEYBOARD,
            event_kind: K_EVENT_HOT_KEY_PRESSED,
        },
        EventTypeSpec {
            event_class: K_EVENT_CLASS_KEYBOARD,
            event_kind: K_EVENT_HOT_KEY_RELEASED,
        },
    ];
    unsafe {
        let mut handler_ref: EventHandlerRef = null_mut();
        let st = InstallEventHandler(
            GetApplicationEventTarget(),
            hotkey_handler,
            specs.len(),
            specs.as_ptr(),
            null_mut(),
            &mut handler_ref,
        );
        if st != 0 {
            log::error!("hotkey: InstallEventHandler failed (OSStatus {st})");
        } else {
            log::info!("hotkey: Carbon event handler installed");
        }
    }
}

fn reregister() {
    // Drop the previous registration first.
    let prev = HOTKEY_REF.swap(null_mut(), Ordering::SeqCst);
    if !prev.is_null() {
        unsafe {
            UnregisterEventHotKey(prev);
        }
    }
    IS_DOWN.store(0, Ordering::SeqCst);

    let token = HOTKEY_TOKEN.load(Ordering::SeqCst);
    REGISTERED_TOKEN.store(token, Ordering::SeqCst);

    let kind = unpack_kind(token);
    if kind != KIND_KEY {
        if kind == KIND_MOUSE {
            log::warn!(
                "hotkey: mouse-button triggers aren't supported on macOS (that needs the \
                 Input-Monitoring tap we deliberately avoid). Pick a keyboard hotkey in Settings."
            );
        }
        return;
    }

    let key_code = unpack_val(token);
    let carbon_mods = to_carbon_mods(unpack_mods(token));
    let hk_id = EventHotKeyID {
        signature: u32::from_be_bytes(*b"vtyp"),
        id: 1,
    };
    unsafe {
        let mut hk_ref: EventHotKeyRef = null_mut();
        let st = RegisterEventHotKey(
            key_code,
            carbon_mods,
            hk_id,
            GetApplicationEventTarget(),
            0,
            &mut hk_ref,
        );
        if st != 0 || hk_ref.is_null() {
            log::error!(
                "hotkey: RegisterEventHotKey failed (OSStatus {st}) for keycode {key_code:#x}"
            );
        } else {
            HOTKEY_REF.store(hk_ref, Ordering::SeqCst);
            log::info!(
                "hotkey: registered keycode {key_code:#x} mods {carbon_mods:#x} \
                 (Carbon RegisterEventHotKey — no permission, no beep)"
            );
        }
    }
}

fn send(ev: HotkeyEvent) {
    if let Some(slot) = SENDER.get() {
        if let Some(tx) = slot.lock().as_ref() {
            let _ = tx.send(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_f9() {
        let t = parse_hotkey("f9").expect("f9");
        assert_eq!(unpack_kind(t as u32), KIND_KEY);
        assert_eq!(unpack_val(t as u32), 0x65);
    }

    #[test]
    fn parses_modifier_combo() {
        let t = parse_hotkey("ctrl+shift+v").expect("combo");
        assert_eq!(unpack_kind(t as u32), KIND_KEY);
        assert_eq!(unpack_val(t as u32), 0x09);
        assert_eq!(unpack_mods(t as u32), MOD_CTRL | MOD_SHIFT);
    }

    #[test]
    fn carbon_mods_map() {
        assert_eq!(to_carbon_mods(MOD_CTRL | MOD_SHIFT), CONTROL_KEY | SHIFT_KEY);
        assert_eq!(to_carbon_mods(MOD_WIN), CMD_KEY);
        assert_eq!(to_carbon_mods(MOD_ALT), OPTION_KEY);
    }

    #[test]
    fn parses_cmd_alias() {
        let a = parse_hotkey("cmd+space").expect("cmd+space");
        let b = parse_hotkey("command+space").expect("command+space");
        assert_eq!(a, b);
        assert_eq!(unpack_mods(a as u32), MOD_WIN);
    }
}
