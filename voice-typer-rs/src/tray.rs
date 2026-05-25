//! System tray icon (right-click menu: status, Settings, Quit).
use std::sync::mpsc::Sender;

use anyhow::{anyhow, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub enum TrayAction {
    OpenSettings,
    Quit,
}

pub struct TrayHandle {
    _icon: TrayIcon,
    pub settings_id: String,
    pub quit_id: String,
}

pub fn create(action_tx: Sender<TrayAction>) -> Result<TrayHandle> {
    let icon = load_icon()?;
    let menu = Menu::new();
    let settings = MenuItem::new("Settings…", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&settings).map_err(|e| anyhow!("{e}"))?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|e| anyhow!("{e}"))?;
    menu.append(&quit).map_err(|e| anyhow!("{e}"))?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("Voice Typer")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()?;

    let settings_id = settings.id().0.clone();
    let quit_id = quit.id().0.clone();

    // Forward menu events to the action channel
    let s_id = settings_id.clone();
    let q_id = quit_id.clone();
    let receiver = MenuEvent::receiver();
    std::thread::spawn(move || {
        for ev in receiver {
            if ev.id.0 == s_id {
                let _ = action_tx.send(TrayAction::OpenSettings);
            } else if ev.id.0 == q_id {
                let _ = action_tx.send(TrayAction::Quit);
            }
        }
    });

    Ok(TrayHandle {
        _icon: tray,
        settings_id,
        quit_id,
    })
}

fn load_icon() -> Result<Icon> {
    let bytes = include_bytes!("../assets/logo.png");
    let img = image::load_from_memory(bytes)?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok(Icon::from_rgba(img.into_raw(), w, h)?)
}

pub fn update_tooltip(handle: &TrayHandle, text: &str) {
    let _ = handle._icon.set_tooltip(Some(text));
}

pub fn open_config_in_notepad() {
    let path = crate::config::config_path();
    let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
}
