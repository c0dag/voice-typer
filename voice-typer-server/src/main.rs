mod auth;
mod billing;
mod config;
mod db;
mod email;
mod error;
mod proxy;
mod rate_limit;
mod web;

use anyhow::Result;
use axum::{
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use config::Config;
use proxy::session_lock::SessionLock;
use sqlx::SqlitePool;
use std::sync::Arc;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
    pub session_lock: SessionLock,
    pub rate_limiter: Arc<rate_limit::RateLimiter>,
    pub concurrency: rate_limit::Concurrency,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg = Config::from_env()?;
    let db = db::connect(&cfg.database_url).await?;
    db::migrate(&db).await?;

    bootstrap_admin(&db, &cfg).await?;

    let state = AppState {
        db,
        cfg: Arc::new(cfg.clone()),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?,
        session_lock: SessionLock::new(),
        rate_limiter: Arc::new(rate_limit::RateLimiter::new()),
        concurrency: rate_limit::Concurrency::new(),
    };

    // Static landing assets: logo, favicon, svgs. Served from ./landing.
    let landing_assets = ServeDir::new("./landing").append_index_html_on_directories(false);

    let app = Router::new()
        .route("/", get(web::landing::render))
        .route("/signup", get(web::auth::signup_get).post(web::auth::signup_post))
        .route("/login", get(web::auth::login_get).post(web::auth::login_post))
        .route("/logout", post(web::auth::logout_post))
        .route("/dashboard", get(web::dashboard::render))
        .route("/dashboard/token/rotate", post(web::dashboard::rotate_token_post))
        .route("/verify", get(web::auth::verify_get))
        .route("/verify/resend", post(web::auth::verify_resend_post))
        .route("/admin", get(web::admin::render))
        .route("/admin/invites", post(web::admin::mint_invite_post))
        .route("/admin/users/:id/quota", post(web::admin::set_quota_post))
        .route("/admin/users/:id/plan", post(web::admin::set_plan_post))
        .route("/download/:platform", get(web::downloads::download))
        .route("/health", get(health))
        .merge(billing::routes::router())
        // JSON API kept for clients (the Voice Typer app uses /api/transcribe, /api/stream).
        .nest("/api", auth::routes::router())
        .nest("/api", proxy::routes::router())
        // Static files (logo.png, favicon.ico, *.svg) — must be last so it doesn't shadow routes.
        .fallback_service(landing_assets)
        .layer(
            // Log method + path only. Query strings can carry ?token= for the
            // WebSocket stream, and we must not write tokens to the logs.
            TraceLayer::new_for_http().make_span_with(|req: &axum::extract::Request| {
                tracing::info_span!("request", method = %req.method(), path = %req.uri().path())
            }),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    tracing::info!("listening on http://{}", cfg.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "ok": true }))
}

async fn bootstrap_admin(db: &SqlitePool, cfg: &Config) -> Result<()> {
    let (Some(email), Some(password)) = (
        cfg.admin_bootstrap_email.as_ref(),
        cfg.admin_bootstrap_password.as_ref(),
    ) else {
        return Ok(());
    };

    let email = email.trim().to_lowercase();
    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE email = ?1")
        .bind(&email)
        .fetch_optional(db)
        .await?;
    if existing.is_some() {
        tracing::info!("bootstrap admin: '{email}' already exists, skipping");
        return Ok(());
    }

    let pw_hash = auth::password::hash(password)?;
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, is_admin) VALUES (?1, ?2, 1) RETURNING id",
    )
    .bind(&email)
    .bind(&pw_hash)
    .fetch_one(db)
    .await?;

    let token = auth::new_user_token();
    sqlx::query("INSERT INTO tokens (user_id, token_hash) VALUES (?1, ?2)")
        .bind(user_id)
        .bind(auth::hash_token(&token))
        .execute(db)
        .await?;

    tracing::warn!(
        "bootstrap admin created: email={email} id={user_id}\nAPI token (save this NOW, shown once): {token}"
    );
    Ok(())
}
