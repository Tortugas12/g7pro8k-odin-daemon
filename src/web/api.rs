use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::{Config, FieldError};
use crate::daemon::{DaemonCommand, SharedDaemonState};
use crate::diagnostics::{format_capture_jsonl, DEFAULT_CAPTURE_SECONDS, MAX_CAPTURE_SECONDS};
use crate::protocol::state::ConnectionState;
use crate::transport::nusb_backend::probe_usb_devices;

/// Embedded 100% offline companion Web UI SPA.
pub const EMBEDDED_INDEX_HTML: &str = include_str!("../../web/index.html");

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldError>,
}

#[derive(Debug, Deserialize)]
pub struct RumbleTestRequest {
    #[serde(default = "default_large_motor")]
    pub large_motor: u8,
    #[serde(default = "default_small_motor")]
    pub small_motor: u8,
    #[serde(default = "default_rumble_duration_ms")]
    pub duration_ms: u64,
}

fn default_large_motor() -> u8 {
    200
}

fn default_small_motor() -> u8 {
    160
}

fn default_rumble_duration_ms() -> u64 {
    300
}

#[derive(Debug, Serialize)]
pub struct RumbleTestResponse {
    pub ok: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct PauseResumeResponse {
    pub paused: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct CaptureRequest {
    pub duration_secs: Option<u64>,
    /// Optional millisecond override (clamped 50..=30000) for fast automated tests.
    pub duration_ms: Option<u64>,
}

/// `GET /` — serves the embedded offline companion Web UI.
pub async fn serve_index() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Html(EMBEDDED_INDEX_HTML),
    )
}

/// `GET /api/status` — current daemon, USB, profile, Hz, and passive Odin 3 status.
pub async fn get_status(State(state): State<Arc<SharedDaemonState>>) -> impl IntoResponse {
    Json(state.current_status())
}

/// `GET /api/config` — returns the active configuration JSON.
pub async fn get_config(State(state): State<Arc<SharedDaemonState>>) -> impl IntoResponse {
    Json(state.current_config())
}

/// `PUT /api/config` — validates, atomically writes `config.json`, and hot-reloads the daemon.
pub async fn put_config(
    State(state): State<Arc<SharedDaemonState>>,
    Json(new_cfg): Json<Config>,
) -> Response {
    if let Err(err) = new_cfg.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                error: "validation_failed".to_string(),
                fields: err.errors,
            }),
        )
            .into_response();
    }

    if let Err(err) = new_cfg.save_atomic(&state.config_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody {
                error: format!("failed to persist config atomically: {err}"),
                fields: Vec::new(),
            }),
        )
            .into_response();
    }

    {
        *state.config.write().expect("config lock") = new_cfg.clone();
        state.status.write().expect("status lock").profile = new_cfg.profile;
        state.telemetry.write().expect("telemetry lock").profile = new_cfg.profile;
    }
    state.send_command(DaemonCommand::ReloadConfig(Box::new(new_cfg.clone())));

    (StatusCode::OK, Json(new_cfg)).into_response()
}

/// `POST /api/rumble-test` — triggers a safe bounded dual-motor vibration test (default 300ms).
pub async fn post_rumble_test(
    State(state): State<Arc<SharedDaemonState>>,
    body: Option<Json<RumbleTestRequest>>,
) -> Response {
    let status = state.current_status();
    let connected = matches!(
        status.connection_state,
        ConnectionState::ConnectedWired | ConnectionState::ConnectedDongle
    );
    if !connected {
        return (
            StatusCode::CONFLICT,
            Json(ApiErrorBody {
                error: "GameSir G7 Pro 8K is not connected".to_string(),
                fields: Vec::new(),
            }),
        )
            .into_response();
    }

    let req = body.map(|b| b.0).unwrap_or(RumbleTestRequest {
        large_motor: default_large_motor(),
        small_motor: default_small_motor(),
        duration_ms: default_rumble_duration_ms(),
    });
    let duration_ms = req.duration_ms.clamp(50, 1000);

    state.send_command(DaemonCommand::RumbleTest {
        large_motor: req.large_motor,
        small_motor: req.small_motor,
        duration_ms,
    });

    (
        StatusCode::OK,
        Json(RumbleTestResponse {
            ok: true,
            duration_ms,
        }),
    )
        .into_response()
}

