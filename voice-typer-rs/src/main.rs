//! Voice Typer — push-to-talk transcription via Deepgram cloud.
//!
//! Threads:
//!   - main:   Win32 message pump (popup window + tray-icon)
//!   - hooks:  WH_KEYBOARD_LL / WH_MOUSE_LL
//!   - worker: hotkey + popup + tray actions
//!   - per-utterance worker: HTTP POST to Deepgram + paste
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod audio;
mod config;
mod deepgram;
mod logging;
mod streaming;
mod tray;

#[cfg(windows)]
#[path = "platform_win/hotkey.rs"]
mod hotkey;
#[cfg(windows)]
#[path = "platform_win/popup.rs"]
mod popup;
#[cfg(windows)]
#[path = "platform_win/paste.rs"]
mod paste;
#[cfg(windows)]
#[path = "platform_win/overlay.rs"]
mod overlay;
#[cfg(windows)]
#[path = "platform_win/render.rs"]
mod render;
#[cfg(windows)]
#[path = "platform_win/settings.rs"]
mod settings;

#[cfg(target_os = "macos")]
#[path = "platform_mac/hotkey.rs"]
mod hotkey;
#[cfg(target_os = "macos")]
#[path = "platform_mac/popup.rs"]
mod popup;
#[cfg(target_os = "macos")]
#[path = "platform_mac/paste.rs"]
mod paste;
#[cfg(target_os = "macos")]
#[path = "platform_mac/overlay.rs"]
mod overlay;
#[cfg(target_os = "macos")]
#[path = "platform_mac/render.rs"]
mod render;
#[cfg(target_os = "macos")]
#[path = "platform_mac/settings.rs"]
mod settings;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::Mutex;

use crate::audio::Recorder;
use crate::config::Config;
use crate::hotkey::HotkeyEvent;
use crate::popup::PopupAction;
use crate::render::PopupState;
use crate::tray::TrayAction;

fn main() {
    if let Err(e) = real_main() {
        log::error!("fatal: {e:?}");
    }
}

fn real_main() -> Result<()> {
    let _ = logging::init();
    log::info!("=== voice-typer (proxy) start ===");

    let cfg = Arc::new(Mutex::new(Config::load()));
    {
        let g = cfg.lock();
        log::info!(
            "config: hotkey={} token={} proxy={} (fixed)",
            g.hotkey,
            if g.proxy_token.is_empty() { "(missing)" } else { "(set)" },
            crate::config::PROXY_URL,
        );
    }

    let (hotkey_tx, hotkey_rx) = channel::<HotkeyEvent>();
    let (popup_action_tx, popup_action_rx) = channel::<PopupAction>();
    let (tray_action_tx, tray_action_rx) = channel::<TrayAction>();
    let (settings_tx, settings_rx) = channel::<()>();

    // Hotkey hook setup
    let hk = cfg.lock().hotkey.clone();
    if let Some(token) = hotkey::parse_hotkey(&hk) {
        hotkey::set_hotkey(token);
    } else {
        log::warn!("could not parse hotkey '{hk}', defaulting to f9");
        if let Some(t) = hotkey::parse_hotkey("f9") {
            hotkey::set_hotkey(t);
        }
    }
    hotkey::spawn(hotkey_tx);

    // Live transcript overlay (separate thread + message pump)
    overlay::set_enabled(cfg.lock().show_live_overlay);
    overlay::spawn();

    // Worker thread
    let worker_cfg = Arc::clone(&cfg);
    let worker_settings_tx = settings_tx.clone();
    thread::Builder::new()
        .name("app-worker".into())
        .spawn(move || {
            run_worker(
                worker_cfg,
                hotkey_rx,
                popup_action_rx,
                tray_action_rx,
                settings_rx,
                worker_settings_tx,
            );
        })
        .expect("spawn worker");

    // Tray
    let _tray = match tray::create(tray_action_tx.clone()) {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("tray icon failed: {e}");
            None
        }
    };

    // Popup runs the Win32 message pump on this (main) thread.
    if let Err(e) = popup::run(popup_action_tx) {
        log::error!("popup error: {e:?}");
    }

    log::info!("popup loop exited; shutting down");
    Ok(())
}

