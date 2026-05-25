use serde::Serialize;

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct User {
    pub id: i64,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub is_admin: i64,
    pub daily_quota_seconds: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Invite {
    pub code: String,
    pub created_for: Option<String>,
    pub created_by: i64,
    pub used_by: Option<i64>,
    pub used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    pub user_id: i64,
    pub token_hash: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}
