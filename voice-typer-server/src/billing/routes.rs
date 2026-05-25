//! Billing HTTP routes: Checkout, Customer Portal, success/cancel, and the
//! Stripe webhook. Mounted at the site root (not under /api).

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use serde::Deserialize;

use super::SubInfo;
use crate::auth::session::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/billing/checkout", post(checkout))
        .route("/billing/portal", post(portal))
        .route("/billing/success", get(success))
        .route("/billing/cancel", get(cancel))
        .route("/stripe/webhook", post(webhook))
}

#[derive(Deserialize)]
pub struct CheckoutQuery {
    #[serde(default)]
    pub plan: Option<String>,
}

/// POST /billing/checkout?plan=starter|pro -> redirect to Stripe Checkout.
async fn checkout(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<CheckoutQuery>,
) -> AppResult<Response> {
    if !state.cfg.billing_enabled() {
        return Err(AppError::BadRequest("billing is not configured".into()));
    }
    let plan = q.plan.as_deref().unwrap_or("starter");
    let price = state
        .cfg
        .price_for_plan(plan)
        .ok_or_else(|| AppError::BadRequest("unknown plan".into()))?;
    let url = super::checkout_session_url(&state.http, &state.cfg, price, user.id, &user.email)
        .await
        .map_err(AppError::Other)?;
    Ok(Redirect::to(&url).into_response())
}

