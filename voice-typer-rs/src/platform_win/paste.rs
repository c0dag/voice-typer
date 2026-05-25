//! Paste text at the focused cursor:
//!   1) put text on the clipboard via arboard,
//!   2) synthesize Ctrl+V via SendInput.
//! The text remains on the clipboard so Win+V (clipboard history) keeps it.
use std::thread;
use std::time::Duration;

use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_V,
};

pub fn paste(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    {
        let mut cb = arboard::Clipboard::new()?;
        cb.set_text(text)?;
    }
    // Tiny delay so the clipboard is settled before we send Ctrl+V
    thread::sleep(Duration::from_millis(50));

    unsafe { send_ctrl_v()? };
    Ok(())
}

unsafe fn send_ctrl_v() -> Result<()> {
    let inputs = [
        kb_event(VK_CONTROL, false),
        kb_event(VK_V, false),
        kb_event(VK_V, true),
        kb_event(VK_CONTROL, true),
    ];
    let n = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    if n == 0 {
        return Err(anyhow::anyhow!("SendInput returned 0"));
    }
    Ok(())
}

unsafe fn kb_event(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    let flags = if key_up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
