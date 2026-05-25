//! Transactional email via Resend's REST API (no SDK). Only used for email
//! verification right now. A missing API key disables sending (and the callers
//! treat verification as disabled), so the server runs fine without it.

use anyhow::{anyhow, Context, Result};
use serde_json::json;

use crate::config::Config;

const RESEND_API: &str = "https://api.resend.com/emails";

/// Send the "verify your email" message with a one-click link. Errors if Resend
/// is not configured or the API call fails.
pub async fn send_verification(
    http: &reqwest::Client,
    cfg: &Config,
    to_email: &str,
    verify_url: &str,
) -> Result<()> {
    let key = cfg
        .resend_api_key
        .as_deref()
        .ok_or_else(|| anyhow!("email is not configured"))?;

    let safe_url = html_escape(verify_url);
    let html = format!(
        "<div style=\"font-family:-apple-system,Segoe UI,sans-serif;font-size:15px;color:#1d1d1f\">\
           <h2 style=\"font-weight:600\">Confirm your email</h2>\
           <p>Thanks for signing up for Voice Typer. Click the button to verify your email and start your free trial.</p>\
           <p style=\"margin:24px 0\"><a href=\"{safe_url}\" \
             style=\"background:#0066cc;color:#fff;text-decoration:none;padding:12px 22px;border-radius:999px;font-weight:600\">\
             Verify email</a></p>\
           <p style=\"color:#6e6e73;font-size:13px\">If the button does not work, paste this link into your browser:<br>{safe_url}</p>\
           <p style=\"color:#6e6e73;font-size:13px\">If you did not create this account, you can ignore this email.</p>\
         </div>"
    );

    let resp = http
        .post(RESEND_API)
        .bearer_auth(key)
        .json(&json!({
            "from": cfg.email_from,
            "to": [to_email],
            "subject": "Verify your email for Voice Typer",
            "html": html,
        }))
        .send()
        .await
        .context("resend: send request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("resend returned {status}: {body}"));
    }
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
