//! macOS live-transcript overlay state.
//!
//! The actual AppKit window is owned and driven by the `popup` module (which
//! runs the single NSApplication run loop on the main thread). This module only
//! holds the shared state that `popup`'s main-thread timer reads each tick.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static ENABLED: AtomicBool = AtomicBool::new(true);
static VISIBLE: AtomicBool = AtomicBool::new(false);

fn text_slot() -> &'static Mutex<String> {
    static TEXT: OnceLock<Mutex<String>> = OnceLock::new();
    TEXT.get_or_init(|| Mutex::new(String::new()))
}

pub fn set_enabled(b: bool) {
    ENABLED.store(b, Ordering::SeqCst);
}

pub fn show() {
    if ENABLED.load(Ordering::SeqCst) {
        VISIBLE.store(true, Ordering::SeqCst);
    }
}

pub fn hide() {
    VISIBLE.store(false, Ordering::SeqCst);
}

pub fn set_text(s: &str) {
    *text_slot().lock() = s.to_string();
}

pub fn spawn() {
    // No separate thread: the AppKit overlay window lives in popup::run on the
    // main thread. Nothing to start here.
}

// --- Read side, used by popup's main-thread timer ---

pub(crate) fn is_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

pub(crate) fn is_visible() -> bool {
    VISIBLE.load(Ordering::SeqCst) && ENABLED.load(Ordering::SeqCst)
}

pub(crate) fn current_text() -> String {
    text_slot().lock().clone()
}
