use crate::error::{AppError, AppResult};
use crate::proxy::{authenticate_token, check_quota, extract_token_ws, log_usage, TokenAuth};
use crate::AppState;
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::request::Parts,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message as TgMessage};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/transcribe", post(transcribe))
        .route("/stream", get(stream))
}

#[derive(Deserialize)]
pub struct TranscribeQuery {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub encoding: Option<String>,
}

/// POST /api/transcribe?model=...&language=...&sample_rate=...&encoding=linear16
/// Body: raw PCM (or any container Deepgram accepts).
async fn transcribe(
    State(state): State<AppState>,
    parts_extr: AxumParts,
    Query(q): Query<TranscribeQuery>,
    body: Bytes,
) -> AppResult<impl IntoResponse> {
    let auth = authenticate_token(&state, &parts_extr.0).await?;
    let _used = check_quota(&state, &auth).await?;

    let mut url = "https://api.deepgram.com/v1/listen?".to_string();
    let mut first = true;
    let mut push = |k: &str, v: &str| {
        if !first {
            url.push('&');
        }
        url.push_str(k);
        url.push('=');
        url.push_str(&urlencoding(v));
        first = false;
    };
    if let Some(m) = q.model.as_deref() {
        push("model", m);
    }
    if let Some(l) = q.language.as_deref() {
        push("language", l);
    }
    if let Some(sr) = q.sample_rate {
        push("sample_rate", &sr.to_string());
    }
    if let Some(enc) = q.encoding.as_deref() {
        push("encoding", enc);
    } else {
        push("encoding", "linear16");
    }
    push("smart_format", "true");

    let resp = state
        .http
        .post(&url)
        .header("Authorization", format!("Token {}", state.cfg.deepgram_api_key))
        .header("Content-Type", "audio/raw")
        .body(body.to_vec())
        .send()
        .await
        .map_err(|e| AppError::Upstream(format!("deepgram: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!(
            "deepgram returned {status}: {text}"
        )));
    }

    // Estimate audio duration from body size assuming linear16 mono if sample_rate known.
    if let Some(sr) = q.sample_rate {
        let seconds = body.len() as f64 / (sr as f64 * 2.0);
        log_usage(&state, auth.user_id, seconds, "batch").await;
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Upstream(format!("deepgram body: {e}")))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        bytes,
    ))
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

#[derive(Deserialize)]
pub struct StreamQuery {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

/// GET /api/stream?token=...&model=...&language=...&sample_rate=...
/// Upgrades to WebSocket and proxies bidirectionally to Deepgram listen WS.
async fn stream(
    State(state): State<AppState>,
    parts_extr: AxumParts,
    Query(q): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> AppResult<axum::response::Response> {
    // Re-extract auth from the parts to ensure we honor Authorization header path too.
    let token = extract_token_ws(&parts_extr.0).ok_or(AppError::Unauthorized)?;
    let auth = crate::proxy::authenticate_token_str(&state, &token).await?;
    let _used = check_quota(&state, &auth).await?;

    let mut url = format!("wss://api.deepgram.com/v1/listen?encoding={}", q.encoding.as_deref().unwrap_or("linear16"));
    if let Some(sr) = q.sample_rate {
        url.push_str(&format!("&sample_rate={sr}"));
    }
    if let Some(m) = q.model.as_deref() {
        url.push_str(&format!("&model={}", urlencoding(m)));
    }
    if let Some(l) = q.language.as_deref() {
        url.push_str(&format!("&language={}", urlencoding(l)));
    }
    url.push_str("&interim_results=true&smart_format=true");

    let api_key = state.cfg.deepgram_api_key.clone();
    let session_lock = state.session_lock.clone();
    let app_state = state.clone();
    let sr_for_usage = q.sample_rate;

    Ok(ws.on_upgrade(move |socket| async move {
        let guard = session_lock.acquire(auth.user_id).await;
        if let Err(e) = proxy_ws(socket, &url, &api_key, guard, app_state, auth, sr_for_usage).await {
            tracing::warn!("ws proxy error: {e}");
        }
    }))
}

async fn proxy_ws(
    client_ws: WebSocket,
    deepgram_url: &str,
    api_key: &str,
    guard: crate::proxy::session_lock::Guard,
    state: AppState,
    auth: TokenAuth,
    sample_rate: Option<u32>,
) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::http::Request;

    let req = Request::builder()
        .uri(deepgram_url)
        .header("Authorization", format!("Token {}", api_key))
        .header("Host", "api.deepgram.com")
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
        .body(())?;

    let (dg_ws, _resp) = tokio::time::timeout(Duration::from_secs(10), connect_async(req))
        .await
        .map_err(|_| anyhow::anyhow!("deepgram connect timeout"))??;

    let (mut dg_tx, mut dg_rx) = dg_ws.split();
    let (mut cl_tx, mut cl_rx) = client_ws.split();

    let mut bytes_sent: u64 = 0;

    let kicked = async move {
        guard.kicked().await;
    };
    tokio::pin!(kicked);

    loop {
        tokio::select! {
            _ = &mut kicked => {
                let _ = cl_tx.send(WsMessage::Close(Some(axum::extract::ws::CloseFrame {
                    code: 4001,
                    reason: "token_in_use_elsewhere".into(),
                }))).await;
                let _ = dg_tx.send(TgMessage::Close(None)).await;
                break;
            }
            msg = cl_rx.next() => {
                match msg {
                    Some(Ok(WsMessage::Binary(b))) => {
                        bytes_sent += b.len() as u64;
                        if let Err(e) = dg_tx.send(TgMessage::Binary(b)).await {
                            tracing::debug!("dg send: {e}");
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Text(t))) => {
                        // Allow client to send control frames like {"type":"CloseStream"}
                        if let Err(e) = dg_tx.send(TgMessage::Text(t)).await {
                            tracing::debug!("dg send text: {e}");
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        let _ = dg_tx.send(TgMessage::Close(None)).await;
                        break;
                    }
                    Some(Ok(WsMessage::Ping(p))) => {
                        let _ = cl_tx.send(WsMessage::Pong(p)).await;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!("client recv: {e}");
                        break;
                    }
                }
            }
            msg = dg_rx.next() => {
                match msg {
                    Some(Ok(TgMessage::Text(t))) => {
                        if let Err(e) = cl_tx.send(WsMessage::Text(t)).await {
                            tracing::debug!("client send: {e}");
                            break;
                        }
                    }
                    Some(Ok(TgMessage::Binary(b))) => {
                        let _ = cl_tx.send(WsMessage::Binary(b)).await;
                    }
                    Some(Ok(TgMessage::Close(_))) | None => {
                        let _ = cl_tx.send(WsMessage::Close(None)).await;
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!("dg recv: {e}");
                        break;
                    }
                }
            }
        }
    }

    if let Some(sr) = sample_rate {
        let seconds = bytes_sent as f64 / (sr as f64 * 2.0);
        if seconds > 0.0 {
            log_usage(&state, auth.user_id, seconds, "stream").await;
        }
    }

    Ok(())
}

// Helper extractor to grab request parts before consuming the body.
pub struct AxumParts(pub Parts);

#[axum::async_trait]
impl<S> axum::extract::FromRequestParts<S> for AxumParts
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(AxumParts(parts.clone()))
    }
}