/// `POST /api/daemon/pause` — pauses daemon, releasing USB & virtual devices cleanly.
pub async fn post_pause(State(state): State<Arc<SharedDaemonState>>) -> impl IntoResponse {
    {
        let mut st = state.status.write().expect("status lock");
        st.paused = true;
        st.connection_state = ConnectionState::Paused;
        st.usb_hz = 0;
        st.emit_hz = 0;
    }
    {
        let mut tel = state.telemetry.write().expect("telemetry lock");
        tel.connection_state = ConnectionState::Paused;
        tel.usb_hz = 0;
        tel.emit_hz = 0;
    }
    state.send_command(DaemonCommand::Pause);
    Json(PauseResumeResponse { paused: true })
}

/// `POST /api/daemon/resume` — resumes daemon from paused state.
pub async fn post_resume(State(state): State<Arc<SharedDaemonState>>) -> impl IntoResponse {
    {
        let mut st = state.status.write().expect("status lock");
        st.paused = false;
        if st.connection_state == ConnectionState::Paused {
            st.connection_state = ConnectionState::Disconnected;
        }
    }
    {
        let mut tel = state.telemetry.write().expect("telemetry lock");
        if tel.connection_state == ConnectionState::Paused {
            tel.connection_state = ConnectionState::Disconnected;
        }
    }
    state.send_command(DaemonCommand::Resume);
    Json(PauseResumeResponse { paused: false })
}

/// `GET /api/probe` — returns structured USB descriptor probe output.
pub async fn get_probe(State(state): State<Arc<SharedDaemonState>>) -> Response {
    let extra_pids = state.current_config().extra_pids;
    match tokio::task::spawn_blocking(move || probe_usb_devices(&extra_pids)).await {
        Ok(Ok(report)) => (StatusCode::OK, Json(report)).into_response(),
        Ok(Err(err)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody {
                error: err.to_string(),
                fields: Vec::new(),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorBody {
                error: format!("probe task failed: {err}"),
                fields: Vec::new(),
            }),
        )
            .into_response(),
    }
}

/// `POST /api/capture` — runs a bounded live packet capture (default 10s, max 30s)
/// via the non-disruptive capture tap and returns downloadable `.jsonl` content.
pub async fn post_capture(
    State(state): State<Arc<SharedDaemonState>>,
    body: Option<Json<CaptureRequest>>,
) -> Response {
    let req = body.map(|b| b.0).unwrap_or_default();
    let duration = if let Some(ms) = req.duration_ms {
        Duration::from_millis(ms.clamp(50, MAX_CAPTURE_SECONDS * 1000))
    } else {
        let secs = req
            .duration_secs
            .unwrap_or(DEFAULT_CAPTURE_SECONDS)
            .clamp(1, MAX_CAPTURE_SECONDS);
        Duration::from_secs(secs)
    };

    let (tx, mut rx) = mpsc::channel(4096);
    state.set_capture_tap(Some(tx));

    let mut records = Vec::new();
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(rec)) => records.push(rec),
            Ok(None) | Err(_) => break,
        }
    }

    state.set_capture_tap(None);

    let jsonl = match format_capture_jsonl(&records) {
        Ok(s) => s,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorBody {
                    error: err.to_string(),
                    fields: Vec::new(),
                }),
            )
                .into_response();
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"g7pro8k-capture.jsonl\""),
    );
    if let Ok(val) = HeaderValue::from_str(&records.len().to_string()) {
        headers.insert("x-capture-packets", val);
    }

    (StatusCode::OK, headers, jsonl).into_response()
}

/// `GET /ws/live` — 60Hz WebSocket telemetry stream for both GameSir and read-only Odin 3 SVGs.
pub async fn ws_live(
    ws: WebSocketUpgrade,
    State(state): State<Arc<SharedDaemonState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_live(socket, state))
}

async fn handle_ws_live(mut socket: WebSocket, state: Arc<SharedDaemonState>) {
    let mut ticker = tokio::time::interval(Duration::from_micros(16_667));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let snap = state.current_telemetry();
                let Ok(payload) = serde_json::to_string(&snap) else {
                    continue;
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(bytes))) => {
                        if socket.send(Message::Pong(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}
