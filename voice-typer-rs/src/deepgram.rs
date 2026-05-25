//! Batch transcription via the Voice Typer proxy server.
//!
//! Proxy URL, model and language are fixed constants — only the per-user
//! `token` is configurable from the client. The proxy server forwards to
//! Deepgram with its own API key; the Deepgram key never reaches this client.

use anyhow::{anyhow, Context, Result};

use crate::config::{DEEPGRAM_MODEL, LANGUAGE, PROXY_URL};

/// POST raw 16-bit PCM mono @ `sample_rate` Hz to `{PROXY_URL}/api/transcribe`
/// authenticated with `token`. Returns the plain transcript text.
pub fn transcribe(token: &str, samples: &[f32], sample_rate: u32) -> Result<String> {
    if token.trim().is_empty() {
        return Err(anyhow!("Proxy token is not set (open Settings)"));
    }
    if samples.is_empty() {
        return Ok(String::new());
    }

    let pcm = pcm_le_bytes(samples);
    log::info!(
        "proxy: POSTing {} samples ({:.2}s @ {}Hz, model={}, lang={})",
        samples.len(),
        samples.len() as f32 / sample_rate as f32,
        sample_rate,
        DEEPGRAM_MODEL,
        LANGUAGE,
    );

    let url = format!(
        "{base}/api/transcribe?model={model}&language={lang}&sample_rate={sample_rate}&encoding=linear16",
        base = PROXY_URL.trim_end_matches('/'),
        model = urlencoded(DEEPGRAM_MODEL),
        lang = urlencoded(LANGUAGE),
    );

    let t0 = std::time::Instant::now();
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .set("Content-Type", "audio/raw")
        .timeout(std::time::Duration::from_secs(60))
        .send_bytes(&pcm);

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            let snippet: String = body.chars().take(300).collect();
            return Err(anyhow!("Proxy HTTP {code}: {snippet}"));
        }
        Err(e) => return Err(anyhow!("Proxy request failed: {e}")),
    };

    let json: serde_json::Value = resp.into_json().context("parse proxy JSON")?;
    let transcript = json
        .pointer("/results/channels/0/alternatives/0/transcript")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    log::info!(
        "proxy: {:.2}s round-trip, {} chars",
        t0.elapsed().as_secs_f32(),
        transcript.chars().count()
    );
    Ok(transcript)
}

fn pcm_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let clamped = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        buf.extend_from_slice(&clamped.to_le_bytes());
    }
    buf
}

fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
