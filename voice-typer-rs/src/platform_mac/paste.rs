//! Paste text at the focused cursor on macOS:
//!   1) put text on NSPasteboard via arboard,
//!   2) synthesize Cmd+V via CGEvent.
//!
//! Synthesizing key events requires the app to be trusted for **Accessibility**
//! (System Settings > Privacy & Security > Accessibility). It's the only OS
//! permission Voice Typer needs beyond the microphone, and only for auto-paste.
//! If the app isn't trusted yet, a posted Cmd+V is silently dropped, so on the
//! first failed paste we trigger the system Accessibility prompt (which also
//! adds the app to the list). The text always lands on the clipboard first, so
//! the user can paste manually with Cmd+V until the permission is granted.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};

const KVK_V: u16 = 0x09;

/// Whether we've already shown the one-time Accessibility prompt this session.
static PROMPTED: AtomicBool = AtomicBool::new(false);

pub fn paste(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    // Always stage the text on the clipboard first, so it's recoverable with a
    // manual Cmd+V even when auto-paste can't run yet.
    {
        let mut cb = arboard::Clipboard::new()?;
        cb.set_text(text)?;
    }

    // Auto-paste needs Accessibility. Prompt once if we're not trusted.
    if !accessibility_trusted() {
        let first = !PROMPTED.swap(true, Ordering::SeqCst);
        if first {
            prompt_for_accessibility();
        }
        return Err(anyhow!(
            "Accessibility not granted yet — text is on the clipboard (paste with Cmd+V). \
             Enable Voice Typer under System Settings > Privacy & Security > Accessibility, \
             then relaunch to auto-paste."
        ));
    }

    thread::sleep(Duration::from_millis(50));
    send_cmd_v()
}

fn send_cmd_v() -> Result<()> {
    use objc2_core_graphics::{
        CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    };

    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .ok_or_else(|| anyhow!("CGEventSource::new returned null"))?;

    let down = CGEvent::new_keyboard_event(Some(&src), KVK_V, true)
        .ok_or_else(|| anyhow!("CGEvent keydown returned null"))?;
    let up = CGEvent::new_keyboard_event(Some(&src), KVK_V, false)
        .ok_or_else(|| anyhow!("CGEvent keyup returned null"))?;

    CGEvent::set_flags(Some(&down), CGEventFlags::MaskCommand);
    CGEvent::set_flags(Some(&up), CGEventFlags::MaskCommand);
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
    Ok(())
}

// --- Accessibility (AX) trust check + one-time prompt --------------------

#[repr(C)]
struct CFCallbacks {
    _opaque: [u8; 0],
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: CFCallbacks;
    static kCFTypeDictionaryValueCallBacks: CFCallbacks;
    static kCFBooleanTrue: *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize, // CFIndex
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

#[allow(non_upper_case_globals)]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: *const c_void; // CFStringRef
    fn AXIsProcessTrusted() -> u8; // Boolean (unsigned char)
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
}

fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Show the system Accessibility prompt and add the app to the list (greyed
/// until the user flips the toggle). Safe to call when already trusted.
fn prompt_for_accessibility() {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let opts = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const CFCallbacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const CFCallbacks as *const c_void,
        );
        let _ = AXIsProcessTrustedWithOptions(opts);
        if !opts.is_null() {
            CFRelease(opts);
        }
        log::info!("paste: requested Accessibility permission (system prompt shown)");
    }
}
