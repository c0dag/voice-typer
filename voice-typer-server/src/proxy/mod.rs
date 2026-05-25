pub mod routes;
pub mod session_lock;

use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::http::{header, request::Parts};

/// Free trial allowance per account, in seconds (10 minutes). Accounts without
/// an active subscription can transcribe up to this much in total before they
/// must subscribe.
pub const TRIAL_SECONDS: f64 = 600.0;

pub async fn authenticate_token(state: &AppState, parts: &Parts) -> AppResult<TokenAuth> {
    let token = extract_bearer(parts).ok_or(AppError::Unauthorized)?;
    authenticate_token_str(state, &token).await
}

/// Authenticate a raw token string. Shared by the Authorization-header path and
/// the WebSocket `?token=` path so both get identical subscription gating.
pub async fn authenticate_token_str(state: &AppState, token: &str) -> AppResult<TokenAuth> {
    let token_hash = crate::auth::hash_token(token);

    let row: Option<(i64, i64, i64, String, i64, Option<String>, Option<String>, i64)> =
        sqlx::query_as(
            r#"SELECT users.id, users.is_admin, users.daily_quota_seconds,
                      users.subscription_status, users.monthly_minute_quota,
                      users.period_start, users.period_end, users.email_verified
               FROM tokens
               JOIN users ON users.id = tokens.user_id
               WHERE tokens.token_hash = ?1"#,
        )
        .bind(&token_hash)
        .fetch_optional(&state.db)
        .await?;

    let (
        user_id,
        is_admin,
        daily_quota_seconds,
        subscription_status,
        monthly_minute_quota,
        period_start,
        period_end,
        email_verified,
    ) = row.ok_or(AppError::Unauthorized)?;

    // Touch last_used_at (best-effort).
    let _ = sqlx::query("UPDATE tokens SET last_used_at = datetime('now') WHERE user_id = ?1")
        .bind(user_id)
        .execute(&state.db)
        .await;

    Ok(TokenAuth {
        user_id,
        is_admin: is_admin != 0,
        daily_quota_seconds,
        subscription_status,
        monthly_minute_quota,
        period_start,
        period_end,
        email_verified: email_verified != 0,
    })
}

fn extract_bearer(parts: &Parts) -> Option<String> {
    let auth = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let rest = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer "))?;
    Some(rest.trim().to_string())
}

/// For WebSocket: allow token via ?token= as well, since browsers can't send
/// custom headers on WS upgrade. Native clients should still prefer Authorization.
pub fn extract_token_ws(parts: &Parts) -> Option<String> {
    if let Some(t) = extract_bearer(parts) {
        return Some(t);
    }
    let q = parts.uri.query()?;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next()?;
        if k == "token" {
            return Some(urlencoding_decode(v));
        }
    }
    None
}

fn urlencoding_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[derive(Clone, Debug)]
pub struct TokenAuth {
    pub user_id: i64,
    pub is_admin: bool,
    pub daily_quota_seconds: i64,
    pub subscription_status: String,
    pub monthly_minute_quota: i64,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub email_verified: bool,
}

/// Gate a request before forwarding to Deepgram. Returns the seconds already
/// used in the relevant window (for logging). Errors deny the request.
///
/// - Admins are exempt from all limits.
/// - When Stripe billing is configured: require an active (or trialing)
///   subscription and enforce the monthly minute cap within the billing period
///   (falling back to the calendar month if the period is unknown).
/// - When billing is not configured (local/dev): keep the legacy daily cap.
pub async fn check_quota(state: &AppState, auth: &TokenAuth) -> AppResult<f64> {
    if auth.is_admin {
        return Ok(0.0);
    }

    if state.cfg.billing_enabled() {
        if auth.subscription_status == "active" || auth.subscription_status == "trialing" {
            let cap_seconds = (auth.monthly_minute_quota.max(0) as f64) * 60.0;
            let used: f64 = if let (Some(ps), Some(pe)) = (&auth.period_start, &auth.period_end) {
                sqlx::query_scalar(
                    "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log \
                     WHERE user_id = ?1 AND at >= ?2 AND at < ?3",
                )
                .bind(auth.user_id)
                .bind(ps)
                .bind(pe)
                .fetch_one(&state.db)
                .await?
            } else {
                sqlx::query_scalar(
                    "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log \
                     WHERE user_id = ?1 AND at >= strftime('%Y-%m-01 00:00:00','now')",
                )
                .bind(auth.user_id)
                .fetch_one(&state.db)
                .await?
            };
            if used >= cap_seconds {
                return Err(AppError::QuotaExceeded);
            }
            Ok(used)
        } else {
            // The free trial requires a verified email (only enforced when an
            // email provider is configured; otherwise this is a no-op).
            if state.cfg.email_verification_enabled() && !auth.email_verified {
                return Err(AppError::EmailNotVerified);
            }
            // Free trial: allow up to TRIAL_SECONDS of total (lifetime) usage,
            // after which the account must subscribe.
            let lifetime: f64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log WHERE user_id = ?1",
            )
            .bind(auth.user_id)
            .fetch_one(&state.db)
            .await?;
            if lifetime >= TRIAL_SECONDS {
                return Err(AppError::SubscriptionInactive);
            }
            Ok(lifetime)
        }
    } else {
        let used: f64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log WHERE user_id = ?1 AND at >= date('now')",
        )
        .bind(auth.user_id)
        .fetch_one(&state.db)
        .await?;
        if used >= auth.daily_quota_seconds as f64 {
            return Err(AppError::QuotaExceeded);
        }
        Ok(used)
    }
}

pub async fn log_usage(state: &AppState, user_id: i64, seconds: f64, kind: &str) {
    if let Err(e) = sqlx::query("INSERT INTO usage_log (user_id, seconds, kind) VALUES (?1, ?2, ?3)")
        .bind(user_id)
        .bind(seconds)
        .bind(kind)
        .execute(&state.db)
        .await
    {
        tracing::warn!("usage_log insert: {e}");
    }
}
