-- Stripe billing: per-user subscription state + monthly minute quota.
ALTER TABLE users ADD COLUMN stripe_customer_id     TEXT;
ALTER TABLE users ADD COLUMN stripe_subscription_id TEXT;
ALTER TABLE users ADD COLUMN subscription_status    TEXT    NOT NULL DEFAULT 'inactive';
ALTER TABLE users ADD COLUMN plan                   TEXT;
ALTER TABLE users ADD COLUMN monthly_minute_quota   INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN period_start           TEXT;
ALTER TABLE users ADD COLUMN period_end             TEXT;

CREATE INDEX IF NOT EXISTS idx_users_stripe_customer ON users(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_users_stripe_sub      ON users(stripe_subscription_id);
