mod tracking_loop;

use std::net::{IpAddr, SocketAddr, UdpSocket as StdUdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream;
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, warn};
use uuid::Uuid;
use vc_core::{
    BitfocusButtonsConfig, CameraConfig, CameraId, ColorAdjustments, ConfigStore, DaemonWsEvent,
    FramingConfig, FramingPreset, NdiOutputConfig, PtzVelocity, RuleOfThirds, StreamDeckConfig,
    VmixConfig,
};
use vc_integrations::vmix::resolve_camera_tally;
use vc_integrations::{
    BitfocusButtonsClient, ButtonsTriggerRequest, StreamDeckAction, StreamDeckController,
    VmixClient, VmixSwitchCommand,
};
use vc_media::{CaptureDeviceInfo, MediaEngine, NdiDiscoveredSource};
use vc_ptz::{
    discover_onvif_devices, resolve_onvif_camera, FocusCommand, ImagingCommand,
    OnvifDiscoveredDevice, PtzManager,
};

use crate::tracking_loop::TrackingSupervisor;

#[derive(Clone)]
struct AppState {
    store: ConfigStore,
    media: MediaEngine,
    ptz: PtzManager,
    tracking: TrackingSupervisor,
    vmix: VmixClient,
    buttons: BitfocusButtonsClient,
    streamdeck: StreamDeckController,
    http_port: u16,
    osc_port: u16,
    lan_access_enabled: bool,
    web_ui_dir: Option<PathBuf>,
    shutdown_tx: watch::Sender<bool>,
}

fn resolve_web_ui_dir() -> Option<PathBuf> {
    if let Ok(env_dir) = std::env::var("VC_WEB_DIST_DIR") {
        let p = PathBuf::from(env_dir);
        if p.join("index.html").exists() {
            return Some(p);
        }
    }

    let mut candidates = vec![
        PathBuf::from("apps/desktop/dist"),
        PathBuf::from("../apps/desktop/dist"),
        PathBuf::from("../../apps/desktop/dist"),
    ];

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("web"));
            candidates.push(dir.join("dist"));
            // target/debug/vc-daemon.exe -> repo root/apps/desktop/dist
            if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                candidates.push(root.join("apps").join("desktop").join("dist"));
            }
        }
    }

    candidates.into_iter().find(|p| p.join("index.html").exists())
}

fn detect_lan_ipv4_addresses() -> Vec<String> {
    let mut ips = Vec::new();
    // Outbound UDP connect probe (sends no packets) reveals the primary active LAN interface IP
    for target in ["192.168.1.1:80", "10.0.0.1:80", "8.8.8.8:80"] {
        if let Ok(sock) = StdUdpSocket::bind("0.0.0.0:0") {
            if sock.connect(target).is_ok() {
                if let Ok(local) = sock.local_addr() {
                    if let IpAddr::V4(v4) = local.ip() {
                        if !v4.is_loopback() {
                            let s = v4.to_string();
                            if !ips.contains(&s) {
                                ips.push(s);
                            }
                        }
                    }
                }
            }
        }
    }
    ips
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vc_daemon=info,vc_media=info,vc_ptz=info,tower_http=info".into()),
        )
        .init();

    let config_path = std::env::var("VC_CONFIG_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| ConfigStore::default_path());

    let store = ConfigStore::load_or_create(&config_path)?;
    info!(db = %store.db_path().display(), "Loaded SQLite configuration database");
    let cfg_snapshot = store.snapshot().await;
    let media = MediaEngine::new();
    let ptz = PtzManager::new(media.clone());
    let tracking = TrackingSupervisor::new(store.clone(), media.clone(), ptz.clone());
    let vmix = VmixClient::new(cfg_snapshot.vmix.clone());
    let buttons = BitfocusButtonsClient::new();
    let streamdeck = StreamDeckController::new();

    for cam in &cfg_snapshot.cameras {
        if cam.enabled {
            ptz.configure_camera(cam).await;
            if let Err(err) = media.start_or_restart_camera(cam.clone()).await {
                warn!(camera = %cam.name, "Failed to start camera: {err}");
            }
            tracking.start_or_restart_camera(cam.id).await;
        }
    }

    // Spawn vMix TCP Tally + XML background sync & live tally-to-camera propagation
    let vmix_sync_handle = vmix.clone().spawn_background_sync();
    let tally_bridge_handle = {
        let mut tally_rx = vmix.subscribe();
        let tally_store = store.clone();
        let tally_media = media.clone();
        tokio::spawn(async move {
            while tally_rx.changed().await.is_ok() {
                let vmix_state = tally_rx.borrow().clone();
                let cams = tally_store.list_cameras().await;
                for cam in cams {
                    let tally = cam
                        .vmix
                        .input_ref
                        .as_deref()
                        .map(|r| resolve_camera_tally(r, &vmix_state))
                        .unwrap_or(vc_core::TallyState::Off);
                    if let Some(handle) = tally_media.get_handle(cam.id).await {
                        handle.set_tally(tally).await;
                    }
                }
            }
        })
    };

    let web_ui_dir = resolve_web_ui_dir();
    let lan_access_enabled = cfg_snapshot.server.lan_access_enabled;
    let http_port = cfg_snapshot.server.http_port;
    let osc_port = cfg_snapshot.server.osc_port;
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let state = AppState {
        store: store.clone(),
        media: media.clone(),
        ptz,
        tracking: tracking.clone(),
        vmix,
        buttons,
        streamdeck,
        http_port,
        osc_port,
        lan_access_enabled,
        web_ui_dir: web_ui_dir.clone(),
        shutdown_tx: shutdown_tx.clone(),
    };

    // Spawn lightweight OSC listener for Bitfocus Companion / Buttons & external controllers
    let osc_state = state.clone();
    let osc_handle = tokio::spawn(async move {
        run_osc_listener(osc_port, osc_state).await;
    });

    let app = build_router(state);

    let bind_host = if let Ok(env_host) = std::env::var("VC_BIND_ADDR") {
        env_host
    } else if lan_access_enabled {
        "0.0.0.0".to_string()
    } else {
        cfg_snapshot.server.bind_addr.clone()
    };

    let bind_addr: SocketAddr = format!("{bind_host}:{http_port}").parse()?;

    info!(addr = %bind_addr, osc_port, lan_urls = ?detect_lan_ipv4_addresses(), "VideoController daemon listening");
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    let ctrl_c_tx = shutdown_tx.clone();
    let graceful = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received Ctrl+C; initiating graceful shutdown");
                let _ = ctrl_c_tx.send(true);
            }
            _ = async {
                while !*shutdown_rx.borrow() {
                    if shutdown_rx.changed().await.is_err() {
                        break;
                    }
                }
            } => {
                info!("Received shutdown request; initiating graceful shutdown");
            }
        }
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await?;

    // Cleanly tear down background loops, camera/NDI/GStreamer workers, and flush SQLite WAL
    vmix_sync_handle.abort();
    tally_bridge_handle.abort();
    osc_handle.abort();
    tracking.stop_all_cameras().await;
    media.stop_all_cameras().await;
    store.checkpoint();
    info!("VideoController daemon stopped cleanly");
    Ok(())
}

