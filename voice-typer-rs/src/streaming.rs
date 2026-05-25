//! Real-time streaming transcription via Deepgram's WebSocket API.
//!
//! Single-thread design:
//!
//!   `StreamSession::start()` returns IMMEDIATELY — it spawns ONE
//!   `stream-worker` thread that does connect + send + recv in a unified
//!   event loop with non-blocking socket I/O. There is no Mutex<WebSocket>:
//!   the worker owns the connection. Audio chunks arrive over an mpsc; the
//!   loop pulls them with `try_recv`, encodes to PCM16 LE, and ships them.
//!
//!   This replaces an earlier 2-thread (sender + receiver) design where a
//!   shared `Mutex<WebSocket>` caused the receiver thread to starve the
//!   sender for ~80ms at a time (because tungstenite's `read()` blocked on
//!   the socket's read_timeout while holding the lock). The starvation made
//!   the receiver's "idle timeout after shutdown" fire BEFORE the sender
//!   had drained the buffered audio — so `CloseStream` was sent after
//!   Deepgram had stopped listening, and the final transcript came back
//!   empty every time.
//!
//!   Lifecycle:
//!     1. Open WSS to `wss://api.deepgram.com/v1/listen?…`
//!     2. Loop forever:
//!        a. `try_recv` audio chunks (non-blocking), send each as a
//!           binary frame. When `try_recv` returns Disconnected (recorder
//!           dropped its sender side), `audio_done = true`.
//!        b. If `audio_done && !closed_sent`, send `{"type":"CloseStream"}`
//!           and flush.
//!        c. `ws.read()` for any incoming messages (non-blocking). Accumulate
//!           `is_final:true` transcripts; mirror live preview to overlay.
//!        d. `ws.flush()` to push any buffered writes.
//!        e. Exit when (closed_sent && idle 2s) OR socket closed OR 10min cap.
//!        f. Sleep 5ms.
//!     3. `finish()` joins the worker (returns once it exits the loop).
//!
//! Audio chunks are sent at the cpal device's NATIVE sample rate
//! (`encoding=linear16&sample_rate=<native>`). Deepgram does the resampling.

use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::overlay;

pub struct StreamSession {
    abort: Arc<AtomicBool>,
    final_transcript: Arc<Mutex<String>>,
    worker: Option<JoinHandle<()>>,
}

impl StreamSession {
    /// Returns IMMEDIATELY — connect happens on the spawned worker thread.
    pub fn start(
        token: String,
        sample_rate: u32,
        audio_rx: Receiver<Vec<f32>>,
    ) -> Result<Self> {
        if token.trim().is_empty() {
            return Err(anyhow!("Proxy token is empty"));
        }
        if sample_rate == 0 {
            return Err(anyhow!("invalid sample rate (recorder not started?)"));
        }

        let abort = Arc::new(AtomicBool::new(false));
        let final_transcript = Arc::new(Mutex::new(String::new()));

        let worker = {
            let abort = Arc::clone(&abort);
            let final_transcript = Arc::clone(&final_transcript);
            thread::Builder::new()
                .name("stream-worker".into())
                .spawn(move || {
                    if let Err(e) = run_session(
                        token,
                        sample_rate,
                        audio_rx,
                        abort,
                        final_transcript,
                    ) {
                        log::error!("streaming session error: {e}");
                        overlay::set_text(&format!("\u{26A0} {e}"));
                    }
                })
                .expect("spawn stream-worker")
        };

        Ok(Self {
            abort,
            final_transcript,
            worker: Some(worker),
        })
    }

    /// Wait for the session to finish naturally (audio drained + finals received),
    /// then return the accumulated transcript. Blocks the calling thread.
    pub fn finish(mut self) -> String {
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        let out = self.final_transcript.lock().unwrap().clone();
        log::info!("streaming: final transcript ({} chars)", out.len());
        out
    }

    /// Best-effort: force the worker to bail on the next loop tick.
    /// Not used by the normal lifecycle (the worker exits naturally when
    /// the recorder closes the audio mpsc). Kept for emergency shutdown.
    #[allow(dead_code)]
    pub fn abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
    }
}

