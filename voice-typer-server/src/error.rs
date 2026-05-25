use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("token in use elsewhere")]
    TokenInUseElsewhere,
    #[error("quota exceeded")]
    QuotaExceeded,
    #[error("too many requests")]
    RateLimited,
    #[error("subscription inactive")]
    SubscriptionInactive,
    #[error("email not verified")]
    EmailNotVerified,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::TokenInUseElsewhere => (StatusCode::FORBIDDEN, "token_in_use_elsewhere"),
            AppError::QuotaExceeded => (StatusCode::TOO_MANY_REQUESTS, "quota_exceeded"),
            AppError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            AppError::SubscriptionInactive => (StatusCode::PAYMENT_REQUIRED, "subscription_inactive"),
            AppError::EmailNotVerified => (StatusCode::FORBIDDEN, "email_not_verified"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::Upstream(_) => (StatusCode::BAD_GATEWAY, "upstream"),
            AppError::Sqlx(_) | AppError::Other(_) => {
                tracing::error!(error = ?self, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        };

        // Never leak internal error text (SQL/anyhow detail) to clients.
        let message = match &self {
            AppError::Sqlx(_) | AppError::Other(_) => "internal server error".to_string(),
            other => other.to_string(),
        };
        let body = Json(json!({
            "error": code,
            "message": message,
        }));
        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