fn build_router(state: AppState) -> Router {
    let web_ui_dir = state.web_ui_dir.clone();
    let mut app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/shutdown", post(request_shutdown))
        .route("/api/v1/capabilities", get(get_capabilities))
        .route("/api/v1/vision/status", get(get_vision_status))
        .route("/api/v1/remote-info", get(get_remote_info))
        .route("/api/v1/discovery/ndi", get(discover_ndi))
        .route("/api/v1/discovery/usb", get(discover_usb))
        .route("/api/v1/discovery/onvif", get(discover_onvif))
        .route("/api/v1/onvif/resolve", post(resolve_onvif))
        .route("/api/v1/cameras", get(list_cameras).post(create_camera))
        .route(
            "/api/v1/cameras/{id}",
            get(get_camera).put(update_camera).delete(delete_camera),
        )
        .route("/api/v1/cameras/{id}/ndi-output", put(update_ndi_output))
        .route("/api/v1/cameras/{id}/framing", put(update_framing))
        .route("/api/v1/cameras/{id}/color", put(update_color))
        .route("/api/v1/cameras/{id}/vmix", put(update_camera_vmix_binding))
        .route("/api/v1/cameras/{id}/imaging", post(camera_imaging))
        .route("/api/v1/cameras/{id}/tracking/select", post(select_subject))
        .route("/api/v1/cameras/{id}/ptz/move", post(ptz_move))
        .route("/api/v1/cameras/{id}/ptz/stop", post(ptz_stop))
        .route("/api/v1/cameras/{id}/ptz/focus", post(ptz_focus))
        .route("/api/v1/cameras/{id}/ptz/preset", post(ptz_preset))
        .route("/api/v1/cameras/{id}/ptz/calibrate", post(ptz_calibrate))
        .route("/api/v1/cameras/{id}/stream.mjpeg", get(stream_mjpeg))
        .route("/api/v1/cameras/{id}/snapshot.jpg", get(snapshot_jpeg))
        .route("/api/v1/integrations/vmix", get(get_vmix_state))
        .route("/api/v1/integrations/vmix/config", put(update_vmix_config))
        .route("/api/v1/integrations/vmix/command", post(send_vmix_command))
        .route("/api/v1/integrations/streamdeck", get(get_streamdeck_status))
        .route("/api/v1/integrations/streamdeck/config", put(update_streamdeck_config))
        .route("/api/v1/integrations/streamdeck/trigger", post(trigger_streamdeck_action))
        .route(
            "/api/v1/integrations/buttons/config",
            get(get_buttons_config).put(update_buttons_config),
        )
        .route("/api/v1/integrations/buttons/trigger", post(trigger_buttons_control))
        .route("/ws/telemetry", get(ws_telemetry))
        .layer(CorsLayer::permissive())
        .with_state(state);

    if let Some(ref dist_dir) = web_ui_dir {
        info!(dir = %dist_dir.display(), "Serving Remote Web / PWA UI from daemon");
        let index_file = dist_dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dist_dir).fallback(ServeFile::new(index_file)));
    } else {
        warn!("Built PWA UI directory (apps/desktop/dist) not found; run `pnpm build:ui` to enable LAN Remote PWA");
    }

    app
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn request_shutdown(State(state): State<AppState>) -> StatusCode {
    let _ = state.shutdown_tx.send(true);
    StatusCode::ACCEPTED
}