fn run_session(
    token: String,
    sample_rate: u32,
    audio_rx: Receiver<Vec<f32>>,
    abort: Arc<AtomicBool>,
    final_transcript: Arc<Mutex<String>>,
) -> Result<()> {
    let ws_base = crate::config::PROXY_URL
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let model = crate::config::DEEPGRAM_MODEL;
    let language = crate::config::LANGUAGE;
    let url = format!(
        "{ws_base}/api/stream\
         ?model={model}\
         &language={language}\
         &encoding=linear16\
         &sample_rate={sample_rate}"
    );
    log::info!(
        "streaming: connecting proxy={} model={} lang={} sr={}",
        ws_base,
        model,
        language,
        sample_rate
    );
    let t0 = Instant::now();

    let mut req = url.into_client_request()?;
    req.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", token.trim()).parse()?,
    );

    let (mut ws, _resp) =
        tungstenite::connect(req).map_err(|e| anyhow!("Proxy WS connect failed: {e}"))?;
    log::info!("streaming: WS handshake OK ({}ms)", t0.elapsed().as_millis());

    // Make BOTH directions non-blocking so a slow read can never starve a
    // pending write. tungstenite's read/send/flush all bubble WouldBlock up
    // for us to ignore; we just loop with a 5ms sleep instead.
    if let Some(tcp) = tcp_from_ws_mut(&mut ws) {
        if let Err(e) = tcp.set_nonblocking(true) {
            log::warn!("streaming: set_nonblocking failed: {e}");
        }
    }

    // ---- Warm-up silence prefix --------------------------------------------
    // Nova-3 multilingual streaming has a documented weakness on short
    // utterances: the language detector decides on the first ~200-500ms and
    // can drift to the wrong language (e.g. Portuguese mis-detected as Dutch).
    // We send 200ms of zero-padded PCM16 BEFORE the real audio so Deepgram's
    // model has time to settle on the audio's characteristics before the
    // user's first phoneme arrives. It costs ~200ms of nothing — under the
    // `endpointing=300` threshold, so no spurious utterance boundary fires.
    {
        let silence_ms: u32 = 200;
        let silence_samples = (sample_rate * silence_ms / 1000) as usize;
        let silence_bytes = vec![0u8; silence_samples * 2]; // PCM16 LE zeros
        log::info!(
            "streaming: sending {}ms warm-up silence ({} bytes)",
            silence_ms,
            silence_bytes.len()
        );
        match ws.send(Message::Binary(silence_bytes)) {
            Ok(_) => {
                let _ = ws.flush();
            }
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                log::warn!("streaming: warm-up send hit WouldBlock — will retry on next iter");
            }
            Err(e) => {
                log::warn!("streaming: warm-up send err: {e}");
            }
        }
    }
    // ------------------------------------------------------------------------

    let mut audio_done = false;
    let mut closed_sent = false;
    let mut last_msg_at = Instant::now();
    let started = Instant::now();
    let mut chunks_sent: u64 = 0;
    let mut bytes_sent: u64 = 0;
    let mut log_chunk_throttle: u64 = 0;

    loop {
        // (1) Drain pending audio chunks (non-blocking).
        if !audio_done {
            loop {
                match audio_rx.try_recv() {
                    Ok(chunk) => {
                        let mut bin = Vec::with_capacity(chunk.len() * 2);
                        for &s in chunk.iter() {
                            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                            bin.extend_from_slice(&v.to_le_bytes());
                        }
                        let nbytes = bin.len();
                        match ws.send(Message::Binary(bin)) {
                            Ok(_) => {
                                chunks_sent += 1;
                                bytes_sent += nbytes as u64;
                                log_chunk_throttle += 1;
                                if log_chunk_throttle == 50 {
                                    log::debug!(
                                        "streaming: sent {} chunks / {} bytes so far",
                                        chunks_sent,
                                        bytes_sent
                                    );
                                    log_chunk_throttle = 0;
                                }
                            }
                            Err(tungstenite::Error::Io(ref e))
                                if e.kind() == std::io::ErrorKind::WouldBlock =>
                            {
                                // Write buffer full, retry next iter (we keep the chunk lost
                                // unfortunately — but at our throughput this is rare).
                                log::debug!("streaming: send WouldBlock, dropping a chunk");
                                break;
                            }
                            Err(e) => {
                                log::warn!("streaming: send error: {e}");
                                return Ok(());
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        audio_done = true;
                        log::info!(
                            "streaming: audio mpsc closed (sent {} chunks, {} bytes)",
                            chunks_sent,
                            bytes_sent
                        );
                        break;
                    }
                }
            }
        }

        // (2) Send CloseStream once the audio side is exhausted.
        if audio_done && !closed_sent {
            match ws.send(Message::Text(r#"{"type":"CloseStream"}"#.into())) {
                Ok(_) | Err(tungstenite::Error::Io(_)) => {}
                Err(e) => log::warn!("streaming: CloseStream send err: {e}"),
            }
            let _ = ws.flush();
            closed_sent = true;
            log::info!("streaming: CloseStream sent");
        }

        // (3) Drain any incoming messages.
        loop {
            match ws.read() {
                Ok(Message::Text(txt)) => {
                    last_msg_at = Instant::now();
                    handle_text(&txt, &final_transcript);
                }
                Ok(Message::Close(frame)) => {
                    log::info!("streaming: server Close ({:?})", frame);
                    return Ok(());
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(tungstenite::Error::ConnectionClosed)
                | Err(tungstenite::Error::AlreadyClosed) => {
                    log::info!("streaming: connection already closed");
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("streaming: read error: {e}");
                    return Ok(());
                }
            }
        }

        // (4) Try to flush queued writes; WouldBlock is fine.
        match ws.flush() {
            Ok(_) => {}
            Err(tungstenite::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => log::debug!("streaming: flush err: {e}"),
        }

        // (5) Exit conditions.
        if abort.load(Ordering::SeqCst) {
            log::info!("streaming: abort signalled");
            return Ok(());
        }
        if closed_sent && last_msg_at.elapsed() > Duration::from_millis(2500) {
            log::info!(
                "streaming: idle {}ms after CloseStream — exiting",
                last_msg_at.elapsed().as_millis()
            );
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(600) {
            log::warn!("streaming: 10min hard cap reached");
            return Ok(());
        }

        // (6) Yield. 5ms = up to ~200 iters/sec which is plenty for both
        //     a steady stream of audio chunks and incoming JSON messages.
        thread::sleep(Duration::from_millis(5));
    }
}

fn tcp_from_ws_mut(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
) -> Option<&mut TcpStream> {
    match ws.get_mut() {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(s.get_mut()),
        _ => None,
    }
}

fn handle_text(txt: &str, accum: &Arc<Mutex<String>>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(txt) else {
        log::debug!("streaming: non-JSON msg: {}", &txt[..txt.len().min(200)]);
        return;
    };

    let msg_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    if msg_type != "Results" {
        log::debug!("streaming: msg type={msg_type}");
        return;
    }

    let transcript = v
        .pointer("/channel/alternatives/0/transcript")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let is_final = v.get("is_final").and_then(|x| x.as_bool()).unwrap_or(false);

    if transcript.trim().is_empty() {
        return;
    }

    if is_final {
        let updated = {
            let mut g = accum.lock().unwrap();
            if !g.is_empty() && !g.ends_with(' ') {
                g.push(' ');
            }
            g.push_str(transcript.trim());
            g.clone()
        };
        overlay::set_text(&updated);
        log::info!("streaming: [final] {transcript}");
    } else {
        let preview = {
            let g = accum.lock().unwrap();
            if g.is_empty() {
                transcript.to_string()
            } else {
                format!("{g} {transcript}")
            }
        };
        overlay::set_text(&preview);
        log::debug!("streaming: [interim] {transcript}");
    }
}
