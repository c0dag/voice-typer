//! /signup, /login, /logout: server-rendered HTML pages with form posts.

use crate::auth::session::{self, AuthUser};
use crate::auth::{hash_token, new_user_token, password};
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form,
};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
pub struct SignupQuery {
    #[serde(default)]
    pub err: Option<String>,
}

pub async fn signup_get(
    user: Option<AuthUser>,
    Query(q): Query<SignupQuery>,
) -> impl IntoResponse {
    if user.is_some() {
        return Redirect::to("/dashboard").into_response();
    }

    let error = q.err.as_deref().map(|e| match e {
        "weak_password" => "Password must be at least 12 characters.",
        "email_taken" => "That email is already registered. Try signing in.",
        "bad_email" => "Please enter a valid email address.",
        "too_many" => "Too many sign-up attempts. Please wait a few minutes and try again.",
        _ => "Something went wrong. Try again.",
    });
    let error_div = error
        .map(|e| format!(r#"<div class="error">{}</div>"#, super::html_escape(e)))
        .unwrap_or_default();

    let body = format!(
        r#"<h1 class="page-title">Create your account</h1>
           <p class="subtitle">Sign up, then choose a plan to start.</p>
           <p class="subtitle" style="margin-top:-24px;font-size:14px;">From $2.50 / month for 150 minutes. Windows and macOS. See <a href="/#pricing">pricing</a> for details.</p>
           <div class="card">
             <form method="post" action="/signup">
               <label for="email">Email</label>
               <input id="email" name="email" type="email" autocomplete="email" required>
               <label for="password">Password (at least 12 characters)</label>
               <div class="pw-wrap">
                 <input id="password" name="password" type="password" autocomplete="new-password" minlength="12" required>
                 <button type="button" class="pw-toggle" aria-label="Show password" onclick="var i=document.getElementById('password');var s=i.type=='password';i.type=s?'text':'password';this.classList.toggle('on',s);this.setAttribute('aria-label',s?'Hide password':'Show password')">
                   <svg class="eye" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7-11-7-11-7z"/><circle cx="12" cy="12" r="3"/></svg>
                   <svg class="eye-off" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
                 </button>
               </div>
               {error_div}
               <div class="row">
                 <button class="primary" type="submit">Create account</button>
                 <a class="btn secondary" href="/login">I already have an account</a>
               </div>
             </form>
           </div>"#
    );

    super::page("Sign up · Voice Typer", None, &body).into_response()
}

#[derive(Deserialize)]
pub struct SignupForm {
    pub email: String,
    pub password: String,
}

pub async fn signup_post(
    State(state): State<AppState>,
    req_headers: HeaderMap,
    Form(req): Form<SignupForm>,
) -> AppResult<Response> {
    let ip = crate::rate_limit::client_ip(&req_headers);
    if !state
        .rate_limiter
        .check(&format!("signup:{ip}"), 5, Duration::from_secs(600))
    {
        return Ok(Redirect::to("/signup?err=too_many").into_response());
    }
    let email = req.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Ok(Redirect::to("/signup?err=bad_email").into_response());
    }
    if req.password.len() < 12 {
        return Ok(Redirect::to("/signup?err=weak_password").into_response());
    }

    let mut tx = state.db.begin().await?;

    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE email = ?1")
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?;
    if existing.is_some() {
        return Ok(Redirect::to("/signup?err=email_taken").into_response());
    }

    let pw_hash = password::hash(&req.password)
        .map_err(|e| AppError::Other(anyhow::anyhow!("password hash: {e}")))?;

    // Open signup: the account starts on a free trial (10 minutes). Mint the
    // token now so the trial is usable right away; it is shown once on the
    // dashboard. Subscribing later raises the cap to the plan's monthly minutes.
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash) VALUES (?1, ?2) RETURNING id",
    )
    .bind(&email)
    .bind(&pw_hash)
    .fetch_one(&mut *tx)
    .await?;

    let token = new_user_token();
    sqlx::query("INSERT INTO tokens (user_id, token_hash) VALUES (?1, ?2)")
        .bind(user_id)
        .bind(hash_token(&token))
        .execute(&mut *tx)
        .await?;

    // When email verification is on, stash a token to email after commit.
    let verify_token = if state.cfg.email_verification_enabled() {
        let vt = new_user_token();
        sqlx::query("UPDATE users SET email_verify_token = ?1 WHERE id = ?2")
            .bind(&vt)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        Some(vt)
    } else {
        None
    };

    tx.commit().await?;

    // Best effort: a failed send must not break signup (the user can resend).
    if let Some(vt) = verify_token {
        let url = format!("{}/verify?token={}", state.cfg.public_base_url, urlencode(&vt));
        if let Err(e) = crate::email::send_verification(&state.http, &state.cfg, &email, &url).await {
            tracing::warn!("verification email send failed for {email}: {e}");
        }
    }

    let sid = session::create(&state.db, user_id).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        session::cookie_header(&sid, session::SESSION_TTL_DAYS * 86400, state.cfg.cookie_secure)
            .parse()
            .unwrap(),
    );
    headers.insert(
        header::LOCATION,
        format!("/dashboard?token={}", urlencode(&token))
            .parse()
            .unwrap(),
    );
    Ok((StatusCode::SEE_OTHER, headers).into_response())
}

