//! /admin: mint invites, list users, set quotas. Admin only.

use crate::auth::session::AdminUser;
use crate::auth::new_invite_code;
use crate::error::AppResult;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    Form,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AdminQuery {
    #[serde(default)]
    pub minted: Option<String>,
    #[serde(default)]
    pub err: Option<String>,
}

fn fmt_seconds(s: f64) -> String {
    if s < 60.0 {
        format!("{:.1}s", s)
    } else {
        let m = (s / 60.0).floor() as u32;
        let sec = (s % 60.0) as u32;
        format!("{m}m {sec}s")
    }
}

pub async fn render(
    State(state): State<AppState>,
    admin: Option<AdminUser>,
    Query(q): Query<AdminQuery>,
) -> AppResult<Response> {
    let Some(admin) = admin else {
        return Ok(Redirect::to("/login?next=/admin").into_response());
    };
    let minted_block = q
        .minted
        .as_deref()
        .map(|code| {
            format!(
                r#"<div class="success">Invite minted. Share this code with the new user.</div>
                   <div class="token-display">{}</div>
                   <p class="muted">They'll paste it into the signup form. Each invite is single-use.</p>"#,
                super::html_escape(code)
            )
        })
        .unwrap_or_default();

    let invites: Vec<(String, Option<String>, Option<i64>, Option<String>, Option<String>, String)> =
        sqlx::query_as(
            "SELECT code, created_for, used_by, used_at, expires_at, created_at
             FROM invites ORDER BY created_at DESC LIMIT 50",
        )
        .fetch_all(&state.db)
        .await?;

    let invite_rows: String = if invites.is_empty() {
        r#"<tr><td colspan="4" class="muted">No invites yet.</td></tr>"#.to_string()
    } else {
        invites
            .iter()
            .map(|(code, for_, used_by, used_at, exp, created)| {
                let status = if let Some(uid) = used_by {
                    format!(
                        "used by #{} at {}",
                        uid,
                        used_at.as_deref().unwrap_or("?")
                    )
                } else if let Some(e) = exp {
                    if e.as_str() < chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string().as_str() {
                        "expired".to_string()
                    } else {
                        format!("available, expires {}", e)
                    }
                } else {
                    "available".to_string()
                };
                format!(
                    r#"<tr><td class="mono">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>"#,
                    super::html_escape(code),
                    super::html_escape(for_.as_deref().unwrap_or("-")),
                    super::html_escape(&status),
                    super::html_escape(created),
                )
            })
            .collect()
    };

    let users: Vec<(i64, String, i64, String, Option<String>, i64, Option<String>, f64)> =
        sqlx::query_as(
            r#"SELECT users.id, users.email, users.is_admin,
                      users.subscription_status, users.plan, users.monthly_minute_quota,
                      users.stripe_customer_id,
                      COALESCE((SELECT SUM(seconds) FROM usage_log
                                WHERE user_id = users.id AND at >= date('now','start of month')), 0.0) AS used_month
               FROM users ORDER BY users.created_at DESC LIMIT 100"#,
        )
        .fetch_all(&state.db)
        .await?;

    let user_rows: String = users
        .iter()
        .map(|(id, email, is_admin, status, plan, minutes, stripe_customer, used_secs)| {
            let active = status == "active" || status == "trialing";
            let sub = if *is_admin != 0 {
                "admin (unlimited)".to_string()
            } else if active {
                let comp = if stripe_customer.is_none() { " (comp)" } else { "" };
                format!(
                    "{} · {} min{}",
                    plan.as_deref().unwrap_or("active"),
                    minutes,
                    comp
                )
            } else {
                super::html_escape(status)
            };
            let sel = |p: &str| if active && plan.as_deref() == Some(p) { " selected" } else { "" };
            let sel_none = if active { "" } else { " selected" };
            format!(
                r#"<tr>
                     <td class="mono">{id}</td>
                     <td>{email}</td>
                     <td>{admin}</td>
                     <td>{sub}</td>
                     <td>{used} min</td>
                     <td>
                       <form method="post" action="/admin/users/{id}/plan" style="display:flex;gap:6px;">
                         <select name="plan" style="padding:6px 8px;font-size:13px;border:1px solid var(--line);border-radius:8px;">
                           <option value="none"{sel_none}>Inactive</option>
                           <option value="starter"{sel_starter}>Starter (150)</option>
                           <option value="pro"{sel_pro}>Pro (350)</option>
                         </select>
                         <button class="secondary" type="submit" style="padding:6px 12px;font-size:13px;">Grant</button>
                       </form>
                     </td>
                   </tr>"#,
                id = id,
                email = super::html_escape(email),
                admin = if *is_admin != 0 { "yes" } else { "" },
                sub = sub,
                used = format!("{:.0}", *used_secs / 60.0),
                sel_none = sel_none,
                sel_starter = sel("starter"),
                sel_pro = sel("pro"),
            )
        })
        .collect();

    let err_block = q
        .err
        .as_deref()
        .map(|e| format!(r#"<div class="error">{}</div>"#, super::html_escape(e)))
        .unwrap_or_default();

    let body = format!(
        r#"<h1 class="page-title">Admin</h1>
           <p class="subtitle">Signed in as <strong>{email}</strong>.</p>

           <div class="card">
             <h2>Mint invite</h2>
             <form method="post" action="/admin/invites">
               <label for="created_for">Label (who's this for?)</label>
               <input id="created_for" name="created_for" type="text" placeholder="e.g. alice@example.com or 'team channel'">
               <label for="expires_in_days">Expires in N days (optional)</label>
               <input id="expires_in_days" name="expires_in_days" type="number" min="1" max="365">
               {err_block}
               <div class="row">
                 <button class="primary" type="submit">Mint invite</button>
               </div>
             </form>
             {minted_block}
           </div>

           <div class="card">
             <h2>Invites</h2>
             <div class="table-wrap"><table>
               <thead><tr><th>Code</th><th>For</th><th>Status</th><th>Created</th></tr></thead>
               <tbody>{invite_rows}</tbody>
             </table></div>
           </div>

           <div class="card">
             <h2>Users</h2>
             <p class="muted">Grant a plan to give someone access without paying (a comp), or set Inactive to revoke. Comped accounts are tagged "(comp)". Paying users manage their own subscription in the billing portal; granting here overrides the status directly. Usage is this calendar month, in minutes.</p>
             <div class="table-wrap"><table>
               <thead><tr><th>ID</th><th>Email</th><th>Admin</th><th>Subscription</th><th>This month</th><th>Grant plan</th></tr></thead>
               <tbody>{user_rows}</tbody>
             </table></div>
           </div>"#,
        email = super::html_escape(&admin.0.email),
    );

    Ok(super::page_wide("Admin · Voice Typer", Some(&admin.0.email), &body).into_response())
}

#[derive(Deserialize)]
pub struct MintInviteForm {
    #[serde(default)]
    pub created_for: Option<String>,
    #[serde(default)]
    pub expires_in_days: Option<String>,
}

pub async fn mint_invite_post(
    State(state): State<AppState>,
    admin: Option<AdminUser>,
    Form(req): Form<MintInviteForm>,
) -> AppResult<Response> {
    let Some(admin) = admin else {
        return Ok(Redirect::to("/login?next=/admin").into_response());
    };
    let code = new_invite_code();
    let expires_at = req.expires_in_days.as_deref().and_then(|s| s.trim().parse::<i64>().ok()).map(|d| {
        (chrono::Utc::now() + chrono::Duration::days(d))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
    });
    let for_label = req.created_for.as_deref().map(str::trim).filter(|s| !s.is_empty());

    sqlx::query(
        "INSERT INTO invites (code, created_for, created_by, expires_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&code)
    .bind(for_label)
    .bind(admin.0.id)
    .bind(&expires_at)
    .execute(&state.db)
    .await?;

    Ok(Redirect::to(&format!("/admin?minted={}", urlencode(&code))).into_response())
}

#[derive(Deserialize)]
pub struct SetQuotaForm {
    pub daily_quota_seconds: i64,
}

pub async fn set_quota_post(
    State(state): State<AppState>,
    admin: Option<AdminUser>,
    Path(id): Path<i64>,
    Form(req): Form<SetQuotaForm>,
) -> AppResult<Response> {
    let _admin = match admin {
        Some(a) => a,
        None => return Ok(Redirect::to("/login").into_response()),
    };
    sqlx::query("UPDATE users SET daily_quota_seconds = ?1 WHERE id = ?2")
        .bind(req.daily_quota_seconds)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Redirect::to("/admin").into_response())
}

#[derive(Deserialize)]
pub struct SetPlanForm {
    pub plan: String,
}

/// Grant or revoke a plan for a user without Stripe (a comp). "starter"/"pro"
/// activate with the matching monthly minutes; anything else sets it inactive.
pub async fn set_plan_post(
    State(state): State<AppState>,
    admin: Option<AdminUser>,
    Path(id): Path<i64>,
    Form(req): Form<SetPlanForm>,
) -> AppResult<Response> {
    if admin.is_none() {
        return Ok(Redirect::to("/login").into_response());
    }
    let (status, plan, minutes): (&str, Option<&str>, i64) = match req.plan.as_str() {
        "starter" => ("active", Some("starter"), 150),
        "pro" => ("active", Some("pro"), 350),
        _ => ("inactive", None, 0),
    };
    sqlx::query(
        "UPDATE users SET subscription_status = ?1, plan = ?2, monthly_minute_quota = ?3 WHERE id = ?4",
    )
    .bind(status)
    .bind(plan)
    .bind(minutes)
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(Redirect::to("/admin").into_response())
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
