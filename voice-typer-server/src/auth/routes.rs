use crate::auth::session::{self, AdminUser, AuthUser};
use crate::auth::{hash_token, new_invite_code, new_user_token, password};
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/signup", post(signup))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/me/token/rotate", post(rotate_token))
        .route("/admin/invites", get(list_invites).post(mint_invite))
        .route("/admin/users", get(list_users))
        .route("/admin/users/:id/quota", post(set_quota))
}

#[derive(Deserialize)]
pub struct SignupReq {
    pub email: String,
    pub password: String,
    pub invite_code: String,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub token: String,
}

async fn signup(
    State(state): State<AppState>,
    Json(req): Json<SignupReq>,
) -> AppResult<impl IntoResponse> {
    let email = req.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if req.password.len() < 12 {
        return Err(AppError::BadRequest("password must be at least 12 chars".into()));
    }

    let mut tx = state.db.begin().await?;

    // Validate + consume invite atomically.
    let invite = sqlx::query_as::<_, (String, Option<i64>, Option<String>)>(
        "SELECT code, used_by, expires_at FROM invites WHERE code = ?1",
    )
    .bind(&req.invite_code)
    .fetch_optional(&mut *tx)
    .await?;

    let invite = invite.ok_or(AppError::BadRequest("invalid invite code".into()))?;
    if invite.1.is_some() {
        return Err(AppError::BadRequest("invite already used".into()));
    }
    if let Some(exp) = invite.2 {
        if exp.as_str() < chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string().as_str() {
            return Err(AppError::BadRequest("invite expired".into()));
        }
    }

    // Email uniqueness check.
    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE email = ?1")
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?;
    if existing.is_some() {
        return Err(AppError::Conflict("email already registered".into()));
    }

    let pw_hash = password::hash(&req.password)
        .map_err(|e| AppError::Other(anyhow::anyhow!("password hash: {e}")))?;

    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash) VALUES (?1, ?2) RETURNING id",
    )
    .bind(&email)
    .bind(&pw_hash)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE invites SET used_by = ?1, used_at = datetime('now') WHERE code = ?2",
    )
    .bind(user_id)
    .bind(&req.invite_code)
    .execute(&mut *tx)
    .await?;

    // Mint API token.
    let token = new_user_token();
    sqlx::query("INSERT INTO tokens (user_id, token_hash) VALUES (?1, ?2)")
        .bind(user_id)
        .bind(hash_token(&token))
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    let sid = session::create(&state.db, user_id).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        session::cookie_header(&sid, session::SESSION_TTL_DAYS * 86400)
            .parse()
            .unwrap(),
    );

    Ok((StatusCode::CREATED, headers, Json(TokenResponse { token })))
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginReq>,
) -> AppResult<impl IntoResponse> {
    let email = req.email.trim().to_lowercase();
    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = ?1")
            .bind(&email)
            .fetch_optional(&state.db)
            .await?;

    let (user_id, pw_hash) = row.ok_or(AppError::Unauthorized)?;
    let ok = password::verify(&req.password, &pw_hash)
        .map_err(|e| AppError::Other(anyhow::anyhow!("verify: {e}")))?;
    if !ok {
        return Err(AppError::Unauthorized);
    }

    let sid = session::create(&state.db, user_id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        session::cookie_header(&sid, session::SESSION_TTL_DAYS * 86400)
            .parse()
            .unwrap(),
    );
    Ok((StatusCode::OK, headers, Json(json!({ "ok": true }))))
}

