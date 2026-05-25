//! User config persisted at `%APPDATA%\VoiceTyper\config.json`.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Hotkey (push-to-talk). Examples: "f9", "f12", "pause", "mouse4", "mouse5".
    pub hotkey: String,
    /// Per-user API token issued by the proxy server (visible on the
    /// signup/dashboard page). Required.
    pub proxy_token: String,
    /// Microphone device name substring; empty = system default.
    pub device_name: String,
    /// Auto-paste after transcription via Ctrl+V.
    pub paste_after_transcribe: bool,
    /// Minimum recording duration to actually transcribe (seconds).
    pub min_recording_seconds: f32,
    /// Minimum hold duration before a press starts recording (milliseconds).
    /// Prevents accidental quick taps from triggering a transcription.
    pub min_hold_ms: u64,
    /// Show the floating indicator popup. False = invisible (no UI feedback).
    pub show_popup: bool,
    /// Real-time streaming transcription via WebSocket. While held, audio is
    /// streamed and an overlay shows the live transcript; on release, the
    /// final accumulated text is pasted.
    pub streaming_mode: bool,
    /// Show the live transcript overlay during streaming. No effect when
    /// `streaming_mode` is false.
    pub show_live_overlay: bool,
}

/// Proxy URL — fixed; cannot be overridden by the client.
pub const PROXY_URL: &str = "https://voiceapi.codag.site";
/// Deepgram model — fixed; the proxy decides which model is in use.
pub const DEEPGRAM_MODEL: &str = "nova-3";
/// Language — fixed; "multi" enables Deepgram's automatic language detection.
pub const LANGUAGE: &str = "multi";

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "f9".into(),
            proxy_token: String::new(),
            device_name: String::new(),
            paste_after_transcribe: true,
            min_recording_seconds: 0.3,
            min_hold_ms: 200,
            show_popup: true,
            streaming_mode: false,
            show_live_overlay: true,
        }
    }
}

pub fn config_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("VoiceTyper");
    let _ = std::fs::create_dir_all(&p);
    p.push("config.json");
    p
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("config parse failed ({e}), using defaults");
                Self::default()
            }),
            Err(_) => {
                let cfg = Self::default();
                let _ = cfg.save();
                cfg
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(path, s)?;
        Ok(())
    }
}
