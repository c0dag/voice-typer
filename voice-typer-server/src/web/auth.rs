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
    Form(req): Form<SignupForm>,
) -> AppResult<Response> {
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

    tx.commit().await?;

    let sid = session::create(&state.db, user_id).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        session::cookie_header(&sid, session::SESSION_TTL_DAYS * 86400)
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
        return Redirect::to(q.next.as_deref().unwrap_or("/dashboard")).into_response();
    }

    let error = q.err.as_deref().map(|e| match e {
        "bad_credentials" => "Wrong email or password.",
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
    Form(req): Form<LoginForm>,
) -> AppResult<Response> {
    let email = req.email.trim().to_lowercase();
    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = ?1")
            .bind(&email)
            .fetch_optional(&state.db)
            .await?;

    let next = req
        .next
        .as_deref()
        .filter(|n| n.starts_with('/'))
        .unwrap_or("/dashboard");

    let Some((user_id, pw_hash)) = row else {
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
        session::cookie_header(&sid, session::SESSION_TTL_DAYS * 86400)
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
        session::clear_cookie_header().parse().unwrap(),
    );
    headers.insert(header::LOCATION, "/".parse().unwrap());
    (StatusCode::SEE_OTHER, headers).into_response()
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
