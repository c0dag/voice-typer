use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use chrono::{Duration, Utc};
use rand::Rng;
use sqlx::SqlitePool;

pub const SESSION_COOKIE: &str = "vt_session";
pub const SESSION_TTL_DAYS: i64 = 30;

pub fn new_id() -> String {
    let bytes: [u8; 32] = rand::thread_rng().gen();
    hex::encode(bytes)
}

pub async fn create(pool: &SqlitePool, user_id: i64) -> AppResult<String> {
    let id = new_id();
    let expires_at = (Utc::now() + Duration::days(SESSION_TTL_DAYS))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?1, ?2, ?3)")
        .bind(&id)
        .bind(user_id)
        .bind(&expires_at)
        .execute(pool)
        .await?;
    Ok(id)
}

pub async fn destroy(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub fn cookie_header(value: &str, max_age_seconds: i64) -> String {
    format!(
        "{SESSION_COOKIE}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_seconds}"
    )
}

pub fn clear_cookie_header() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

fn extract_session_cookie(parts: &Parts) -> Option<String> {
    let header = parts.headers.get(header::COOKIE)?.to_str().ok()?;
    for chunk in header.split(';') {
        let chunk = chunk.trim();
        if let Some(rest) = chunk.strip_prefix(&format!("{SESSION_COOKIE}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub id: i64,
    pub email: String,
    pub is_admin: bool,
    pub daily_quota_seconds: i64,
    pub subscription_status: String,
    pub plan: Option<String>,
    pub monthly_minute_quota: i64,
    pub stripe_customer_id: Option<String>,
}

#[axum::async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let sid = extract_session_cookie(parts).ok_or(AppError::Unauthorized)?;
        let row = sqlx::query_as::<_, (i64, String, i64, i64, String, Option<String>, i64, Option<String>)>(
            r#"SELECT users.id, users.email, users.is_admin, users.daily_quota_seconds,
                      users.subscription_status, users.plan, users.monthly_minute_quota,
                      users.stripe_customer_id
               FROM sessions
               JOIN users ON users.id = sessions.user_id
               WHERE sessions.id = ?1 AND sessions.expires_at > datetime('now')"#,
        )
        .bind(&sid)
        .fetch_optional(&state.db)
        .await?;

        let (id, email, is_admin, daily_quota_seconds, subscription_status, plan, monthly_minute_quota, stripe_customer_id) =
            row.ok_or(AppError::Unauthorized)?;
        Ok(AuthUser {
            id,
            email,
            is_admin: is_admin != 0,
            daily_quota_seconds,
            subscription_status,
            plan,
            monthly_minute_quota,
            stripe_customer_id,
        })
    }
}

pub struct AdminUser(pub AuthUser);

#[axum::async_trait]
impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err(AppError::Forbidden);
        }
        Ok(AdminUser(user))
    }
}
