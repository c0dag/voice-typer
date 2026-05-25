-- Email verification. Enforced only when an email provider is configured
-- (see Config::email_verification_enabled); otherwise these columns are inert.
ALTER TABLE users ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN email_verify_token TEXT;

-- Grandfather every existing account as verified, so turning verification on
-- later never locks out anyone who signed up (or paid) before this change.
UPDATE users SET email_verified = 1;

CREATE INDEX IF NOT EXISTS idx_users_email_verify_token ON users(email_verify_token);