async fn get_capabilities(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.media.capabilities())
}

async fn get_vision_status() -> impl IntoResponse {
    let dir = vc_vision::onnx_engine::default_models_dir();
    Json(vc_vision::onnx_engine::probe_vision_status(&dir))
}

#[derive(Serialize)]
struct RemoteInfoResponse {
    lan_access_enabled: bool,
    http_port: u16,
    osc_port: u16,
    pwa_ready: bool,
    lan_urls: Vec<String>,
    local_url: String,
}

async fn get_remote_info(State(state): State<AppState>) -> impl IntoResponse {
    let lan_urls: Vec<String> = detect_lan_ipv4_addresses()
        .into_iter()
        .map(|ip| format!("http://{ip}:{}", state.http_port))
        .collect();

    Json(RemoteInfoResponse {
        lan_access_enabled: state.lan_access_enabled,
        http_port: state.http_port,
        osc_port: state.osc_port,
        pwa_ready: state.web_ui_dir.is_some(),
        lan_urls,
        local_url: format!("http://127.0.0.1:{}", state.http_port),
    })
}

#[derive(Serialize)]
struct NdiDiscoveryResponse {
    available: bool,
    sources: Vec<NdiDiscoveredSource>,
}

async fn discover_ndi(State(state): State<AppState>) -> impl IntoResponse {
    let caps = state.media.capabilities();
    let sources = state.media.discover_ndi_sources().await;
    Json(NdiDiscoveryResponse {
        available: caps.ndi_runtime_available,
        sources,
    })
}

#[derive(Serialize)]
struct UsbDiscoveryResponse {
    devices: Vec<CaptureDeviceInfo>,
}

async fn discover_usb(State(state): State<AppState>) -> impl IntoResponse {
    let devices = state.media.discover_usb_devices().await;
    Json(UsbDiscoveryResponse { devices })
}

#[derive(Serialize)]
struct OnvifDiscoveryResponse {
    devices: Vec<OnvifDiscoveredDevice>,
}

async fn discover_onvif() -> impl IntoResponse {
    let devices = discover_onvif_devices(1500).await;
    Json(OnvifDiscoveryResponse { devices })
}

