// Embeds the application icon and version metadata into the EXE on Windows.
#[cfg(target_os = "windows")]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/logo.ico");
    res.set("ProductName", "Voice Typer");
    res.set("FileDescription", "Push-to-talk transcription");
    res.set("LegalCopyright", "Voice Typer");
    res.compile().expect("failed to embed icon");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
