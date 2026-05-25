//! File logger to `%APPDATA%\VoiceTyper\voice-typer.log` (windowed app has no console).
use std::path::PathBuf;

pub fn log_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("VoiceTyper");
    let _ = std::fs::create_dir_all(&p);
    p.push("voice-typer.log");
    p
}

pub fn init() -> anyhow::Result<()> {
    let path = log_path();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {:5} {}: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .level_for("tao", log::LevelFilter::Warn)
        .level_for("tray_icon", log::LevelFilter::Warn)
        .chain(file)
        .apply()?;
    Ok(())
}
