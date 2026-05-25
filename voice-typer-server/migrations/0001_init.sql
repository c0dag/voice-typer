CREATE TABLE users (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    email           TEXT    NOT NULL UNIQUE,
    password_hash   TEXT    NOT NULL,
    is_admin        INTEGER NOT NULL DEFAULT 0,
    daily_quota_seconds INTEGER NOT NULL DEFAULT 1800,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE invites (
    code            TEXT    PRIMARY KEY,
    created_for     TEXT,
    created_by      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    used_by         INTEGER REFERENCES users(id) ON DELETE SET NULL,
    used_at         TEXT,
    expires_at      TEXT,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE sessions (
    id              TEXT    PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at      TEXT    NOT NULL,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_sessions_user ON sessions(user_id);

CREATE TABLE tokens (
    user_id         INTEGER NOT NULL PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    token_hash      TEXT    NOT NULL UNIQUE,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    last_used_at    TEXT
);

CREATE TABLE usage_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    seconds         REAL    NOT NULL,
    kind            TEXT    NOT NULL,
    at              TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_usage_user_at ON usage_log(user_id, at);
