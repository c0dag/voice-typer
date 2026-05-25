//! Server-rendered HTML pages: landing, signup, login, dashboard, admin, downloads.
//! The look-and-feel mirrors the Apple-style landing page (`landing/index.html`).

pub mod admin;
pub mod auth;
pub mod dashboard;
pub mod downloads;
pub mod landing;

use axum::response::Html;

/// Render a small auth/dashboard page that re-uses the landing's design tokens.
/// The shared CSS lives in this function so all pages compile to a single binary.
pub fn page(title: &str, current_user_email: Option<&str>, body: &str) -> Html<String> {
    page_inner(title, current_user_email, body, "")
}

/// Wider content column for data-dense back-office pages; the default narrow
/// column makes the /admin tables overflow their card.
pub fn page_wide(title: &str, current_user_email: Option<&str>, body: &str) -> Html<String> {
    page_inner(title, current_user_email, body, "wide")
}

fn page_inner(
    title: &str,
    current_user_email: Option<&str>,
    body: &str,
    main_class: &str,
) -> Html<String> {
    let nav_right = match current_user_email {
        Some(email) => format!(
            r#"<span class="nav-user">{}</span>
               <a class="nav-link" href="/dashboard">Dashboard</a>
               <form method="post" action="/logout" class="nav-form"><button type="submit" class="nav-link as-button">Sign out</button></form>"#,
            html_escape(email)
        ),
        None => r#"<a class="nav-link" href="/login">Sign in</a>
                   <a class="buy" href="/signup">Sign up</a>"#
            .to_string(),
    };

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="icon" type="image/png" href="/logo.png">
<style>
:root {{
  --bg: #ffffff;
  --bg-soft: #f5f5f7;
  --bg-tint: #fbfbfd;
  --line: #d2d2d7;
  --line-soft: #e5e5ea;
  --ink: #1d1d1f;
  --ink-2: #424245;
  --ink-3: #6e6e73;
  --ink-4: #86868b;
  --blue: #0066cc;
  --green: #22c55e;
  --green-soft: #e7f6ec;
  --rust: #ce422b;
  --rust-soft: #fdecea;
  --display: -apple-system, BlinkMacSystemFont, "SF Pro Display", "Inter", system-ui, "Segoe UI", "Helvetica Neue", sans-serif;
  --text:    -apple-system, BlinkMacSystemFont, "SF Pro Text", "Inter", system-ui, "Segoe UI", "Helvetica Neue", sans-serif;
  --mono:    ui-monospace, "SF Mono", Menlo, Consolas, monospace;
}}
*, *::before, *::after {{ box-sizing: border-box; }}
html, body {{ margin: 0; padding: 0; background: var(--bg); color: var(--ink); }}
body {{
  font: 17px/1.47 var(--text);
  letter-spacing: -0.022em;
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
}}
a {{ color: var(--blue); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}

nav {{
  position: sticky; top: 0; z-index: 10;
  background: rgba(255,255,255,0.78);
  backdrop-filter: saturate(180%) blur(20px);
  -webkit-backdrop-filter: saturate(180%) blur(20px);
  border-bottom: 1px solid rgba(0,0,0,0.06);
}}
.nav-inner {{
  max-width: 1280px; margin: 0 auto; padding: 0 22px;
  display: flex; align-items: center; justify-content: space-between;
  height: 44px; font-size: 13px; letter-spacing: -0.01em;
}}
.nav-brand {{ display: flex; align-items: center; gap: 8px; color: var(--ink); font-weight: 500; }}
.nav-brand img {{ width: 18px; height: 18px; }}
.nav-brand a {{ color: inherit; text-decoration: none; }}
.nav-actions {{ display: flex; gap: 22px; align-items: center; }}
.nav-link {{ color: var(--ink-2); text-decoration: none; transition: color 120ms; font: inherit; }}
.nav-link:hover {{ color: var(--ink); text-decoration: none; }}
.nav-link.as-button {{ background: none; border: 0; padding: 0; cursor: pointer; }}
.nav-form {{ margin: 0; padding: 0; display: inline; }}
.nav-user {{ color: var(--ink-3); font-size: 12px; }}
.buy {{
  color: white; background: var(--ink);
  padding: 5px 14px; border-radius: 999px;
  font-weight: 500; transition: background 120ms;
  text-decoration: none !important;
}}
.buy:hover {{ background: var(--blue); color: white; }}

main {{
  max-width: 580px;
  margin: 0 auto;
  padding: clamp(28px, 6vw, 64px) 22px clamp(40px, 8vw, 80px);
}}
main.wide {{ max-width: 980px; }}
h1.page-title {{
  font-family: var(--display);
  font-weight: 600;
  font-size: clamp(28px, 4vw, 40px);
  line-height: 1.1;
  letter-spacing: -0.025em;
  margin: 0 0 8px;
}}
.subtitle {{
  font-size: 17px; color: var(--ink-3); margin: 0 0 32px;
}}
.card {{
  background: var(--bg);
  border: 1px solid var(--line-soft);
  border-radius: 16px;
  padding: 28px 28px;
  margin-bottom: 16px;
}}
.card h2 {{
  font-family: var(--display);
  font-size: 20px; font-weight: 600;
  letter-spacing: -0.015em;
  margin: 0 0 6px;
}}
.card p {{ margin: 6px 0; color: var(--ink-2); }}
.card p.muted {{ color: var(--ink-3); font-size: 14px; }}

label {{
  display: block; margin: 16px 0 6px;
  font-size: 13px; color: var(--ink-3); letter-spacing: -0.01em;
}}
input[type=text], input[type=email], input[type=password], input[type=number] {{
  width: 100%;
  font: 15px/1.4 var(--text);
  color: var(--ink);
  background: var(--bg-tint);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 10px 14px;
  transition: border-color 120ms, background 120ms;
}}
input:focus {{ outline: none; border-color: var(--blue); background: var(--bg); }}

button, .btn {{
  font: 500 14px/1 var(--text);
  letter-spacing: -0.01em;
  padding: 12px 18px;
  border-radius: 999px;
  border: 0;
  cursor: pointer;
  display: inline-flex; align-items: center; gap: 8px;
  text-decoration: none;
}}
button.primary, .btn.primary {{
  background: var(--ink); color: white;
  transition: background 120ms;
}}
button.primary:hover, .btn.primary:hover {{ background: var(--blue); text-decoration: none; }}
button.secondary, .btn.secondary {{
  background: transparent; border: 1px solid var(--line); color: var(--ink);
}}
button.secondary:hover, .btn.secondary:hover {{ background: var(--bg-soft); text-decoration: none; }}
button.danger, .btn.danger {{
  background: var(--rust-soft); color: var(--rust);
}}
button:disabled, .btn.disabled {{
  background: var(--bg-soft); color: var(--ink-4); cursor: not-allowed;
  pointer-events: none;
}}

.token-display {{
  font-family: var(--mono);
  font-size: 13px;
  background: var(--bg-tint);
  border: 1px solid var(--line-soft);
  border-radius: 10px;
  padding: 14px 16px;
  word-break: break-all;
  user-select: all;
  color: var(--ink-2);
  margin: 12px 0;
}}
.error {{
  color: var(--rust); font-size: 14px; margin: 10px 0;
  background: var(--rust-soft); padding: 10px 14px; border-radius: 10px;
}}
.success {{
  color: var(--green); font-size: 14px; margin: 10px 0;
  background: var(--green-soft); padding: 10px 14px; border-radius: 10px;
}}
.row {{ display: flex; gap: 10px; align-items: center; margin: 16px 0 0; flex-wrap: wrap; }}
.spacer {{ height: 12px; }}

table {{ width: 100%; border-collapse: collapse; font-size: 14px; }}
th, td {{ text-align: left; padding: 10px 12px; border-bottom: 1px solid var(--line-soft); }}
th {{ color: var(--ink-3); font-weight: 500; font-size: 12px; letter-spacing: 0.02em; text-transform: uppercase; }}
td.mono {{ font-family: var(--mono); font-size: 12px; color: var(--ink-2); }}
.table-wrap {{ overflow-x: auto; -webkit-overflow-scrolling: touch; }}
.table-wrap table {{ min-width: 640px; }}

.download-grid {{
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
  margin-top: 16px;
}}
.download-tile {{
  border: 1px solid var(--line-soft);
  border-radius: 14px;
  padding: 20px 22px;
  text-decoration: none;
  color: inherit;
  display: flex; flex-direction: column; gap: 6px;
  transition: border-color 120ms, background 120ms;
  background: var(--bg);
}}
.download-tile:hover {{ border-color: var(--ink); text-decoration: none; }}
.download-tile.disabled {{ background: var(--bg-soft); color: var(--ink-4); cursor: not-allowed; }}
.download-tile.disabled:hover {{ border-color: var(--line-soft); }}
.download-tile svg {{ width: 28px; height: 28px; }}
.download-tile h3 {{ margin: 4px 0 0; font-size: 16px; font-weight: 600; letter-spacing: -0.01em; }}
.download-tile p {{ margin: 0; font-size: 13px; color: var(--ink-3); }}
@media (max-width: 600px) {{ .download-grid {{ grid-template-columns: 1fr; }} }}
.help-note {{ margin-top: 14px; }}
.help-note summary {{ cursor: pointer; color: var(--ink-2); font-size: 14px; }}
.help-note summary:hover {{ color: var(--ink); }}
.help-note p {{ margin: 6px 0 0; }}
.help-note video {{ width: 100%; border-radius: 12px; margin-top: 10px; display: block; background: var(--bg-soft); }}

/* Password show/hide toggle */
.pw-wrap {{ position: relative; }}
.pw-wrap input {{ padding-right: 42px; }}
.pw-toggle {{
  position: absolute; right: 8px; top: 50%; transform: translateY(-50%);
  background: none; border: 0; padding: 6px; cursor: pointer;
  color: var(--ink-4); display: inline-flex; align-items: center;
}}
.pw-toggle:hover {{ color: var(--ink-2); }}
.pw-toggle svg {{ width: 18px; height: 18px; display: block; }}
.pw-toggle .eye-off {{ display: none; }}
.pw-toggle.on .eye {{ display: none; }}
.pw-toggle.on .eye-off {{ display: block; }}

/* Usage progress bar */
.progress {{ height: 9px; background: var(--line-soft); border-radius: 999px; overflow: hidden; margin: 10px 0 6px; }}
.progress > span {{ display: block; height: 100%; background: var(--blue); border-radius: 999px; transition: width 250ms ease; }}
.progress.warn > span {{ background: var(--rust); }}
</style>
</head>
<body>
<nav>
  <div class="nav-inner">
    <span class="nav-brand">
      <a href="/" style="display:flex;align-items:center;gap:8px;">
        <img src="/logo.png" alt="">
        <span>Voice Typer</span>
      </a>
    </span>
    <span class="nav-actions">
      {nav_right}
    </span>
  </div>
</nav>
<main class="{main_class}">
{body}
</main>
</body>
</html>"#,
        title = html_escape(title),
        nav_right = nav_right,
        body = body,
        main_class = main_class,
    );
    Html(html)
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}