/// POST /billing/portal -> redirect to the Stripe Customer Portal.
async fn portal(State(state): State<AppState>, user: AuthUser) -> AppResult<Response> {
    let customer: Option<String> =
        sqlx::query_scalar("SELECT stripe_customer_id FROM users WHERE id = ?1")
            .bind(user.id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    let Some(customer) = customer else {
        // No Stripe customer yet: send them to pick a plan.
        return Ok(Redirect::to("/dashboard").into_response());
    };
    let url = super::portal_session_url(&state.http, &state.cfg, &customer)
        .await
        .map_err(AppError::Other)?;
    Ok(Redirect::to(&url).into_response())
}

#[derive(Deserialize)]
pub struct SuccessQuery {
    #[serde(default)]
    pub session_id: Option<String>,
}

/// GET /billing/success: best-effort immediate activation (so the dashboard
/// reflects the new subscription without waiting for the webhook), then to the
/// dashboard. The webhook remains the source of truth and is idempotent.
async fn success(
    State(state): State<AppState>,
    req_headers: HeaderMap,
    Query(q): Query<SuccessQuery>,
) -> Response {
    // Do not require the session cookie here: this is a cross-site return from
    // Stripe, and we identify the user from the Checkout Session's
    // client_reference_id instead. The webhook is still the source of truth.
    // Rate-limit per IP so it cannot be spammed into many Stripe API lookups.
    let ip = crate::rate_limit::client_ip(&req_headers);
    if !state
        .rate_limiter
        .check(&format!("success:{ip}"), 20, std::time::Duration::from_secs(60))
    {
        return Redirect::to("/dashboard").into_response();
    }
    let mut extra = String::new();
    if let Some(session_id) = q.session_id.as_deref() {
        match activate_from_session(&state, session_id).await {
            // First activation: mint the access token now and show it once.
            Ok(Some(user_id)) => {
                if let Some(token) = mint_token_if_absent(&state, user_id).await {
                    extra = format!("&token={token}");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("billing success activation (webhook will retry): {e:?}"),
        }
    }
    Redirect::to(&format!("/dashboard?sub=success{extra}")).into_response()
}

/// Mint the user's access token if they do not have one yet. Returns the
/// plaintext token only when newly created (the token is otherwise hash-only).
async fn mint_token_if_absent(state: &AppState, user_id: i64) -> Option<String> {
    let existing: Option<i64> = sqlx::query_scalar("SELECT user_id FROM tokens WHERE user_id = ?1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    if existing.is_some() {
        return None;
    }
    let token = crate::auth::new_user_token();
    match sqlx::query("INSERT INTO tokens (user_id, token_hash) VALUES (?1, ?2)")
        .bind(user_id)
        .bind(crate::auth::hash_token(&token))
        .execute(&state.db)
        .await
    {
        Ok(_) => Some(token),
        Err(e) => {
            tracing::warn!("mint token on activation: {e}");
            None
        }
    }
}

async fn cancel() -> Response {
    Redirect::to("/dashboard?sub=cancel").into_response()
}

/// Look up the Checkout Session, find the user via client_reference_id, then
/// fetch its subscription and apply the state.
async fn activate_from_session(state: &AppState, session_id: &str) -> anyhow::Result<Option<i64>> {
    let sk = state
        .cfg
        .stripe_secret_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("stripe not configured"))?;
    let v: serde_json::Value = state
        .http
        .get(format!(
            "https://api.stripe.com/v1/checkout/sessions/{session_id}"
        ))
        .bearer_auth(sk)
        .send()
        .await?
        .json()
        .await?;
    let customer = v["customer"].as_str();
    let sub_id = v["subscription"].as_str();
    let user_id = v["client_reference_id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok());
    if let (Some(user_id), Some(sub_id)) = (user_id, sub_id) {
        let info = super::fetch_subscription(&state.http, &state.cfg, sub_id).await?;
        apply(state, user_id, customer, Some(sub_id), &info).await?;
        return Ok(Some(user_id));
    }
    Ok(None)
}

/// POST /stripe/webhook: verify signature, then update subscription state.
async fn webhook(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(secret) = state.cfg.stripe_webhook_secret.as_deref() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let sig = headers
        .get("Stripe-Signature")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    if !super::verify_webhook_signature(&body, sig, secret) {
        return (StatusCode::BAD_REQUEST, "invalid signature").into_response();
    }
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad json").into_response(),
    };
    let typ = event["type"].as_str().unwrap_or_default();
    let obj = &event["data"]["object"];
    match handle_event(&state, typ, obj).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::error!("stripe webhook ({typ}): {e:?}");
            // 500 so Stripe retries.
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn handle_event(
    state: &AppState,
    typ: &str,
    obj: &serde_json::Value,
) -> anyhow::Result<()> {
    match typ {
        "checkout.session.completed" => {
            let user_id = obj["client_reference_id"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok());
            let customer = obj["customer"].as_str();
            let sub_id = obj["subscription"].as_str();
            if let (Some(user_id), Some(sub_id)) = (user_id, sub_id) {
                let info = super::fetch_subscription(&state.http, &state.cfg, sub_id).await?;
                apply(state, user_id, customer, Some(sub_id), &info).await?;
            }
        }
        "customer.subscription.created" | "customer.subscription.updated" => {
            let info = super::sub_info_from_value(obj);
            let sub_id = obj["id"].as_str();
            let customer = obj["customer"].as_str();
            if let (Some(uid), Some(sub_id)) = (resolve_user(state, obj).await, sub_id) {
                apply(state, uid, customer, Some(sub_id), &info).await?;
            }
        }
        "customer.subscription.deleted" => {
            let sub_id = obj["id"].as_str();
            let customer = obj["customer"].as_str();
            if let Some(uid) = resolve_user(state, obj).await {
                super::set_user_subscription(
                    &state.db, uid, customer, sub_id, "canceled", None, 0, None, None,
                )
                .await?;
            }
        }
        "invoice.paid" => {
            if let Some(sub_id) = obj["subscription"].as_str() {
                if let Some(uid) = super::user_id_by_subscription(&state.db, sub_id).await {
                    let info = super::fetch_subscription(&state.http, &state.cfg, sub_id).await?;
                    apply(state, uid, None, Some(sub_id), &info).await?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Resolve which local user an event's subscription object belongs to.
async fn resolve_user(state: &AppState, obj: &serde_json::Value) -> Option<i64> {
    if let Some(uid) = obj["metadata"]["user_id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
    {
        return Some(uid);
    }
    if let Some(sub_id) = obj["id"].as_str() {
        if let Some(uid) = super::user_id_by_subscription(&state.db, sub_id).await {
            return Some(uid);
        }
    }
    if let Some(cust) = obj["customer"].as_str() {
        if let Some(uid) = super::user_id_by_customer(&state.db, cust).await {
            return Some(uid);
        }
    }
    None
}

/// Map the subscription's price to plan + minutes and persist it.
async fn apply(
    state: &AppState,
    user_id: i64,
    customer: Option<&str>,
    sub_id: Option<&str>,
    info: &SubInfo,
) -> anyhow::Result<()> {
    let (plan, minutes) = info
        .price_id
        .as_deref()
        .and_then(|p| state.cfg.plan_for_price(p))
        .unwrap_or(("", 0));
    let plan_opt = if plan.is_empty() { None } else { Some(plan) };
    super::set_user_subscription(
        &state.db,
        user_id,
        customer,
        sub_id,
        &info.status,
        plan_opt,
        minutes,
        info.period_start,
        info.period_end,
    )
    .await
}