fn run_worker(
    cfg: Arc<Mutex<Config>>,
    hotkey_rx: Receiver<HotkeyEvent>,
    popup_action_rx: Receiver<PopupAction>,
    tray_action_rx: Receiver<TrayAction>,
    settings_rx: Receiver<()>,
    settings_tx: Sender<()>,
) {
    // Recorder stays on this (worker) thread because cpal::Stream is !Send.
    let mut recorder = Recorder::new();
    let busy = Arc::new(AtomicBool::new(false));
    // press-and-hold debounce state
    let mut pending_press_at: Option<Instant> = None;
    let mut recording_started: bool = false;
    // Active streaming session (Some when streaming_mode is on and recording).
    let mut active_stream: Option<crate::streaming::StreamSession> = None;

    // Apply initial popup visibility from config
    popup::set_show_popup(cfg.lock().show_popup);

    loop {
        // Hotkey events (Press/Release)
        while let Ok(ev) = hotkey_rx.try_recv() {
            match ev {
                HotkeyEvent::Press => {
                    if busy.load(Ordering::SeqCst) || recording_started || pending_press_at.is_some() {
                        continue;
                    }
                    pending_press_at = Some(Instant::now());
                }
                HotkeyEvent::Release => {
                    if recording_started {
                        recording_started = false;

                        if let Some(stream) = active_stream.take() {
                            // STREAMING path: stop the cpal stream (closes audio_rx for
                            // the sender thread), wait for Deepgram finalization, paste.
                            recorder.set_chunk_tap(None);
                            let _ = recorder.stop();
                            let cfg2 = Arc::clone(&cfg);
                            let busy2 = Arc::clone(&busy);
                            thread::spawn(move || finish_streaming(stream, cfg2, busy2));
                        } else {
                            // BATCH path (existing behavior)
                            let audio = recorder.stop();
                            let sr = recorder.output_sample_rate();
                            let cfg2 = Arc::clone(&cfg);
                            let busy2 = Arc::clone(&busy);
                            thread::spawn(move || finish(audio, sr, cfg2, busy2));
                        }
                    } else if pending_press_at.is_some() {
                        // Quick tap below the hold threshold: silently ignore.
                        log::debug!("hotkey tap below min_hold_ms — ignored");
                        pending_press_at = None;
                    }
                }
            }
        }

        // Debounced start: only after the key has been held continuously for min_hold_ms
        if let Some(t0) = pending_press_at {
            let min_ms = cfg.lock().min_hold_ms;
            if t0.elapsed() >= Duration::from_millis(min_ms) {
                pending_press_at = None;
                let (dev, streaming_mode, proxy_token) = {
                    let g = cfg.lock();
                    (
                        g.device_name.clone(),
                        g.streaming_mode,
                        g.proxy_token.clone(),
                    )
                };
                match recorder.start(&dev) {
                    Ok(()) => {
                        recording_started = true;
                        popup::set_state_async(PopupState::Recording);

                        if streaming_mode {
                            let sr = recorder.source_sample_rate();
                            let (atx, arx) = channel::<Vec<f32>>();
                            recorder.set_chunk_tap(Some(atx));
                            match crate::streaming::StreamSession::start(
                                proxy_token, sr, arx,
                            ) {
                                Ok(s) => {
                                    active_stream = Some(s);
                                    overlay::set_text("");
                                    overlay::show();
                                }
                                Err(e) => {
                                    log::error!("streaming start: {e}");
                                    recorder.set_chunk_tap(None);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("mic start: {e}");
                    }
                }
            }
        }

        while let Ok(a) = popup_action_rx.try_recv() {
            match a {
                PopupAction::OpenSettings => settings::spawn(settings_tx.clone()),
                PopupAction::Quit => {
                    popup::quit_async();
                    return;
                }
            }
        }

        while let Ok(a) = tray_action_rx.try_recv() {
            match a {
                TrayAction::OpenSettings => settings::spawn(settings_tx.clone()),
                TrayAction::Quit => {
                    popup::quit_async();
                    return;
                }
            }
        }

        // Settings reload
        while settings_rx.try_recv().is_ok() {
            let new_cfg = Config::load();
            log::info!(
                "config reloaded: hotkey={} token={}",
                new_cfg.hotkey,
                if new_cfg.proxy_token.is_empty() { "(missing)" } else { "(set)" }
            );
            if let Some(token) = hotkey::parse_hotkey(&new_cfg.hotkey) {
                hotkey::set_hotkey(token);
            }
            popup::set_show_popup(new_cfg.show_popup);
            overlay::set_enabled(new_cfg.show_live_overlay);
            *cfg.lock() = new_cfg;
        }

        thread::sleep(Duration::from_millis(15));
    }
}

fn finish(audio: Vec<f32>, sample_rate: u32, cfg: Arc<Mutex<Config>>, busy: Arc<AtomicBool>) {
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let _guard = scopeguard::ScopeGuard::new(&busy);

    let (proxy_token, paste_enabled, min_seconds);
    {
        let g = cfg.lock();
        proxy_token = g.proxy_token.clone();
        paste_enabled = g.paste_after_transcribe;
        min_seconds = g.min_recording_seconds;
    }

    let duration = audio.len() as f32 / sample_rate as f32;
    if audio.is_empty() || duration < min_seconds {
        log::info!("recording too short ({:.2}s)", duration);
        popup::set_state_async(PopupState::Idle);
        busy.store(false, Ordering::SeqCst);
        return;
    }

    popup::set_state_async(PopupState::Working);

    let text = match deepgram::transcribe(&proxy_token, &audio, sample_rate) {
        Ok(t) => t,
        Err(e) => {
            log::error!("transcribe: {e}");
            popup::set_state_async(PopupState::Idle);
            busy.store(false, Ordering::SeqCst);
            return;
        }
    };

    if !text.is_empty() && paste_enabled {
        if let Err(e) = paste::paste(&text) {
            log::error!("paste: {e}");
        }
    }

    popup::set_state_async(PopupState::Idle);
    busy.store(false, Ordering::SeqCst);
}

fn finish_streaming(
    session: crate::streaming::StreamSession,
    cfg: Arc<Mutex<Config>>,
    busy: Arc<AtomicBool>,
) {
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    popup::set_state_async(PopupState::Working);

    let text = session.finish();
    let paste_enabled = cfg.lock().paste_after_transcribe;

    if !text.trim().is_empty() && paste_enabled {
        if let Err(e) = paste::paste(text.trim()) {
            log::error!("paste (streaming): {e}");
        }
    }
    overlay::hide();
    popup::set_state_async(PopupState::Idle);
    busy.store(false, Ordering::SeqCst);
}

mod scopeguard {
    /// Tiny RAII helper so `busy` always gets reset even on panic. Not used right
    /// now (we manually clear) but kept as a hedge.
    pub struct ScopeGuard<'a, T>(&'a T);
    impl<'a, T> ScopeGuard<'a, T> {
        pub fn new(t: &'a T) -> Self {
            Self(t)
        }
    }
}
