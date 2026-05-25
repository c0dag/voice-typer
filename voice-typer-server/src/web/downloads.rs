//! /download/*: auth-required binary downloads.

use crate::auth::session::AuthUser;
use crate::error::{AppError, AppResult};
use axum::{
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

const DOWNLOAD_DIR: &str = "./downloads";

pub async fn download(
    user: Option<AuthUser>,
    Path(platform): Path<String>,
) -> AppResult<Response> {
    let _user = match user {
        Some(u) => u,
        None => return Ok(axum::response::Redirect::to(&format!("/login?next=/download/{platform}")).into_response()),
    };
    let (filename, content_type) = match platform.as_str() {
        "windows" => ("VoiceTyper.exe", "application/vnd.microsoft.portable-executable"),
        "windows-installer" => ("VoiceTyper-Setup.exe", "application/vnd.microsoft.portable-executable"),
        "mac" | "macos" => ("VoiceTyper.dmg", "application/x-apple-diskimage"),
        "mac-zip" => ("VoiceTyper.app.zip", "application/zip"),
        _ => return Err(AppError::NotFound),
    };

    let path: PathBuf = PathBuf::from(DOWNLOAD_DIR).join(filename);
    let mut file = tokio::fs::File::open(&path).await.map_err(|e| {
        tracing::warn!("download open {}: {e}", path.display());
        AppError::NotFound
    })?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).await.map_err(|e| {
        AppError::Other(anyhow::anyhow!("read {}: {e}", path.display()))
    })?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!(r#"attachment; filename="{}""#, filename)
            .parse()
            .unwrap(),
    );
    headers.insert(header::CONTENT_LENGTH, buf.len().to_string().parse().unwrap());

    Ok((StatusCode::OK, headers, buf).into_response())
}
