use std::net::SocketAddr;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use g7pro8k_odin_daemon::config::{Config, ExtraButtonTarget, GamepadButton, Profile};
use g7pro8k_odin_daemon::daemon::{DaemonCommand, SharedDaemonState};
use g7pro8k_odin_daemon::protocol::state::ConnectionState;
use g7pro8k_odin_daemon::web::{build_router, serve_web_ui};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::tempdir;
use tower::ServiceExt;

async fn read_body_string(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn root_serves_offline_html_with_all_three_svgs() {
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let (shared, _rx) = SharedDaemonState::new(cfg_path, Config::default(), true);
    let app = build_router(shared);

    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = read_body_string(res.into_body()).await;

    assert!(html.contains("id=\"svg-xbox360\""));
    assert!(html.contains("id=\"svg-ds4\""));
    assert!(html.contains("id=\"svg-odin3\""));
    assert!(html.contains("id=\"plot-ls\""));
    assert!(html.contains("id=\"plot-rs\""));
    assert!(html.contains("rel=\"manifest\""));
    // 100% offline: no external CDN script/stylesheet links.
    assert!(!html.contains("https://cdn"));
    assert!(!html.contains("fonts.googleapis.com"));
}

#[tokio::test]
async fn get_and_put_config_validates_atomically_saves_and_hot_reloads() {
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let initial = Config::default();
    initial.save_atomic(&cfg_path).unwrap();

    let (shared, mut cmd_rx) = SharedDaemonState::new(cfg_path.clone(), initial, true);
    let app = build_router(shared);

    // GET /api/config
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let loaded: Config = serde_json::from_str(&read_body_string(res.into_body()).await).unwrap();
    assert_eq!(loaded.profile, Profile::Xbox360);

    // PUT /api/config (valid) -> saves atomically and enqueues ReloadConfig
    let mut updated = loaded.clone();
    updated.profile = Profile::Ds4;
    updated.emit_rate_hz = 500;
    updated.extra_buttons.l4 = ExtraButtonTarget::Gamepad {
        button: GamepadButton::L3,
    };

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&updated).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let saved_on_disk = Config::load_from_file(&cfg_path).unwrap();
    assert_eq!(saved_on_disk, updated);

    match cmd_rx.try_recv().expect("expected ReloadConfig command") {
        DaemonCommand::ReloadConfig(cfg) => assert_eq!(*cfg, updated),
        other => panic!("expected ReloadConfig, got {other:?}"),
    }

    // PUT /api/config (invalid) -> 400 with field-level errors and disk unchanged
    let mut invalid = updated.clone();
    invalid.emit_rate_hz = 9999;
    invalid.web_ui.bind = "0.0.0.0:8787".to_string();
    invalid.extra_buttons.r5 = ExtraButtonTarget::Key {
        code: "KEY_BOGUS".to_string(),
    };

    let res = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&invalid).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let err_json: Value = serde_json::from_str(&read_body_string(res.into_body()).await).unwrap();
    let fields: Vec<&str> = err_json["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field"].as_str().unwrap())
        .collect();
    assert!(fields.contains(&"emit_rate_hz"));
    assert!(fields.contains(&"web_ui.bind"));
    assert!(fields.contains(&"extra_buttons.r5.code"));
    assert_eq!(Config::load_from_file(&cfg_path).unwrap(), updated);
}

#[tokio::test]
async fn pause_resume_status_rumble_probe_and_capture_endpoints() {
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let (shared, mut cmd_rx) = SharedDaemonState::new(cfg_path, Config::default(), true);
    let app = build_router(shared.clone());

    // Pause
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/daemon/pause")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(matches!(cmd_rx.try_recv(), Ok(DaemonCommand::Pause)));

    // Status reflects paused + odin3 isolated
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let status_json: Value =
        serde_json::from_str(&read_body_string(res.into_body()).await).unwrap();
    assert_eq!(status_json["paused"], Value::Bool(true));
    assert_eq!(status_json["connection_state"], "paused");
    assert_eq!(status_json["odin3"]["isolated"], Value::Bool(true));

    // Resume
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/daemon/resume")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(matches!(cmd_rx.try_recv(), Ok(DaemonCommand::Resume)));

    // Rumble test while disconnected -> 409 Conflict
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/rumble-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Rumble test while connected -> 200 OK & enqueues RumbleTest
    shared.status.write().unwrap().connection_state = ConnectionState::ConnectedDongle;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/rumble-test")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"large_motor":180,"small_motor":120,"duration_ms":300}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    match cmd_rx.try_recv().expect("expected RumbleTest command") {
        DaemonCommand::RumbleTest {
            large_motor,
            small_motor,
            duration_ms,
        } => {
            assert_eq!(large_motor, 180);
            assert_eq!(small_motor, 120);
            assert_eq!(duration_ms, 300);
        }
        other => panic!("expected RumbleTest, got {other:?}"),
    }

    // Probe returns structured JSON
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let probe_json: Value = serde_json::from_str(&read_body_string(res.into_body()).await).unwrap();
    assert!(probe_json["devices"].is_array());

    // Bounded capture returns downloadable .jsonl
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capture")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"duration_ms":60}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("g7pro8k-capture.jsonl"));
}

#[tokio::test]
async fn serve_web_ui_refuses_non_loopback_address() {
    let dir = tempdir().unwrap();
    let (shared, _rx) = SharedDaemonState::new(dir.path().join("c.json"), Config::default(), true);
    let non_loopback: SocketAddr = "0.0.0.0:8787".parse().unwrap();
    let err = serve_web_ui(non_loopback, shared, async {})
        .await
        .expect_err("must reject non-loopback bind");
    assert!(err.contains("non-loopback"));
}
