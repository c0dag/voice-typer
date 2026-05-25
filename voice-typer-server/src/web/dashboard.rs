//! /dashboard: plan + token + downloads + usage. Requires auth.
//! The token, downloads, and usage are only shown once the account can actually
//! transcribe (an active subscription, or an admin account). Inactive accounts
//! see only the plan picker.

use crate::auth::session::AuthUser;
use crate::auth::{hash_token, new_user_token};
use crate::error::AppResult;
use crate::AppState;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct DashboardQuery {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub sub: Option<String>,
}

/// Format a duration given in seconds as minutes, matching how plans are
/// measured ("minutes per month"). Shows one decimal under 10 minutes.
fn fmt_minutes(seconds: f64) -> String {
    let mins = seconds / 60.0;
    if mins < 10.0 {
        let m = (mins * 10.0).round() / 10.0;
        if (m - 1.0).abs() < f64::EPSILON {
            "1 minute".to_string()
        } else {
            format!("{m} minutes")
        }
    } else {
        format!("{} minutes", mins.round() as i64)
    }
}

pub async fn render(
    State(state): State<AppState>,
    user: Option<AuthUser>,
    Query(q): Query<DashboardQuery>,
) -> AppResult<Response> {
    let Some(user) = user else {
        return Ok(Redirect::to("/login?next=/dashboard").into_response());
    };

    let active = user.subscription_status == "active" || user.subscription_status == "trialing";

    let secs_this_month: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log WHERE user_id = ?1 AND at >= date('now','start of month')",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    let secs_today: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log WHERE user_id = ?1 AND at >= date('now')",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    // Lifetime usage drives the free-trial allowance for accounts that have no
    // active subscription.
    let lifetime_secs: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(seconds), 0.0) FROM usage_log WHERE user_id = ?1",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    let trial_total = crate::proxy::TRIAL_SECONDS;
    let on_trial = !active && !user.is_admin && lifetime_secs < trial_total;
    // Token / downloads / usage are shown once the account can transcribe: an
    // active subscription, an admin, or a free trial that is not used up.
    let show_app = active || user.is_admin || on_trial;

    let flash = match q.sub.as_deref() {
        Some("success") => r#"<div class="success">Subscription active. You are all set.</div>"#,
        Some("cancel") => {
            r#"<div class="error">Checkout canceled. You can pick a plan whenever you are ready.</div>"#
        }
        _ => "",
    };

    let plan_card = if user.is_admin {
        r#"<div class="card">
             <h2>Your plan</h2>
             <p>Admin account. Unlimited access, no billing.</p>
           </div>"#
            .to_string()
    } else if active {
        let plan_name = match user.plan.as_deref() {
            Some("starter") => "Starter",
            Some("pro") => "Pro",
            _ => "Active",
        };
        // Paying users get the Stripe portal; comped users (no Stripe customer)
        // just see a note.
        let manage = if user.stripe_customer_id.is_some() {
            r#"<form method="post" action="/billing/portal">
                 <div class="row">
                   <button class="secondary" type="submit">Manage subscription</button>
                   <span class="muted">Update payment, change plan, or cancel. Canceling keeps access until the end of the paid period.</span>
                 </div>
               </form>"#
        } else {
            r#"<p class="muted">Access granted by your admin.</p>"#
        };
        format!(
            r#"<div class="card">
                 <h2>Your plan</h2>
                 <p><strong>{plan_name}</strong> &middot; {quota} minutes per month.</p>
                 {manage}
               </div>"#,
            plan_name = plan_name,
            quota = user.monthly_minute_quota,
            manage = manage,
        )
    } else {
        let picker = r#"<div class="download-grid">
               <div class="download-tile" style="cursor:default;">
                 <h3>Starter</h3>
                 <p>$2.50 / month &middot; 150 minutes</p>
                 <form method="post" action="/billing/checkout?plan=starter"><button class="primary" type="submit">Subscribe</button></form>
               </div>
               <div class="download-tile" style="cursor:default;">
                 <h3>Pro</h3>
                 <p>$5 / month &middot; 350 minutes</p>
                 <form method="post" action="/billing/checkout?plan=pro"><button class="primary" type="submit">Subscribe</button></form>
               </div>
             </div>"#;
        if on_trial {
            format!(
                r#"<div class="card">
                     <h2>Free trial</h2>
                     <p>You are on the free trial: <strong>{used} of 10 minutes</strong> used, {left} left.</p>
                     <p class="muted">Your token below works during the trial. Subscribe any time for monthly minutes.</p>
                     {picker}
                   </div>"#,
                used = fmt_minutes(lifetime_secs),
                left = fmt_minutes((trial_total - lifetime_secs).max(0.0)),
                picker = picker,
            )
        } else {
            format!(
                r#"<div class="card">
                     <h2>Free trial used up</h2>
                     <p class="muted">You have used your 10 free trial minutes. Pick a plan to keep transcribing.</p>
                     {picker}
                   </div>"#,
                picker = picker,
            )
        }
    };

    let token_card = if !show_app {
        String::new()
    } else if let Some(t) = q.token.as_deref() {
        format!(
            r#"<div class="card">
                 <h2>Your token</h2>
                 <p class="muted">Shown only once. Paste it into Voice Typer's Settings on your computer.</p>
                 <div class="token-display">{}</div>
                 <p class="muted">The previous token (if any) is now invalid. If you were signed in on another device with the old token, it will get kicked.</p>
               </div>"#,
            super::html_escape(t)
        )
    } else {
        r#"<div class="card">
             <h2>Your access token</h2>
             <p class="muted">Your token is only displayed at the moment it's minted. If you've lost it, rotate to mint a new one. The old one stops working immediately.</p>
             <form method="post" action="/dashboard/token/rotate" onsubmit="return confirm('Rotate token? Any device currently using the old token will stop working immediately.');">
               <div class="row">
                 <button class="primary" type="submit">Rotate token</button>
                 <span class="muted">One device per token.</span>
               </div>
             </form>
           </div>"#
            .to_string()
    };

    let downloads_card = if show_app {
        r##"<div class="card">
             <h2>Downloads</h2>
             <p class="muted">Install Voice Typer, then paste your token in Settings to link the app to your account. Your token stays on your computer.</p>
             <div class="download-grid">
               <a class="download-tile" href="/download/windows" download>
                 <svg viewBox="0 0 24 24" fill="#0078d4" aria-hidden="true"><path d="M3 5.5L11 4.3V11.5H3zM12 4.1L21 2.7V11.5H12zM3 12.5H11V19.7L3 18.5zM12 12.5H21V21.3L12 19.9z"/></svg>
                 <h3>Windows 10 / 11</h3>
                 <p>Fast, lightweight install</p>
               </a>
               <a class="download-tile" href="/download/mac" download>
                 <svg viewBox="0 0 24 24" fill="#1d1d1f" aria-hidden="true"><path d="M17.05 12.04c-.03-2.85 2.33-4.22 2.44-4.28-1.33-1.95-3.4-2.21-4.13-2.24-1.76-.18-3.43 1.04-4.32 1.04-.89 0-2.26-1.01-3.72-.99-1.91.03-3.68 1.11-4.66 2.82-1.99 3.45-.51 8.55 1.42 11.35.94 1.37 2.06 2.91 3.53 2.85 1.42-.06 1.95-.92 3.66-.92 1.71 0 2.19.92 3.69.89 1.52-.03 2.49-1.39 3.42-2.77 1.08-1.59 1.52-3.13 1.55-3.21-.03-.01-2.98-1.14-3.01-4.53zM14.6 4.13c.78-.95 1.31-2.27 1.16-3.58-1.12.05-2.48.75-3.29 1.69-.72.83-1.36 2.17-1.19 3.45 1.25.1 2.53-.63 3.32-1.56z"/></svg>
                 <h3>macOS 12+</h3>
                 <p>Universal &middot; DMG</p>
               </a>
             </div>
             <p class="muted">New to Voice Typer? See <a href="/#how">how it works</a> for the per-platform setup steps.</p>
           </div>"##
            .to_string()
    } else {
        String::new()
    };

    let usage_card = if show_app {
        // Pick the cap + used that the bar should reflect for this account.
        let (used_secs, cap_secs, scope) = if active {
            (secs_this_month, user.monthly_minute_quota as f64 * 60.0, "this month")
        } else if on_trial {
            (lifetime_secs, trial_total, "free trial")
        } else {
            (secs_this_month, 0.0, "this month") // admin: unlimited
        };
        let inner = if cap_secs > 0.0 {
            let pct = ((used_secs / cap_secs) * 100.0).clamp(0.0, 100.0);
            let warn = if pct >= 90.0 { " warn" } else { "" };
            format!(
                r#"<p><strong>{used}</strong> of {cap} used ({scope}).</p>
                   <div class="progress{warn}" role="progressbar"><span style="width:{pct:.1}%"></span></div>
                   <p class="muted">Today: {today}.</p>"#,
                used = fmt_minutes(used_secs),
                cap = fmt_minutes(cap_secs),
                scope = scope,
                warn = warn,
                pct = pct,
                today = fmt_minutes(secs_today),
            )
        } else {
            format!(
                r#"<p>This month: <strong>{used}</strong>. <span class="muted">Unlimited (admin).</span></p>
                   <p class="muted">Today: {today}.</p>"#,
                used = fmt_minutes(secs_this_month),
                today = fmt_minutes(secs_today),
            )
        };
        format!(
            r#"<div class="card">
                 <h2>Your usage</h2>
                 {inner}
               </div>"#,
            inner = inner,
        )
    } else {
        String::new()
    };

    let admin_link = if user.is_admin {
        r#"<a class="btn secondary" href="/admin">Admin panel</a>"#
    } else {
        ""
    };

    let body = format!(
        r##"<h1 class="page-title">Dashboard</h1>
           <p class="subtitle">Signed in as <strong>{email}</strong>.</p>

           {flash}
           {plan_card}
           {token_card}
           {downloads_card}
           {usage_card}

           <div class="row">
             {admin_link}
           </div>"##,
        email = super::html_escape(&user.email),
        flash = flash,
        plan_card = plan_card,
        token_card = token_card,
        downloads_card = downloads_card,
        usage_card = usage_card,
        admin_link = admin_link,
    );

    Ok(super::page("Dashboard · Voice Typer", Some(&user.email), &body).into_response())
}

pub async fn rotate_token_post(
    State(state): State<AppState>,
    user: Option<AuthUser>,
) -> AppResult<Response> {
    let Some(user) = user else {
        return Ok(Redirect::to("/login?next=/dashboard").into_response());
    };
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

    state.session_lock.kick(user.id).await;

    Ok(Redirect::to(&format!("/dashboard?token={}", urlencode(&token))).into_response())
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
