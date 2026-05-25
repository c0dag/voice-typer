//! macOS popup (state indicator) + live-transcript overlay.
//!
//! Owns the single NSApplication run loop on the main thread. Both windows are
//! borderless, transparent, non-activating panels floating above everything.
//! A repeating main-thread NSTimer polls the shared atomics (popup state +
//! overlay text/visibility, the latter living in the `overlay` module) and
//! updates the AppKit views accordingly — this lets worker threads change state
//! via the lock-free setters below without touching AppKit off the main thread.

use crate::render::PopupState;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont, NSImage,
    NSImageScaling, NSImageView, NSLineBreakMode, NSPanel, NSScreen, NSTextField, NSTextAlignment,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer};

#[derive(Debug)]
pub enum PopupAction {
    OpenSettings,
    Quit,
}

static SHOW_POPUP: AtomicBool = AtomicBool::new(true);
static STATE: AtomicU8 = AtomicU8::new(0);
static SHOULD_QUIT: AtomicBool = AtomicBool::new(false);

pub fn set_show_popup(b: bool) {
    SHOW_POPUP.store(b, Ordering::SeqCst);
}

pub fn set_state_async(state: PopupState) {
    STATE.store(state as u8, Ordering::SeqCst);
}

pub fn quit_async() {
    SHOULD_QUIT.store(true, Ordering::SeqCst);
}

/// Build a borderless, transparent, floating, non-activating panel.
fn make_panel(mtm: MainThreadMarker, rect: NSRect) -> Retained<NSPanel> {
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel = unsafe {
        NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(true);
        // Float above normal windows (status-bar level is plenty).
        panel.setLevel(objc2_app_kit::NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
    }
    panel
}

/// A non-editable, borderless label inside `panel`, filling it.
fn make_label(mtm: MainThreadMarker, frame: NSRect, font_size: f64) -> Retained<NSTextField> {
    let label = unsafe { NSTextField::initWithFrame(NSTextField::alloc(mtm), frame) };
    unsafe {
        label.setEditable(false);
        label.setSelectable(false);
        label.setBordered(false);
        label.setBezeled(false);
        label.setDrawsBackground(false);
        label.setFont(Some(&NSFont::systemFontOfSize(font_size)));
        label.setAlignment(NSTextAlignment::Center);
        let cell = label.cell();
        if let Some(cell) = cell {
            cell.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        }
    }
    label
}

pub fn run(_action_tx: Sender<PopupAction>) -> anyhow::Result<()> {
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("popup::run must be called on the main thread");
        return Ok(());
    };

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // Screen geometry for positioning.
    let screen_frame = NSScreen::mainScreen(mtm)
        .map(|s| s.frame())
        .unwrap_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0)));
    let sw = screen_frame.size.width;

    // Render the three state badges (logo + colored ring) to PNGs once.
    let cache_dir = dirs::cache_dir()
        .unwrap_or(std::env::temp_dir())
        .join("VoiceTyper");
    let imgs: [Option<Retained<NSImage>>; 3] = match crate::render::write_state_pngs(&cache_dir) {
        Ok(paths) => {
            let load = |p: &std::path::Path| -> Option<Retained<NSImage>> {
                let s = NSString::from_str(&p.to_string_lossy());
                unsafe { NSImage::initWithContentsOfFile(NSImage::alloc(), &s) }
            };
            [load(&paths[0]), load(&paths[1]), load(&paths[2])]
        }
        Err(e) => {
            log::warn!("popup: could not render state PNGs: {e}");
            [None, None, None]
        }
    };

    // --- State badge popup: small panel, bottom-center, ~120px up ---
    // Match the Windows popup's compact footprint (~18px).
    let dot_size = 19.0;
    let dot_x = screen_frame.origin.x + (sw - dot_size) / 2.0;
    let dot_y = screen_frame.origin.y + 120.0;
    let dot_panel = make_panel(
        mtm,
        NSRect::new(NSPoint::new(dot_x, dot_y), NSSize::new(dot_size, dot_size)),
    );
    let dot_view = unsafe {
        NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(dot_size, dot_size)),
        )
    };
    unsafe {
        dot_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        if let Some(cv) = dot_panel.contentView() {
            cv.addSubview(&dot_view);
        }
    }

    // --- Overlay: wide panel just above the dot ---
    let ov_w = (sw * 0.6).min(720.0);
    let ov_h = 64.0;
    let ov_x = screen_frame.origin.x + (sw - ov_w) / 2.0;
    let ov_y = dot_y + dot_size + 16.0;
    let ov_panel = make_panel(
        mtm,
        NSRect::new(NSPoint::new(ov_x, ov_y), NSSize::new(ov_w, ov_h)),
    );
    let ov_label = make_label(
        mtm,
        NSRect::new(NSPoint::new(16.0, 8.0), NSSize::new(ov_w - 32.0, ov_h - 16.0)),
        18.0,
    );
    unsafe {
        ov_label.setTextColor(Some(&NSColor::whiteColor()));
        // Semi-transparent dark background drawn by the label itself.
        ov_label.setDrawsBackground(true);
        ov_label.setBackgroundColor(Some(&NSColor::colorWithCalibratedWhite_alpha(0.0, 0.72)));
        ov_label.setAlignment(NSTextAlignment::Left);
        if let Some(cv) = ov_panel.contentView() {
            cv.addSubview(&ov_label);
        }
    }

    // --- Main-thread timer: poll atomics and update UI ---
    let tick = RcBlock::new(move |_t: core::ptr::NonNull<NSTimer>| unsafe {
        let mtm = MainThreadMarker::new().unwrap();

        // Quit handling.
        if SHOULD_QUIT.load(Ordering::SeqCst) {
            NSApplication::sharedApplication(mtm).stop(None);
            return;
        }

        // Open/close the Settings window on the main thread when requested.
        crate::settings::poll(mtm);

        // Pick up hotkey changes (Settings) — Carbon (re)registration must run
        // on this main thread, where NSApplication pumps the hotkey events.
        crate::hotkey::poll_reregister();

        // Popup badge.
        let show = SHOW_POPUP.load(Ordering::SeqCst);
        let state = STATE.load(Ordering::SeqCst);
        if show && state != 0 {
            if let Some(img) = imgs.get(state as usize).and_then(|o| o.as_ref()) {
                dot_view.setImage(Some(img));
            }
            dot_panel.orderFrontRegardless();
        } else {
            dot_panel.orderOut(None);
        }

        // Overlay.
        if crate::overlay::is_visible() {
            let txt = crate::overlay::current_text();
            ov_label.setStringValue(&NSString::from_str(&txt));
            ov_panel.orderFrontRegardless();
        } else {
            ov_panel.orderOut(None);
        }
    });

    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.1, true, &tick);
    }

    // Register the global hotkey now that we're on the main thread with an
    // NSApplication run loop to pump Carbon hotkey events. No OS permission
    // needed (RegisterEventHotKey is exempt) and the key is consumed (no beep).
    crate::hotkey::install_main_thread();

    log::info!("popup: AppKit run loop starting");
    app.run();
    log::info!("popup: AppKit run loop exited");
    Ok(())
}
