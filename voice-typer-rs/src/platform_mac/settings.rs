//! macOS native Settings window (parity with the Windows dialog).
//!
//! `spawn()` is called from the worker thread (tray/popup "Settings" action). It
//! can't touch AppKit off the main thread, so it just sets a request flag. The
//! popup module's main-thread timer calls `poll(mtm)` every tick, which opens
//! the window on the main thread and — when the user closes it — reads the
//! controls, writes config.json, and signals a reload via the stored Sender.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn, NSPopUpButton,
    NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::audio::Recorder;
use crate::config::Config;

static OPEN_REQUESTED: AtomicBool = AtomicBool::new(false);

fn notify_slot() -> &'static parking_lot::Mutex<Option<Sender<()>>> {
    static N: OnceLock<parking_lot::Mutex<Option<Sender<()>>>> = OnceLock::new();
    N.get_or_init(|| parking_lot::Mutex::new(None))
}

/// Called from the worker thread. Stores the reload-notify sender and requests
/// the main thread to open the window.
pub fn spawn(notify_tx: Sender<()>) {
    *notify_slot().lock() = Some(notify_tx);
    OPEN_REQUESTED.store(true, Ordering::SeqCst);
}

struct SettingsUi {
    window: Retained<NSWindow>,
    hotkey: Retained<NSTextField>,
    token: Retained<NSTextField>,
    device: Retained<NSPopUpButton>,
    devices: Vec<String>, // index 0 = "(system default)"
    paste: Retained<NSButton>,
    show_popup: Retained<NSButton>,
    streaming: Retained<NSButton>,
    overlay: Retained<NSButton>,
}

thread_local! {
    static UI: RefCell<Option<SettingsUi>> = const { RefCell::new(None) };
    static WAS_VISIBLE: RefCell<bool> = const { RefCell::new(false) };
}

/// Called by the popup main-thread timer every tick.
pub(crate) fn poll(mtm: MainThreadMarker) {
    // Open on request.
    if OPEN_REQUESTED.swap(false, Ordering::SeqCst) {
        UI.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(ui) = slot.as_ref() {
                // Already open — just bring to front.
                unsafe { ui.window.makeKeyAndOrderFront(None) };
            } else {
                let ui = build_ui(mtm);
                unsafe { ui.window.makeKeyAndOrderFront(None) };
                *slot = Some(ui);
                WAS_VISIBLE.with(|v| *v.borrow_mut() = true);
            }
        });
        return;
    }

    // Detect close → save.
    UI.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(ui) = slot.as_ref() else { return };
        let visible = unsafe { ui.window.isVisible() };
        let was = WAS_VISIBLE.with(|v| *v.borrow());
        if was && !visible {
            save_from_ui(ui);
            WAS_VISIBLE.with(|v| *v.borrow_mut() = false);
            *slot = None;
        } else {
            WAS_VISIBLE.with(|v| *v.borrow_mut() = visible);
        }
    });
}

const W: f64 = 440.0;
const H: f64 = 400.0;
const PAD: f64 = 22.0;
const FIELD_W: f64 = W - 2.0 * PAD;

fn label(mtm: MainThreadMarker, text: &str, y: f64) -> Retained<NSTextField> {
    let l = unsafe {
        NSTextField::labelWithString(&NSString::from_str(text), mtm)
    };
    unsafe {
        l.setFrame(NSRect::new(NSPoint::new(PAD, y), NSSize::new(FIELD_W, 16.0)));
    }
    l
}

fn text_field(mtm: MainThreadMarker, value: &str, y: f64, secure: bool) -> Retained<NSTextField> {
    let f = if secure {
        // Use a plain field even for the token (secure field adds friction in a VM);
        // could swap to NSSecureTextField if desired.
        unsafe { NSTextField::initWithFrame(NSTextField::alloc(mtm), NSRect::new(NSPoint::new(PAD, y), NSSize::new(FIELD_W, 24.0))) }
    } else {
        unsafe { NSTextField::initWithFrame(NSTextField::alloc(mtm), NSRect::new(NSPoint::new(PAD, y), NSSize::new(FIELD_W, 24.0))) }
    };
    unsafe {
        f.setStringValue(&NSString::from_str(value));
        f.setBezeled(true);
        f.setEditable(true);
        f.setSelectable(true);
    }
    f
}