#[derive(Deserialize)]
struct OnvifResolveRequest {
    host: String,
    port: u16,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

async fn resolve_onvif(Json(req): Json<OnvifResolveRequest>) -> impl IntoResponse {
    match resolve_onvif_camera(&req.host, req.port, &req.username, &req.password).await {
        Ok(info) => Json(info).into_response(),
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

async fn list_cameras(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.store.list_cameras().await)
}

async fn get_camera(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
) -> impl IntoResponse {
    match state.store.get_camera(id).await {
        Some(cam) => Json(cam).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn create_camera(
    State(state): State<AppState>,
    Json(mut cam): Json<CameraConfig>,
) -> impl IntoResponse {
    if cam.id.is_nil() {
        cam.id = Uuid::new_v4();
    }
    match state.store.upsert_camera(cam.clone()).await {
        Ok(saved) => {
            state.ptz.configure_camera(&saved).await;
            if saved.enabled {
                let _ = state.media.start_or_restart_camera(saved.clone()).await;
                state.tracking.start_or_restart_camera(saved.id).await;
            }
            (StatusCode::CREATED, Json(saved)).into_response()
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn update_camera(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(mut cam): Json<CameraConfig>,
) -> impl IntoResponse {
    cam.id = id;
    match state.store.upsert_camera(cam).await {
        Ok(saved) => {
            state.ptz.configure_camera(&saved).await;
            if saved.enabled {
                let _ = state.media.start_or_restart_camera(saved.clone()).await;
                state.tracking.start_or_restart_camera(saved.id).await;
            } else {
                state.tracking.stop_camera(saved.id).await;
                state.media.stop_camera(saved.id).await;
            }
            Json(saved).into_response()
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn delete_camera(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
) -> impl IntoResponse {
    state.tracking.stop_camera(id).await;
    state.media.stop_camera(id).await;
    state.ptz.remove_camera(id).await;
    match state.store.delete_camera(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn update_ndi_output(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(ndi_output): Json<NdiOutputConfig>,
) -> impl IntoResponse {
    match state.store.update_ndi_output(id, ndi_output.clone()).await {
        Ok(Some(cam)) => {
            if let Some(handle) = state.media.get_handle(id).await {
                handle.update_ndi_output(ndi_output).await;
            }
            Json(cam).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn update_framing(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(framing): Json<FramingConfig>,
) -> impl IntoResponse {
    match state.store.update_framing(id, framing.clone()).await {
        Ok(Some(cam)) => {
            if let Some(handle) = state.media.get_handle(id).await {
                handle.update_framing(framing).await;
            }
            Json(cam).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn update_color(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(color): Json<ColorAdjustments>,
) -> impl IntoResponse {
    match state.store.update_color(id, color.clone()).await {
        Ok(Some(cam)) => {
            if let Some(handle) = state.media.get_handle(id).await {
                handle.update_color(color).await;
            }
            Json(cam).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct SelectSubjectRequest {
    track_id: Option<u64>,
    norm_x: Option<f32>,
    norm_y: Option<f32>,
    clear: Option<bool>,
}

#[derive(Serialize)]
struct SelectSubjectResponse {
    locked_track_id: Option<u64>,
}

async fn select_subject(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(req): Json<SelectSubjectRequest>,
) -> impl IntoResponse {
    let Some(handle) = state.media.get_handle(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let locked_track_id = if req.clear.unwrap_or(false) {
        handle.set_locked_track(None).await;
        None
    } else if let Some(tid) = req.track_id {
        handle.set_locked_track(Some(tid)).await;
        Some(tid)
    } else if let (Some(x), Some(y)) = (req.norm_x, req.norm_y) {
        handle.select_subject_at(x, y).await
    } else {
        None
    };

    Json(SelectSubjectResponse { locked_track_id }).into_response()
}

#[derive(Deserialize)]
struct PtzMoveRequest {
    #[serde(default)]
    pan: f32,
    #[serde(default)]
    tilt: f32,
    #[serde(default)]
    zoom: f32,
}

async fn ptz_move(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(req): Json<PtzMoveRequest>,
) -> impl IntoResponse {
    state.tracking.notify_manual_override(id, 2.5).await;
    if let Some(handle) = state.media.get_handle(id).await {
        handle
            .set_ptz_velocity(PtzVelocity {
                pan: req.pan,
                tilt: req.tilt,
                zoom: req.zoom,
            })
            .await;
    }
    if let Some(driver) = state.ptz.get(id).await {
        let _ = driver.pan_tilt_speed(req.pan, req.tilt).await;
        let _ = driver.zoom_speed(req.zoom).await;
    }
    StatusCode::NO_CONTENT
}

async fn ptz_stop(State(state): State<AppState>, Path(id): Path<CameraId>) -> impl IntoResponse {
    state.tracking.notify_manual_override(id, 1.5).await;
    if let Some(handle) = state.media.get_handle(id).await {
        handle.set_ptz_velocity(PtzVelocity::default()).await;
    }
    if let Some(driver) = state.ptz.get(id).await {
        let _ = driver.stop().await;
    }
    StatusCode::NO_CONTENT
}

async fn ptz_focus(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(cmd): Json<FocusCommand>,
) -> impl IntoResponse {
    if let Some(driver) = state.ptz.get(id).await {
        if let Err(err) = driver.focus(cmd).await {
            return (StatusCode::BAD_GATEWAY, err.to_string()).into_response();
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn camera_imaging(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(cmd): Json<ImagingCommand>,
) -> impl IntoResponse {
    if let Some(driver) = state.ptz.get(id).await {
        if let Err(err) = driver.set_imaging(cmd).await {
            return (StatusCode::BAD_GATEWAY, err.to_string()).into_response();
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct PtzPresetRequest {
    action: String,
    slot: u8,
}

async fn ptz_preset(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(req): Json<PtzPresetRequest>,
) -> impl IntoResponse {
    if req.action != "store" {
        state.tracking.notify_manual_override(id, 3.0).await;
    }
    if let Some(driver) = state.ptz.get(id).await {
        let res = match req.action.as_str() {
            "store" => driver.store_preset(req.slot).await,
            _ => driver.recall_preset(req.slot).await,
        };
        if let Err(err) = res {
            return (StatusCode::BAD_GATEWAY, err.to_string()).into_response();
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn ptz_calibrate(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
) -> impl IntoResponse {
    match state.tracking.calibrate_camera(id).await {
        Ok(report) => Json(report).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct CameraVmixBindingReq {
    input_ref: Option<String>,
}

async fn update_camera_vmix_binding(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Json(req): Json<CameraVmixBindingReq>,
) -> impl IntoResponse {
    match state.store.update_vmix_binding(id, req.input_ref.clone()).await {
        Ok(Some(cam)) => {
            let vmix_state = state.vmix.current_state();
            let tally = cam
                .vmix
                .input_ref
                .as_deref()
                .map(|r| resolve_camera_tally(r, &vmix_state))
                .unwrap_or(vc_core::TallyState::Off);
            if let Some(handle) = state.media.get_handle(id).await {
                handle.set_tally(tally).await;
            }
            Json(cam).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn get_vmix_state(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.vmix.current_state())
}

async fn update_vmix_config(
    State(state): State<AppState>,
    Json(cfg): Json<VmixConfig>,
) -> impl IntoResponse {
    match state.store.update_vmix_config(cfg.clone()).await {
        Ok(saved) => {
            state.vmix.update_config(saved.clone()).await;
            if saved.enabled {
                let _ = state.vmix.refresh_xml_state().await;
            }
            Json(state.vmix.current_state()).into_response()
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn send_vmix_command(
    State(state): State<AppState>,
    Json(cmd): Json<VmixSwitchCommand>,
) -> impl IntoResponse {
    match state.vmix.execute(&cmd).await {
        Ok(()) => Json(state.vmix.current_state()).into_response(),
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

async fn get_streamdeck_status(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.store.snapshot().await.stream_deck;
    Json(state.streamdeck.status(&cfg))
}

async fn update_streamdeck_config(
    State(state): State<AppState>,
    Json(cfg): Json<StreamDeckConfig>,
) -> impl IntoResponse {
    match state.store.update_stream_deck_config(cfg).await {
        Ok(saved) => Json(state.streamdeck.status(&saved)).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn trigger_streamdeck_action(
    State(state): State<AppState>,
    Json(action): Json<StreamDeckAction>,
) -> impl IntoResponse {
    match execute_surface_action(&state, action).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

async fn get_buttons_config(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.store.snapshot().await.bitfocus_buttons)
}

async fn update_buttons_config(
    State(state): State<AppState>,
    Json(cfg): Json<BitfocusButtonsConfig>,
) -> impl IntoResponse {
    match state.store.update_bitfocus_buttons_config(cfg).await {
        Ok(saved) => Json(saved).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn trigger_buttons_control(
    State(state): State<AppState>,
    Json(req): Json<ButtonsTriggerRequest>,
) -> impl IntoResponse {
    let cfg = state.store.snapshot().await.bitfocus_buttons;
    match state.buttons.post_control(&cfg, &req).await {
        Ok(val) => Json(val).into_response(),
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

async fn execute_surface_action(state: &AppState, action: StreamDeckAction) -> Result<()> {
    let cams = state.store.list_cameras().await;
    match action {
        StreamDeckAction::ToggleAutoTrack { camera_index } => {
            if let Some(cam) = cams.get(camera_index) {
                let mut f = cam.framing.clone();
                f.auto_track_enabled = !f.auto_track_enabled;
                let _ = state.store.update_framing(cam.id, f.clone()).await;
                if let Some(h) = state.media.get_handle(cam.id).await {
                    h.update_framing(f).await;
                }
            }
        }
        StreamDeckAction::SetFramingPreset { camera_index, preset } => {
            if let Some(cam) = cams.get(camera_index) {
                let mut f = cam.framing.clone();
                f.preset = preset;
                let _ = state.store.update_framing(cam.id, f.clone()).await;
                if let Some(h) = state.media.get_handle(cam.id).await {
                    h.update_framing(f).await;
                }
            }
        }
        StreamDeckAction::SetRuleOfThirds { camera_index, thirds } => {
            if let Some(cam) = cams.get(camera_index) {
                let mut f = cam.framing.clone();
                f.rule_of_thirds = thirds;
                let _ = state.store.update_framing(cam.id, f.clone()).await;
                if let Some(h) = state.media.get_handle(cam.id).await {
                    h.update_framing(f).await;
                }
            }
        }
        StreamDeckAction::TriggerOnePushAf { camera_index } => {
            if let Some(cam) = cams.get(camera_index) {
                if let Some(drv) = state.ptz.get(cam.id).await {
                    let _ = drv.focus(FocusCommand::OnePush).await;
                }
            }
        }
        StreamDeckAction::RecallPreset { camera_index, slot } => {
            if let Some(cam) = cams.get(camera_index) {
                state.tracking.notify_manual_override(cam.id, 3.0).await;
                if let Some(drv) = state.ptz.get(cam.id).await {
                    let _ = drv.recall_preset(slot).await;
                }
            }
        }
        StreamDeckAction::ToggleNdiOut { camera_index } => {
            if let Some(cam) = cams.get(camera_index) {
                let mut ndi = cam.ndi_output.clone();
                ndi.enabled = !ndi.enabled;
                let _ = state.store.update_ndi_output(cam.id, ndi.clone()).await;
                if let Some(h) = state.media.get_handle(cam.id).await {
                    h.update_ndi_output(ndi).await;
                }
            }
        }
        StreamDeckAction::DialZoomStep { camera_index, delta } => {
            if let Some(cam) = cams.get(camera_index) {
                state.tracking.notify_manual_override(cam.id, 1.5).await;
                if let Some(drv) = state.ptz.get(cam.id).await {
                    let _ = drv.zoom_speed(delta.clamp(-1.0, 1.0)).await;
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    let _ = drv.zoom_speed(0.0).await;
                }
            }
        }
        StreamDeckAction::DialFocusStep { camera_index, delta } => {
            if let Some(cam) = cams.get(camera_index) {
                if let Some(drv) = state.ptz.get(cam.id).await {
                    let cmd = if delta >= 0.0 {
                        FocusCommand::Far { speed: delta.abs().clamp(0.1, 1.0) }
                    } else {
                        FocusCommand::Near { speed: delta.abs().clamp(0.1, 1.0) }
                    };
                    let _ = drv.focus(cmd).await;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let _ = drv.focus(FocusCommand::Stop).await;
                }
            }
        }
        StreamDeckAction::DialExposureStep { camera_index, delta } => {
            if let Some(cam) = cams.get(camera_index) {
                let mut c = cam.color.clone();
                c.exposure = (c.exposure + delta).clamp(-2.0, 2.0);
                let _ = state.store.update_color(cam.id, c.clone()).await;
                if let Some(h) = state.media.get_handle(cam.id).await {
                    h.update_color(c).await;
                }
            }
        }
        StreamDeckAction::DialWhiteBalanceStep { camera_index, delta } => {
            if let Some(cam) = cams.get(camera_index) {
                let mut c = cam.color.clone();
                c.white_balance_temp = (c.white_balance_temp + delta).clamp(-1.0, 1.0);
                let _ = state.store.update_color(cam.id, c.clone()).await;
                if let Some(h) = state.media.get_handle(cam.id).await {
                    h.update_color(c).await;
                }
            }
        }
        StreamDeckAction::VmixCut { input } => {
            state
                .vmix
                .execute(&VmixSwitchCommand {
                    function: "Cut".into(),
                    input: Some(input),
                    value: None,
                    duration_ms: None,
                    mix: None,
                })
                .await?;
        }
        StreamDeckAction::VmixFade { input, duration_ms } => {
            state
                .vmix
                .execute(&VmixSwitchCommand {
                    function: "Fade".into(),
                    input: Some(input),
                    value: None,
                    duration_ms: Some(duration_ms),
                    mix: None,
                })
                .await?;
        }
    }
    Ok(())
}

async fn snapshot_jpeg(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
) -> impl IntoResponse {
    let Some(handle) = state.media.get_handle(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let frame = handle.preview_rx.borrow().clone();
    match frame {
        Some(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(bytes))
            .unwrap(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
struct MjpegQuery {
    /// Optional frame rate cap (e.g. max_fps=12 for mobile/tablet or thumbnail grids)
    max_fps: Option<u32>,
}

async fn stream_mjpeg(
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
    Query(query): Query<MjpegQuery>,
) -> impl IntoResponse {
    let Some(handle) = state.media.get_handle(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let min_interval = query
        .max_fps
        .filter(|&f| f > 0 && f < 60)
        .map(|f| Duration::from_millis((1000 / f) as u64));

    let rx = handle.preview_rx;
    let shutdown_rx = state.shutdown_tx.subscribe();
    let body_stream = stream::unfold(
        (rx, shutdown_rx, Instant::now() - Duration::from_secs(1)),
        move |(mut rx, mut shutdown_rx, mut last_sent)| async move {
            loop {
                if *shutdown_rx.borrow() {
                    return None;
                }
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            return None;
                        }
                    }
                    changed = rx.changed() => {
                        if changed.is_err() {
                            return None;
                        }
                    }
                }
                if let Some(interval) = min_interval {
                    if last_sent.elapsed() < interval {
                        continue;
                    }
                }
                let Some(frame) = rx.borrow().clone() else {
                    continue;
                };
                last_sent = Instant::now();
                let mut chunk = Vec::with_capacity(frame.len() + 96);
                chunk.extend_from_slice(b"--vcframe\r\nContent-Type: image/jpeg\r\nContent-Length: ");
                chunk.extend_from_slice(frame.len().to_string().as_bytes());
                chunk.extend_from_slice(b"\r\n\r\n");
                chunk.extend_from_slice(&frame);
                chunk.extend_from_slice(b"\r\n");
                return Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from(chunk)),
                    (rx, shutdown_rx, last_sent),
                ));
            }
        },
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "multipart/x-mixed-replace; boundary=vcframe")
        .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .body(Body::from_stream(body_stream))
        .unwrap()
}

async fn ws_telemetry(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_telemetry_ws(socket, state))
}

async fn handle_telemetry_ws(mut socket: WebSocket, state: AppState) {
    // Immediately send hardware & runtime capabilities
    let caps_event = DaemonWsEvent::Capabilities(state.media.capabilities());
    if let Ok(json) = serde_json::to_string(&caps_event) {
        if socket.send(Message::Text(json.into())).await.is_err() {
            return;
        }
    }

    let mut interval = tokio::time::interval(Duration::from_millis(33)); // ~30 Hz
    let mut shutdown_rx = state.shutdown_tx.subscribe();
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
            _ = interval.tick() => {
                let cameras = state.media.collect_telemetry().await;
                let batch = DaemonWsEvent::TelemetryBatch { cameras };
                if let Ok(json) = serde_json::to_string(&batch) {
                    if socket.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

async fn run_osc_listener(port: u16, state: AppState) {
    let bind = format!("0.0.0.0:{port}");
    let Ok(sock) = UdpSocket::bind(&bind).await else {
        warn!(bind = %bind, "Failed binding OSC UDP listener");
        return;
    };
    info!(bind = %bind, "OSC control listener ready (Bitfocus / Stream Deck compatible)");

    let mut buf = [0u8; 2048];
    while let Ok((len, _peer)) = sock.recv_from(&mut buf).await {
        if let Ok((_, rosc::OscPacket::Message(msg))) = rosc::decoder::decode_udp(&buf[..len]) {
            info!(addr = %msg.addr, args = ?msg.args, "Received OSC message");
            let parts: Vec<&str> = msg.addr.trim_start_matches('/').split('/').collect();
            let first_str = msg.args.first().and_then(|a| a.clone().string());
            let first_int = msg.args.first().and_then(|a| a.clone().int());

            match parts.as_slice() {
                ["cam", idx_s, "track", "toggle"] => {
                    if let Ok(idx) = idx_s.parse::<usize>() {
                        let _ = execute_surface_action(&state, StreamDeckAction::ToggleAutoTrack { camera_index: idx }).await;
                    }
                }
                ["cam", idx_s, "framing", "preset"] => {
                    if let (Ok(idx), Some(p)) = (idx_s.parse::<usize>(), first_str.as_deref()) {
                        let preset = match p {
                            "close_up" => FramingPreset::CloseUp,
                            "full_height" => FramingPreset::FullHeight,
                            _ => FramingPreset::WaistUp,
                        };
                        let _ = execute_surface_action(&state, StreamDeckAction::SetFramingPreset { camera_index: idx, preset }).await;
                    }
                }
                ["cam", idx_s, "framing", "thirds"] => {
                    if let (Ok(idx), Some(t)) = (idx_s.parse::<usize>(), first_str.as_deref()) {
                        let thirds = match t {
                            "left_third" => RuleOfThirds::LeftThird,
                            "right_third" => RuleOfThirds::RightThird,
                            "auto_lead_room" => RuleOfThirds::AutoLeadRoom,
                            _ => RuleOfThirds::Center,
                        };
                        let _ = execute_surface_action(&state, StreamDeckAction::SetRuleOfThirds { camera_index: idx, thirds }).await;
                    }
                }
                ["cam", idx_s, "focus", "one_push"] => {
                    if let Ok(idx) = idx_s.parse::<usize>() {
                        let _ = execute_surface_action(&state, StreamDeckAction::TriggerOnePushAf { camera_index: idx }).await;
                    }
                }
                ["cam", idx_s, "preset", "recall"] => {
                    if let (Ok(idx), Some(slot)) = (idx_s.parse::<usize>(), first_int) {
                        let _ = execute_surface_action(&state, StreamDeckAction::RecallPreset { camera_index: idx, slot: slot.clamp(0, 127) as u8 }).await;
                    }
                }
                ["cam", idx_s, "ndi", "toggle"] => {
                    if let Ok(idx) = idx_s.parse::<usize>() {
                        let _ = execute_surface_action(&state, StreamDeckAction::ToggleNdiOut { camera_index: idx }).await;
                    }
                }
                ["vmix", "cut"] => {
                    if let Some(inp) = first_str.or_else(|| first_int.map(|i| i.to_string())) {
                        let _ = execute_surface_action(&state, StreamDeckAction::VmixCut { input: inp }).await;
                    }
                }
                ["vmix", "fade"] => {
                    if let Some(inp) = first_str.or_else(|| first_int.map(|i| i.to_string())) {
                        let _ = execute_surface_action(&state, StreamDeckAction::VmixFade { input: inp, duration_ms: 500 }).await;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use vc_core::PtzConfig;

    fn test_app() -> (Router, tempfile::TempDir, watch::Receiver<bool>) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::load_or_create(dir.path().join("test.db")).unwrap();
        let media = MediaEngine::new();
        let ptz = PtzManager::new(media.clone());
        let tracking = TrackingSupervisor::new(store.clone(), media.clone(), ptz.clone());
        let vmix = VmixClient::new(VmixConfig::default());
        let buttons = BitfocusButtonsClient::new();
        let streamdeck = StreamDeckController::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let state = AppState {
            store,
            media,
            ptz,
            tracking,
            vmix,
            buttons,
            streamdeck,
            http_port: 8765,
            osc_port: 9000,
            lan_access_enabled: true,
            web_ui_dir: None,
            shutdown_tx,
        };
        (build_router(state), dir, shutdown_rx)
    }

    #[tokio::test]
    async fn health_and_camera_crud_happy_and_404_paths() {
        let (app, _dir, _shutdown_rx) = test_app();

        // Health happy path
        let res = app
            .clone()
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("\"status\":\"ok\""));

        // 404 failure paths for missing camera UUID across endpoints
        let missing = Uuid::new_v4();
        for (method, uri, payload) in [
            ("GET", format!("/api/v1/cameras/{missing}"), None),
            ("DELETE", format!("/api/v1/cameras/{missing}"), None),
            ("GET", format!("/api/v1/cameras/{missing}/snapshot.jpg"), None),
            ("PUT", format!("/api/v1/cameras/{missing}/framing"), Some(serde_json::to_string(&FramingConfig::default()).unwrap())),
            ("PUT", format!("/api/v1/cameras/{missing}/color"), Some(serde_json::to_string(&ColorAdjustments::default()).unwrap())),
            ("POST", format!("/api/v1/cameras/{missing}/tracking/select"), Some("{\"track_id\":1}".to_string())),
        ] {
            let mut builder = Request::builder().method(method).uri(&uri);
            let req = if let Some(json) = payload {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                builder.body(Body::from(json)).unwrap()
            } else {
                builder.body(Body::empty()).unwrap()
            };
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "expected 404 for {method} {uri}");
        }

        // Create -> Get -> Delete roundtrip (enabled: false avoids spawning capture worker)
        let mut cam = CameraConfig::new_demo("API Test Cam", "smpte");
        cam.enabled = false;
        let create_req = Request::post("/api/v1/cameras")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&cam).unwrap()))
            .unwrap();
        let created = app.clone().oneshot(create_req).await.unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        let fetched = app
            .clone()
            .oneshot(Request::get(format!("/api/v1/cameras/{}", cam.id)).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);

        let deleted = app
            .clone()
            .oneshot(Request::delete(format!("/api/v1/cameras/{}", cam.id)).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

        let after_delete = app
            .clone()
            .oneshot(Request::get(format!("/api/v1/cameras/{}", cam.id)).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(after_delete.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bad_requests_and_disabled_integrations_return_proper_error_codes() {
        let (app, _dir, shutdown_rx) = test_app();

        // Invalid UUID path parameter -> 400 Bad Request
        let bad_id = app
            .clone()
            .oneshot(Request::get("/api/v1/cameras/not-a-valid-uuid").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(bad_id.status(), StatusCode::BAD_REQUEST);

        // Malformed JSON payload -> 400 Bad Request
        let bad_json = app
            .clone()
            .oneshot(
                Request::post("/api/v1/cameras")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{broken_json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_json.status(), StatusCode::BAD_REQUEST);

        // Calibrating a camera with PtzConfig::None -> 400 Bad Request
        let mut cam = CameraConfig::new_demo("No-PTZ Cam", "smpte");
        cam.enabled = false;
        cam.ptz = PtzConfig::None;
        let _ = app
            .clone()
            .oneshot(
                Request::post("/api/v1/cameras")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&cam).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cal_resp = app
            .clone()
            .oneshot(
                Request::post(format!("/api/v1/cameras/{}/ptz/calibrate", cam.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cal_resp.status(), StatusCode::BAD_REQUEST);

        // Disabled vMix command -> 502 Bad Gateway
        let vmix_resp = app
            .clone()
            .oneshot(
                Request::post("/api/v1/integrations/vmix/command")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{\"function\":\"Cut\",\"input\":\"1\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(vmix_resp.status(), StatusCode::BAD_GATEWAY);

        // Disabled Bitfocus Buttons trigger -> 502 Bad Gateway
        let buttons_resp = app
            .clone()
            .oneshot(
                Request::post("/api/v1/integrations/buttons/trigger")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{\"endpoint\":\"/control/cuelist/v1/go\",\"payload\":{}}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(buttons_resp.status(), StatusCode::BAD_GATEWAY);

        // Graceful shutdown endpoint signals watch channel and returns 202 Accepted
        assert!(!*shutdown_rx.borrow());
        let shutdown_resp = app
            .clone()
            .oneshot(Request::post("/api/v1/shutdown").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(shutdown_resp.status(), StatusCode::ACCEPTED);
        assert!(*shutdown_rx.borrow());
    }
}
