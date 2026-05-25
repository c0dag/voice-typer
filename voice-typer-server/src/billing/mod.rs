//! Stripe billing: hosted Checkout, Customer Portal, and webhook handling.
//!
//! We talk to the Stripe REST API directly with `reqwest` (no SDK) to keep the
//! binary small. The secret key authenticates via Bearer. Webhook payloads are
//! verified with the `Stripe-Signature` HMAC-SHA256 scheme before we trust them.

pub mod routes;

use anyhow::{anyhow, Context, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::SqlitePool;

use crate::config::Config;

const STRIPE_API: &str = "https://api.stripe.com/v1";

/// The subscription fields we care about, pulled from a Stripe subscription.
pub struct SubInfo {
    pub status: String,
    pub price_id: Option<String>,
    pub period_start: Option<i64>,
    pub period_end: Option<i64>,
}

fn secret(cfg: &Config) -> Result<&str> {
    cfg.stripe_secret_key
        .as_deref()
        .ok_or_else(|| anyhow!("stripe is not configured"))
}

/// Create a subscription Checkout Session and return its hosted URL.
pub async fn checkout_session_url(
    http: &reqwest::Client,
    cfg: &Config,
    price_id: &str,
    user_id: i64,
    email: &str,
) -> Result<String> {
    let sk = secret(cfg)?;
    let success = format!(
        "{}/billing/success?session_id={{CHECKOUT_SESSION_ID}}",
        cfg.public_base_url
    );
    let cancel = format!("{}/billing/cancel", cfg.public_base_url);
    let uid = user_id.to_string();
    let params: Vec<(&str, &str)> = vec![
        ("mode", "subscription"),
        ("line_items[0][price]", price_id),
        ("line_items[0][quantity]", "1"),
        ("success_url", success.as_str()),
        ("cancel_url", cancel.as_str()),
        ("client_reference_id", uid.as_str()),
        ("customer_email", email),
        ("subscription_data[metadata][user_id]", uid.as_str()),
        ("allow_promotion_codes", "true"),
    ];
    let v: serde_json::Value = http
        .post(format!("{STRIPE_API}/checkout/sessions"))
        .bearer_auth(sk)
        .form(&params)
        .send()
        .await
        .context("stripe: create checkout session")?
        .json()
        .await
        .context("stripe: checkout session json")?;
    v["url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("stripe checkout: no url in response: {v}"))
}

/// Create a Billing (Customer) Portal session and return its URL.
pub async fn portal_session_url(
    http: &reqwest::Client,
    cfg: &Config,
    customer_id: &str,
) -> Result<String> {
    let sk = secret(cfg)?;
    let return_url = format!("{}/dashboard", cfg.public_base_url);
    let params: Vec<(&str, &str)> = vec![
        ("customer", customer_id),
        ("return_url", return_url.as_str()),
    ];
    let v: serde_json::Value = http
        .post(format!("{STRIPE_API}/billing_portal/sessions"))
        .bearer_auth(sk)
        .form(&params)
        .send()
        .await
        .context("stripe: create portal session")?
        .json()
        .await
        .context("stripe: portal session json")?;
    v["url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("stripe portal: no url in response: {v}"))
}

/// Fetch a subscription by id and extract the fields we track.
pub async fn fetch_subscription(
    http: &reqwest::Client,
    cfg: &Config,
    sub_id: &str,
) -> Result<SubInfo> {
    let sk = secret(cfg)?;
    let v: serde_json::Value = http
        .get(format!("{STRIPE_API}/subscriptions/{sub_id}"))
        .bearer_auth(sk)
        .send()
        .await
        .context("stripe: fetch subscription")?
        .json()
        .await
        .context("stripe: subscription json")?;
    Ok(sub_info_from_value(&v))
}

/// Parse a Stripe subscription JSON object into `SubInfo`. Newer Stripe API
/// versions expose the billing period on the subscription item rather than the
/// subscription root, so we read the root with a fallback to the first item.
pub fn sub_info_from_value(v: &serde_json::Value) -> SubInfo {
    let item = &v["items"]["data"][0];
    SubInfo {
        status: v["status"].as_str().unwrap_or_default().to_string(),
        price_id: item["price"]["id"].as_str().map(str::to_string),
        period_start: v["current_period_start"]
            .as_i64()
            .or_else(|| item["current_period_start"].as_i64()),
        period_end: v["current_period_end"]
            .as_i64()
            .or_else(|| item["current_period_end"].as_i64()),
    }
}

/// Verify a Stripe webhook signature (`Stripe-Signature` header) against the
/// raw request body using the endpoint's signing secret. Returns true if valid.
pub fn verify_webhook_signature(payload: &[u8], sig_header: &str, secret: &str) -> bool {
    let mut ts: Option<&str> = None;
    let mut sigs: Vec<&str> = Vec::new();
    for part in sig_header.split(',') {
        let mut kv = part.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("t"), Some(v)) => ts = Some(v),
            (Some("v1"), Some(v)) => sigs.push(v),
            _ => {}
        }
    }
    let Some(t) = ts else { return false };
    // Reject stale timestamps (replay protection), allowing for clock skew.
    match t.parse::<i64>() {
        Ok(tsec) => {
            if (chrono::Utc::now().timestamp() - tsec).abs() > 600 {
                return false;
            }
        }
        Err(_) => return false,
    }
    for v1 in sigs {
        let Ok(sig_bytes) = hex::decode(v1) else { continue };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
            return false;
        };
        mac.update(t.as_bytes());
        mac.update(b".");
        mac.update(payload);
        if mac.verify_slice(&sig_bytes).is_ok() {
            return true;
        }
    }
    false
}

/// Format a Unix timestamp as the same text shape SQLite's `datetime('now')`
/// uses ("YYYY-MM-DD HH:MM:SS"), so it compares lexically with `usage_log.at`.
fn unix_to_sqlite(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// Persist subscription state onto a user row. `customer_id`/`sub_id` are only
/// overwritten when provided (COALESCE), so partial events do not wipe them.
#[allow(clippy::too_many_arguments)]
pub async fn set_user_subscription(
    db: &SqlitePool,
    user_id: i64,
    customer_id: Option<&str>,
    sub_id: Option<&str>,
    status: &str,
    plan: Option<&str>,
    minutes: i64,
    period_start: Option<i64>,
    period_end: Option<i64>,
) -> Result<()> {
    let ps = period_start.map(unix_to_sqlite);
    let pe = period_end.map(unix_to_sqlite);
    sqlx::query(
        r#"UPDATE users SET
             stripe_customer_id     = COALESCE(?2, stripe_customer_id),
             stripe_subscription_id = COALESCE(?3, stripe_subscription_id),
             subscription_status    = ?4,
             plan                   = ?5,
             monthly_minute_quota   = ?6,
             period_start           = ?7,
             period_end             = ?8
           WHERE id = ?1"#,
    )
    .bind(user_id)
    .bind(customer_id)
    .bind(sub_id)
    .bind(status)
    .bind(plan)
    .bind(minutes)
    .bind(ps)
    .bind(pe)
    .execute(db)
    .await
    .context("update user subscription")?;
    Ok(())
}

pub async fn user_id_by_subscription(db: &SqlitePool, sub_id: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT id FROM users WHERE stripe_subscription_id = ?1")
        .bind(sub_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

pub async fn user_id_by_customer(db: &SqlitePool, customer_id: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT id FROM users WHERE stripe_customer_id = ?1")
        .bind(customer_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}