fn checkbox(mtm: MainThreadMarker, title: &str, checked: bool, y: f64) -> Retained<NSButton> {
    let b = unsafe {
        NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
    };
    unsafe {
        b.setFrame(NSRect::new(NSPoint::new(PAD, y), NSSize::new(FIELD_W, 20.0)));
        b.setState(if checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }
    b
}

fn build_ui(mtm: MainThreadMarker) -> SettingsUi {
    let cfg = Config::load();

    let window = unsafe {
        let w = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(W, H)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        );
        w.setTitle(&NSString::from_str("Voice Typer — Settings"));
        w.center();
        w.setReleasedWhenClosed(false);
        w
    };
    let content = unsafe { window.contentView() }.expect("settings content view");

    // Lay out top-down (macOS y grows upward, so start near the top = H - ...).
    let mut y = H - 44.0;

    unsafe { content.addSubview(&label(mtm, "Hotkey (push-to-talk)", y)) };
    y -= 22.0;
    let hotkey = text_field(mtm, &cfg.hotkey, y, false);
    unsafe { content.addSubview(&hotkey) };
    y -= 16.0;
    unsafe { content.addSubview(&label(mtm, "e.g. f9 · mouse4 · mouse5 · ctrl+shift+f9", y)) };
    y -= 34.0;

    unsafe { content.addSubview(&label(mtm, "Access token", y)) };
    y -= 22.0;
    let token = text_field(mtm, &cfg.proxy_token, y, true);
    unsafe { content.addSubview(&token) };
    y -= 16.0;
    unsafe { content.addSubview(&label(mtm, "Paste from your dashboard. One device per token.", y)) };
    y -= 34.0;

    unsafe { content.addSubview(&label(mtm, "Microphone", y)) };
    y -= 26.0;
    let device = unsafe {
        NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(PAD, y), NSSize::new(FIELD_W, 26.0)),
            false,
        )
    };
    let mut devices: Vec<String> = vec![String::new()];
    unsafe { device.addItemWithTitle(&NSString::from_str("(system default)")) };
    let mut selected = 0isize;
    for (i, name) in Recorder::list_input_devices().into_iter().enumerate() {
        unsafe { device.addItemWithTitle(&NSString::from_str(&name)) };
        if !cfg.device_name.is_empty() && name.contains(&cfg.device_name) {
            selected = (i + 1) as isize;
        }
        devices.push(name);
    }
    unsafe {
        device.selectItemAtIndex(selected);
        content.addSubview(&device);
    }
    y -= 36.0;

    let paste = checkbox(mtm, "Paste transcript at cursor", cfg.paste_after_transcribe, y);
    unsafe { content.addSubview(&paste) };
    y -= 26.0;
    let show_popup = checkbox(mtm, "Show popup indicator", cfg.show_popup, y);
    unsafe { content.addSubview(&show_popup) };
    y -= 26.0;
    let streaming = checkbox(mtm, "Streaming mode (live transcription)", cfg.streaming_mode, y);
    unsafe { content.addSubview(&streaming) };
    y -= 26.0;
    let overlay = checkbox(mtm, "Show live transcript overlay", cfg.show_live_overlay, y);
    unsafe { content.addSubview(&overlay) };
    y -= 40.0;

    unsafe {
        content.addSubview(&label(mtm, "Close this window to save.", y));
    }

    SettingsUi {
        window,
        hotkey,
        token,
        device,
        devices,
        paste,
        show_popup,
        streaming,
        overlay,
    }
}

fn save_from_ui(ui: &SettingsUi) {
    let mut cfg = Config::load();

    let hk = unsafe { ui.hotkey.stringValue() }.to_string();
    let hk = hk.trim().to_lowercase();
    cfg.hotkey = if hk.is_empty() { "f9".into() } else { hk };

    cfg.proxy_token = unsafe { ui.token.stringValue() }.to_string().trim().to_string();

    let dev_idx = unsafe { ui.device.indexOfSelectedItem() };
    cfg.device_name = if dev_idx <= 0 {
        String::new()
    } else {
        ui.devices.get(dev_idx as usize).cloned().unwrap_or_default()
    };

    let on = |b: &NSButton| unsafe { b.state() } == NSControlStateValueOn;
    cfg.paste_after_transcribe = on(&ui.paste);
    cfg.show_popup = on(&ui.show_popup);
    cfg.streaming_mode = on(&ui.streaming);
    cfg.show_live_overlay = on(&ui.overlay);

    if let Err(e) = cfg.save() {
        log::error!("settings save: {e}");
        return;
    }
    log::info!("settings saved (macOS)");
    if let Some(tx) = notify_slot().lock().as_ref() {
        let _ = tx.send(());
    }
}