async fn logout(
    State(state): State<AppState>,
    headers_in: HeaderMap,
) -> AppResult<impl IntoResponse> {
    if let Some(cookie) = headers_in.get(header::COOKIE).and_then(|c| c.to_str().ok()) {
        for chunk in cookie.split(';') {
            let c = chunk.trim();
            if let Some(rest) = c.strip_prefix("vt_session=") {
                let _ = session::destroy(&state.db, rest).await;
            }
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        session::clear_cookie_header().parse().unwrap(),
    );
    Ok((StatusCode::OK, headers, Json(json!({ "ok": true }))))
}

#[derive(Serialize)]
struct MeResponse {
    id: i64,
    email: String,
    is_admin: bool,
    daily_quota_seconds: i64,
    used_today_seconds: f64,
    has_token: bool,
}

async fn me(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<MeResponse>> {
    let used: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log
         WHERE user_id = ?1 AND at >= date('now')",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    let has_token: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM tokens WHERE user_id = ?1")
        .bind(user.id)
        .fetch_optional(&state.db)
        .await?;

    Ok(Json(MeResponse {
        id: user.id,
        email: user.email,
        is_admin: user.is_admin,
        daily_quota_seconds: user.daily_quota_seconds,
        used_today_seconds: used,
        has_token: has_token.is_some(),
    }))
}

async fn rotate_token(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<TokenResponse>> {
    let token = new_user_token();
    sqlx::query(
        r#"INSERT INTO tokens (user_id, token_hash) VALUES (?1, ?2)
           ON CONFLICT(user_id) DO UPDATE SET token_hash = excluded.token_hash,
                                              created_at = datetime('now'),
                                              last_used_at = NULL"#,
    )
    .bind(user.id)
    .bind(hash_token(&token))
    .execute(&state.db)
    .await?;

    // Force any active streaming session to drop.
    state.session_lock.kick(user.id).await;

    Ok(Json(TokenResponse { token }))
}

#[derive(Deserialize)]
pub struct MintInviteReq {
    pub created_for: Option<String>,
    pub expires_in_days: Option<i64>,
}

#[derive(Serialize)]
struct MintInviteResponse {
    code: String,
}

async fn mint_invite(
    State(state): State<AppState>,
    admin: AdminUser,
    Json(req): Json<MintInviteReq>,
) -> AppResult<Json<MintInviteResponse>> {
    let code = new_invite_code();
    let expires_at = req.expires_in_days.map(|d| {
        (chrono::Utc::now() + chrono::Duration::days(d))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
    });
    sqlx::query(
        "INSERT INTO invites (code, created_for, created_by, expires_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&code)
    .bind(&req.created_for)
    .bind(admin.0.id)
    .bind(&expires_at)
    .execute(&state.db)
    .await?;
    Ok(Json(MintInviteResponse { code }))
}

#[derive(Serialize, sqlx::FromRow)]
struct InviteRow {
    code: String,
    created_for: Option<String>,
    used_by: Option<i64>,
    used_at: Option<String>,
    expires_at: Option<String>,
    created_at: String,
}

async fn list_invites(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> AppResult<Json<Vec<InviteRow>>> {
    let rows: Vec<InviteRow> = sqlx::query_as(
        "SELECT code, created_for, used_by, used_at, expires_at, created_at
         FROM invites ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow)]
struct UserRow {
    id: i64,
    email: String,
    is_admin: i64,
    daily_quota_seconds: i64,
    created_at: String,
    used_today_seconds: f64,
}

async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> AppResult<Json<Vec<UserRow>>> {
    let rows: Vec<UserRow> = sqlx::query_as(
        r#"SELECT users.id, users.email, users.is_admin, users.daily_quota_seconds, users.created_at,
                  COALESCE((SELECT SUM(seconds) FROM usage_log
                            WHERE user_id = users.id AND at >= date('now')), 0.0)
                  AS used_today_seconds
           FROM users
           ORDER BY users.created_at DESC"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct SetQuotaReq {
    pub daily_quota_seconds: i64,
}

async fn set_quota(
    State(state): State<AppState>,
    _admin: AdminUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(req): Json<SetQuotaReq>,
) -> AppResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE users SET daily_quota_seconds = ?1 WHERE id = ?2")
        .bind(req.daily_quota_seconds)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