#[derive(Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    pub err: Option<String>,
    #[serde(default)]
    pub next: Option<String>,
}

pub async fn login_get(
    user: Option<AuthUser>,
    Query(q): Query<LoginQuery>,
) -> Response {
    if user.is_some() {
        return Redirect::to(safe_next(q.next.as_deref())).into_response();
    }

    let error = q.err.as_deref().map(|e| match e {
        "bad_credentials" => "Wrong email or password.",
        "too_many" => "Too many attempts. Please wait a few minutes and try again.",
        _ => "Something went wrong. Try again.",
    });

    let next_input = q
        .next
        .as_deref()
        .map(|n| format!(r#"<input type="hidden" name="next" value="{}">"#, super::html_escape(n)))
        .unwrap_or_default();

    let body = format!(
        r#"<h1 class="page-title">Sign in</h1>
           <p class="subtitle">Welcome back.</p>
           <div class="card">
             <form method="post" action="/login">
               <label for="email">Email</label>
               <input id="email" name="email" type="email" autocomplete="email" required>
               <label for="password">Password</label>
               <div class="pw-wrap">
                 <input id="password" name="password" type="password" autocomplete="current-password" required>
                 <button type="button" class="pw-toggle" aria-label="Show password" onclick="var i=document.getElementById('password');var s=i.type=='password';i.type=s?'text':'password';this.classList.toggle('on',s);this.setAttribute('aria-label',s?'Hide password':'Show password')">
                   <svg class="eye" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7-11-7-11-7z"/><circle cx="12" cy="12" r="3"/></svg>
                   <svg class="eye-off" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
                 </button>
               </div>
               {next_input}
               {error_div}
               <div class="row">
                 <button class="primary" type="submit">Sign in</button>
                 <a class="btn secondary" href="/signup">Create an account</a>
               </div>
             </form>
           </div>"#,
        error_div = error
            .map(|e| format!(r#"<div class="error">{}</div>"#, super::html_escape(e)))
            .unwrap_or_default()
    );
    super::page("Sign in · Voice Typer", None, &body).into_response()
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub next: Option<String>,
}

pub async fn login_post(
    State(state): State<AppState>,
    req_headers: HeaderMap,
    Form(req): Form<LoginForm>,
) -> AppResult<Response> {
    let email = req.email.trim().to_lowercase();
    let next = safe_next(req.next.as_deref());

    let ip = crate::rate_limit::client_ip(&req_headers);
    if !state
        .rate_limiter
        .check(&format!("login:{ip}"), 10, Duration::from_secs(600))
    {
        return Ok(Redirect::to(&format!("/login?err=too_many&next={}", urlencode(next))).into_response());
    }

    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = ?1")
            .bind(&email)
            .fetch_optional(&state.db)
            .await?;

    let Some((user_id, pw_hash)) = row else {
        // Equalize timing with the verify path so a missing account is not
        // distinguishable from a wrong password (defeats user enumeration).
        password::waste_time_verifying(&req.password);
        return Ok(Redirect::to(&format!("/login?err=bad_credentials&next={}", urlencode(next))).into_response());
    };
    let ok = password::verify(&req.password, &pw_hash)
        .map_err(|e| AppError::Other(anyhow::anyhow!("verify: {e}")))?;
    if !ok {
        return Ok(Redirect::to(&format!("/login?err=bad_credentials&next={}", urlencode(next))).into_response());
    }

    let sid = session::create(&state.db, user_id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        session::cookie_header(&sid, session::SESSION_TTL_DAYS * 86400, state.cfg.cookie_secure)
            .parse()
            .unwrap(),
    );
    headers.insert(header::LOCATION, next.parse().unwrap());
    Ok((StatusCode::SEE_OTHER, headers).into_response())
}

pub async fn logout_post(
    State(state): State<AppState>,
    headers_in: HeaderMap,
) -> Response {
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
        session::clear_cookie_header(state.cfg.cookie_secure).parse().unwrap(),
    );
    headers.insert(header::LOCATION, "/".parse().unwrap());
    (StatusCode::SEE_OTHER, headers).into_response()
}

#[derive(Deserialize)]
pub struct VerifyQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// GET /verify?token=... : mark the matching account verified. No auth needed;
/// possession of the emailed token is the proof.
pub async fn verify_get(
    State(state): State<AppState>,
    Query(q): Query<VerifyQuery>,
) -> AppResult<Response> {
    let Some(token) = q.token.filter(|t| !t.is_empty()) else {
        return Ok(Redirect::to("/dashboard?verified=invalid").into_response());
    };
    let res = sqlx::query(
        "UPDATE users SET email_verified = 1, email_verify_token = NULL WHERE email_verify_token = ?1",
    )
    .bind(&token)
    .execute(&state.db)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(Redirect::to("/dashboard?verified=invalid").into_response());
    }
    Ok(Redirect::to("/dashboard?verified=1").into_response())
}

/// POST /verify/resend : re-issue and resend the verification email.
pub async fn verify_resend_post(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Response> {
    if user.email_verified || !state.cfg.email_verification_enabled() {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    if !state
        .rate_limiter
        .check(&format!("verify_resend:{}", user.id), 3, Duration::from_secs(600))
    {
        return Ok(Redirect::to("/dashboard?verified=throttled").into_response());
    }
    let vt = new_user_token();
    sqlx::query("UPDATE users SET email_verify_token = ?1 WHERE id = ?2")
        .bind(&vt)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    let url = format!("{}/verify?token={}", state.cfg.public_base_url, urlencode(&vt));
    if let Err(e) = crate::email::send_verification(&state.http, &state.cfg, &user.email, &url).await {
        tracing::warn!("verification resend failed for {}: {e}", user.email);
        return Ok(Redirect::to("/dashboard?verified=error").into_response());
    }
    Ok(Redirect::to("/dashboard?verified=sent").into_response())
}

/// Only allow same-site relative redirect targets. Rejects protocol-relative
/// (`//host`), backslash variants (`/\host`), and control characters, so the
/// `next` param cannot be turned into an open redirect.
fn safe_next(next: Option<&str>) -> &str {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.starts_with("/\\")
                && !n.contains(|c| c == '\r' || c == '\n') =>
        {
            n
        }
        _ => "/dashboard",
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}
