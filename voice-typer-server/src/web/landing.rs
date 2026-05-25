//! GET / serves the Apple-style landing with auth-state-aware CTAs.

use crate::auth::session::AuthUser;
use crate::AppState;
use axum::{extract::State, response::Html};

const LANDING_HTML: &str = include_str!("../../landing/index.html");

/// Hero CTA shown to guests: a single primary "Get started" button + a subtle "Sign in" link.
/// No downloads on the public page. Downloads live only on the dashboard.
/// The `data-i18n` spans are translated client-side by the page's i18n script.
const GUEST_HERO_CTAS: &str = r##"
      <a class="btn-primary" href="/signup"><span data-i18n="cta.getStarted">Get started</span></a>
      <a class="btn-link" href="/login"><span data-i18n="cta.signin">Sign in</span></a>
"##;

/// Hero CTA shown to authenticated users: open the dashboard (where downloads live).
fn user_hero_ctas() -> &'static str {
    r##"
      <a class="btn-primary" href="/dashboard"><span data-i18n="cta.openDashboard">Open dashboard</span></a>
"##
}

const GUEST_NAV: &str = r##"<a href="/login" style="color: var(--ink-2); text-decoration: none;"><span data-i18n="cta.signin">Sign in</span></a>
<a class="buy" href="/signup"><span data-i18n="cta.signup">Sign up</span></a>"##;

fn user_nav(email: &str) -> String {
    format!(
        r##"<span style="color: var(--ink-3); font-size: 12px;">{}</span>
<a class="buy" href="/dashboard"><span data-i18n="cta.dashboard">Dashboard</span></a>"##,
        super::html_escape(email)
    )
}

pub async fn render(State(_state): State<AppState>, user: Option<AuthUser>) -> Html<String> {
    let (nav, ctas) = match user.as_ref() {
        Some(u) => (user_nav(&u.email), user_hero_ctas().to_string()),
        None => (GUEST_NAV.to_string(), GUEST_HERO_CTAS.to_string()),
    };
    let html = LANDING_HTML
        .replacen("<!--AUTH_NAV-->", &nav, 1)
        .replacen("<!--AUTH_HERO_CTAS-->", &ctas, 1);
    Html(html)
}
