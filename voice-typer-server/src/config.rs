use anyhow::{Context, Result};
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub deepgram_api_key: String,
    /// Deepgram model, pinned server-side so a client cannot request a pricier
    /// model than we intend to pay for.
    pub deepgram_model: String,
    pub database_url: String,
    pub bind_addr: String,
    /// Set the `Secure` attribute on the session cookie. Default true (prod is
    /// HTTPS). Set COOKIE_SECURE=false only for plain-http local dev on a
    /// non-localhost origin.
    pub cookie_secure: bool,
    pub admin_bootstrap_email: Option<String>,
    pub admin_bootstrap_password: Option<String>,

    // Stripe billing. When these are unset the server still runs, but
    // subscription gating is disabled (see `billing_enabled`). This keeps the
    // local/dev proxy usable without Stripe configured.
    pub stripe_secret_key: Option<String>,
    pub stripe_webhook_secret: Option<String>,
    pub stripe_price_starter: Option<String>,
    pub stripe_price_pro: Option<String>,
    /// Public origin used to build Stripe redirect URLs (no trailing slash).
    pub public_base_url: String,

    // Email (Resend). When the API key is unset, email verification is disabled
    // and signup behaves as before (no verification required).
    pub resend_api_key: Option<String>,
    /// From address for outgoing email. Must be on a domain verified in Resend
    /// (or Resend's test sender for development).
    pub email_from: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let nonempty = |k: &str| env::var(k).ok().filter(|s| !s.trim().is_empty());
        Ok(Self {
            deepgram_api_key: env::var("DEEPGRAM_API_KEY")
                .context("DEEPGRAM_API_KEY must be set")?,
            deepgram_model: env::var("DEEPGRAM_MODEL").unwrap_or_else(|_| "nova-3".into()),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://./data/voice-typer.db".into()),
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into()),
            cookie_secure: env::var("COOKIE_SECURE")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            admin_bootstrap_email: nonempty("ADMIN_BOOTSTRAP_EMAIL"),
            admin_bootstrap_password: nonempty("ADMIN_BOOTSTRAP_PASSWORD"),
            stripe_secret_key: nonempty("STRIPE_SECRET_KEY"),
            stripe_webhook_secret: nonempty("STRIPE_WEBHOOK_SECRET"),
            stripe_price_starter: nonempty("STRIPE_PRICE_STARTER"),
            stripe_price_pro: nonempty("STRIPE_PRICE_PRO"),
            public_base_url: env::var("PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8090".into())
                .trim_end_matches('/')
                .to_string(),
            resend_api_key: nonempty("RESEND_API_KEY"),
            email_from: env::var("EMAIL_FROM")
                .unwrap_or_else(|_| "Voice Typer <onboarding@resend.dev>".into()),
        })
    }

    /// Email verification is enforced only when an email provider is configured.
    pub fn email_verification_enabled(&self) -> bool {
        self.resend_api_key.is_some()
    }

    /// Subscription gating is active only when Stripe is fully configured.
    pub fn billing_enabled(&self) -> bool {
        self.stripe_secret_key.is_some()
            && self.stripe_webhook_secret.is_some()
            && self.stripe_price_starter.is_some()
            && self.stripe_price_pro.is_some()
    }

    /// Map a Stripe price id to its (plan name, monthly minute quota).
    pub fn plan_for_price(&self, price_id: &str) -> Option<(&'static str, i64)> {
        if Some(price_id) == self.stripe_price_starter.as_deref() {
            Some(("starter", 150))
        } else if Some(price_id) == self.stripe_price_pro.as_deref() {
            Some(("pro", 350))
        } else {
            None
        }
    }

    /// Map a plan name to its Stripe price id.
    pub fn price_for_plan(&self, plan: &str) -> Option<&str> {
        match plan {
            "starter" => self.stripe_price_starter.as_deref(),
            "pro" => self.stripe_price_pro.as_deref(),
            _ => None,
        }
    }
}
