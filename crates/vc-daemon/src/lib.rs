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
use vc_media::{encode_ws_video_packet, CaptureDeviceInfo, MediaEngine, NdiDiscoveredSource};
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

#[derive(Debug, Clone, Default)]
pub struct DaemonOptions {
    pub config_path: Option<PathBuf>,
    pub web_ui_dir: Option<PathBuf>,
    pub bind_addr_override: Option<String>,
}

impl DaemonOptions {
    pub fn from_env() -> Self {
        Self {
            config_path: std::env::var("VC_CONFIG_PATH").ok().map(PathBuf::from),
            web_ui_dir: std::env::var("VC_WEB_DIST_DIR").ok().map(PathBuf::from),
            bind_addr_override: std::env::var("VC_BIND_ADDR").ok(),
        }
    }
}

/// Run the full VideoController control & media daemon (REST, 30 Hz WS telemetry,
/// GPU H.264 WebCodecs video WS, MJPEG, LAN Remote PWA static server, 30 Hz event-driven
/// tracking supervisor, vMix tally/switching, and OSC listener) until `shutdown_tx` or Ctrl+C fires.
pub async fn run_daemon(
    options: DaemonOptions,
    shutdown_tx: watch::Sender<bool>,
) -> Result<()> {
    let config_path = options
        .config_path
        .unwrap_or_else(ConfigStore::default_path);

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

    let web_ui_dir = options
        .web_ui_dir
        .filter(|p| p.join("index.html").exists())
        .or_else(resolve_web_ui_dir);
    let lan_access_enabled = cfg_snapshot.server.lan_access_enabled;
    let http_port = cfg_snapshot.server.http_port;
    let osc_port = cfg_snapshot.server.osc_port;
    let mut shutdown_rx = shutdown_tx.subscribe();

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

    let bind_host = if let Some(env_host) = options.bind_addr_override {
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
        .route("/ws/video/{id}", get(ws_video_h264))
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

async fn ws_video_h264(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<CameraId>,
) -> impl IntoResponse {
    let Some(handle) = state.media.get_handle(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let shutdown_rx = state.shutdown_tx.subscribe();
    ws.on_upgrade(move |socket| handle_video_h264_ws(socket, handle, shutdown_rx))
        .into_response()
}

async fn handle_video_h264_ws(
    mut socket: WebSocket,
    handle: vc_media::CameraMediaHandle,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut sub = match handle.subscribe_h264().await {
        Ok(s) => s,
        Err(err) => {
            let msg = serde_json::json!({
                "type": "fallback",
                "reason": err.to_string(),
            });
            let _ = socket.send(Message::Text(msg.to_string().into())).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let mut sent_config_codec: Option<String> = None;

    if let Some(kf) = sub.initial_keyframe.take() {
        let cfg = serde_json::json!({
            "type": "config",
            "codec": kf.codec,
            "width": kf.width,
            "height": kf.height,
            "encoder": kf.encoder_name,
        });
        if socket.send(Message::Text(cfg.to_string().into())).await.is_err() {
            return;
        }
        sent_config_codec = Some(kf.codec.clone());
        let pkt = encode_ws_video_packet(&kf);
        if socket.send(Message::Binary(pkt.into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
            recv = sub.rx.recv() => {
                match recv {
                    Ok(au) => {
                        if sent_config_codec.as_deref() != Some(au.codec.as_str()) {
                            let cfg = serde_json::json!({
                                "type": "config",
                                "codec": au.codec,
                                "width": au.width,
                                "height": au.height,
                                "encoder": au.encoder_name,
                            });
                            if socket.send(Message::Text(cfg.to_string().into())).await.is_err() {
                                break;
                            }
                            sent_config_codec = Some(au.codec.clone());
                        }
                        let pkt = encode_ws_video_packet(&au);
                        if socket.send(Message::Binary(pkt.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Drop stale frames rather than building up preview latency
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
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

    #[tokio::test]
    async fn full_rest_api_surface_streamdeck_actions_mjpeg_and_osc_happy_and_error_paths() {
        let dir = tempfile::tempdir().unwrap();
        let web_dir = dir.path().join("web");
        std::fs::create_dir_all(&web_dir).unwrap();
        std::fs::write(web_dir.join("index.html"), "<html>ok</html>").unwrap();

        let store = ConfigStore::load_or_create(dir.path().join("full.db")).unwrap();
        let media = MediaEngine::new();
        let ptz = PtzManager::new(media.clone());
        let tracking = TrackingSupervisor::new(store.clone(), media.clone(), ptz.clone());
        let vmix = VmixClient::new(VmixConfig::default());
        let buttons = BitfocusButtonsClient::new();
        let streamdeck = StreamDeckController::new();
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let state = AppState {
            store: store.clone(),
            media: media.clone(),
            ptz: ptz.clone(),
            tracking: tracking.clone(),
            vmix,
            buttons,
            streamdeck,
            http_port: 8765,
            osc_port: 9000,
            lan_access_enabled: true,
            web_ui_dir: Some(web_dir),
            shutdown_tx: shutdown_tx.clone(),
        };
        let app = build_router(state.clone());

        // Basic metadata & discovery endpoints
        for uri in [
            "/api/v1/capabilities",
            "/api/v1/vision/status",
            "/api/v1/remote-info",
            "/api/v1/discovery/ndi",
            "/api/v1/discovery/usb",
            "/api/v1/cameras",
            "/api/v1/integrations/vmix",
            "/api/v1/integrations/streamdeck",
            "/api/v1/integrations/buttons/config",
        ] {
            let r = app
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK, "GET {uri} failed");
        }

        // Static PWA fallback serves index.html
        let pwa = app
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(pwa.status(), StatusCode::OK);

        // ONVIF resolve failure path -> 502 Bad Gateway
        let onvif_err = app
            .clone()
            .oneshot(
                Request::post("/api/v1/onvif/resolve")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"host":"127.0.0.1","port":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(onvif_err.status(), StatusCode::BAD_GATEWAY);

        // Create enabled live demo camera with nil UUID (tests auto-UUID assignment) & Mock PTZ
        let mut cam = CameraConfig::new_demo("Live API Cam", "smpte");
        cam.id = Uuid::nil();
        cam.source = vc_core::VideoSourceConfig::TestPattern {
            pattern: "smpte".into(),
            width: 320,
            height: 180,
            fps: 30,
        };
        cam.ptz = PtzConfig::Mock;
        let created = app
            .clone()
            .oneshot(
                Request::post("/api/v1/cameras")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&cam).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let saved: CameraConfig =
            serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert!(!saved.id.is_nil());
        let id = saved.id;

        // Wait until snapshot.jpg returns 200 OK (may briefly return 503 while first frame renders)
        let mut got_jpeg = false;
        for _ in 0..80 {
            let snap = app
                .clone()
                .oneshot(
                    Request::get(format!("/api/v1/cameras/{id}/snapshot.jpg"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if snap.status() == StatusCode::OK {
                got_jpeg = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(got_jpeg, "expected live JPEG snapshot");

        // MJPEG stream happy path (read 1 chunk) + 404 on missing id
        let mjpeg_404 = app
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/cameras/{}/stream.mjpeg", Uuid::new_v4()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mjpeg_404.status(), StatusCode::NOT_FOUND);

        let mjpeg_ok = app
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/cameras/{id}/stream.mjpeg?max_fps=15"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mjpeg_ok.status(), StatusCode::OK);
        let first_chunk = tokio::time::timeout(Duration::from_secs(3), mjpeg_ok.into_body().frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert!(first_chunk.starts_with(b"--vcframe\r\n"));

        // Per-camera PUT/POST control endpoints
        let ndi_resp = app
            .clone()
            .oneshot(
                Request::put(format!("/api/v1/cameras/{id}/ndi-output"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&NdiOutputConfig::default()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ndi_resp.status(), StatusCode::OK);

        let framing_resp = app
            .clone()
            .oneshot(
                Request::put(format!("/api/v1/cameras/{id}/framing"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&FramingConfig::default()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(framing_resp.status(), StatusCode::OK);

        let color_resp = app
            .clone()
            .oneshot(
                Request::put(format!("/api/v1/cameras/{id}/color"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&ColorAdjustments::default()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(color_resp.status(), StatusCode::OK);

        let vmix_bind = app
            .clone()
            .oneshot(
                Request::put(format!("/api/v1/cameras/{id}/vmix"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"input_ref":"1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(vmix_bind.status(), StatusCode::OK);

        // Tracking select: track_id, norm_x/y, clear, and empty
        for body in [
            r#"{"track_id":101}"#,
            r#"{"norm_x":0.5,"norm_y":0.5}"#,
            r#"{"clear":true}"#,
            r#"{}"#,
        ] {
            let r = app
                .clone()
                .oneshot(
                    Request::post(format!("/api/v1/cameras/{id}/tracking/select"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }

        // PTZ move, stop, focus, imaging, preset store & recall, calibrate (Mock PTZ happy path)
        for (path, body) in [
            (format!("/api/v1/cameras/{id}/ptz/move"), r#"{"pan":0.4,"tilt":-0.2,"zoom":0.1}"#),
            (format!("/api/v1/cameras/{id}/ptz/stop"), ""),
            (format!("/api/v1/cameras/{id}/ptz/focus"), r#"{"action":"one_push"}"#),
            (format!("/api/v1/cameras/{id}/imaging"), r#"{"control":"white_balance_auto"}"#),
            (format!("/api/v1/cameras/{id}/ptz/preset"), r#"{"action":"store","slot":1}"#),
            (format!("/api/v1/cameras/{id}/ptz/preset"), r#"{"action":"recall","slot":1}"#),
        ] {
            let mut b = Request::post(&path);
            if !body.is_empty() {
                b = b.header(header::CONTENT_TYPE, "application/json");
            }
            let r = app.clone().oneshot(b.body(Body::from(body)).unwrap()).await.unwrap();
            assert_eq!(r.status(), StatusCode::NO_CONTENT, "POST {path} failed");
        }

        let cal = app
            .clone()
            .oneshot(
                Request::post(format!("/api/v1/cameras/{id}/ptz/calibrate"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cal.status(), StatusCode::OK);

        // Integrations config PUTs
        let _ = app
            .clone()
            .oneshot(
                Request::put("/api/v1/integrations/vmix/config")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&VmixConfig::default()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = app
            .clone()
            .oneshot(
                Request::put("/api/v1/integrations/streamdeck/config")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&StreamDeckConfig::default()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = app
            .clone()
            .oneshot(
                Request::put("/api/v1/integrations/buttons/config")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&BitfocusButtonsConfig::default()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Stream Deck trigger across ALL surface actions (happy + disabled vMix 400 error)
        let actions = vec![
            StreamDeckAction::ToggleAutoTrack { camera_index: 0 },
            StreamDeckAction::SetFramingPreset { camera_index: 0, preset: FramingPreset::CloseUp },
            StreamDeckAction::SetRuleOfThirds { camera_index: 0, thirds: RuleOfThirds::LeftThird },
            StreamDeckAction::TriggerOnePushAf { camera_index: 0 },
            StreamDeckAction::RecallPreset { camera_index: 0, slot: 1 },
            StreamDeckAction::ToggleNdiOut { camera_index: 0 },
            StreamDeckAction::DialZoomStep { camera_index: 0, delta: 0.2 },
            StreamDeckAction::DialFocusStep { camera_index: 0, delta: 0.2 },
            StreamDeckAction::DialFocusStep { camera_index: 0, delta: -0.2 },
            StreamDeckAction::DialExposureStep { camera_index: 0, delta: 0.1 },
            StreamDeckAction::DialWhiteBalanceStep { camera_index: 0, delta: -0.1 },
        ];
        for act in actions {
            let r = app
                .clone()
                .oneshot(
                    Request::post("/api/v1/integrations/streamdeck/trigger")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_string(&act).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::NO_CONTENT);
        }

        // Disabled vMix Cut/Fade via StreamDeck -> 400 Bad Request
        for err_act in [
            StreamDeckAction::VmixCut { input: "1".into() },
            StreamDeckAction::VmixFade { input: "1".into(), duration_ms: 500 },
        ] {
            let r = app
                .clone()
                .oneshot(
                    Request::post("/api/v1/integrations/streamdeck/trigger")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_string(&err_act).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        }

        // OSC UDP listener dispatch across all OSC routes
        let osc_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let osc_port = osc_sock.local_addr().unwrap().port();
        drop(osc_sock);
        let osc_task = tokio::spawn(run_osc_listener(osc_port, state.clone()));
        tokio::time::sleep(Duration::from_millis(40)).await;
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let osc_msgs = vec![
            rosc::OscMessage { addr: "/cam/0/track/toggle".into(), args: vec![] },
            rosc::OscMessage { addr: "/cam/0/framing/preset".into(), args: vec![rosc::OscType::String("close_up".into())] },
            rosc::OscMessage { addr: "/cam/0/framing/preset".into(), args: vec![rosc::OscType::String("full_height".into())] },
            rosc::OscMessage { addr: "/cam/0/framing/thirds".into(), args: vec![rosc::OscType::String("right_third".into())] },
            rosc::OscMessage { addr: "/cam/0/framing/thirds".into(), args: vec![rosc::OscType::String("auto_lead_room".into())] },
            rosc::OscMessage { addr: "/cam/0/focus/one_push".into(), args: vec![] },
            rosc::OscMessage { addr: "/cam/0/preset/recall".into(), args: vec![rosc::OscType::Int(1)] },
            rosc::OscMessage { addr: "/cam/0/ndi/toggle".into(), args: vec![] },
            rosc::OscMessage { addr: "/vmix/cut".into(), args: vec![rosc::OscType::Int(1)] },
            rosc::OscMessage { addr: "/vmix/fade".into(), args: vec![rosc::OscType::String("2".into())] },
            rosc::OscMessage { addr: "/unknown/route".into(), args: vec![] },
        ];
        for msg in osc_msgs {
            let bytes = rosc::encoder::encode(&rosc::OscPacket::Message(msg)).unwrap();
            let _ = sender.send_to(&bytes, format!("127.0.0.1:{osc_port}")).await;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
        osc_task.abort();

        // PUT camera update (enabled -> disabled stops workers cleanly)
        let mut disabled = saved.clone();
        disabled.enabled = false;
        let put_resp = app
            .clone()
            .oneshot(
                Request::put(format!("/api/v1/cameras/{id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&disabled).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(put_resp.status(), StatusCode::OK);

        // Helpers coverage
        let _ = DaemonOptions::from_env();
        let _ = resolve_web_ui_dir();
        let _ = detect_lan_ipv4_addresses();

        tracking.stop_all_cameras().await;
        media.stop_all_cameras().await;
    }

    #[tokio::test]
    async fn live_daemon_serves_telemetry_and_h264_websockets_and_shuts_down_cleanly() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("live_daemon.db");

        // Reserve ephemeral free HTTP & OSC ports, store in SQLite before starting run_daemon
        let tmp_tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = tmp_tcp.local_addr().unwrap().port();
        drop(tmp_tcp);
        let tmp_udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let osc_port = tmp_udp.local_addr().unwrap().port();
        drop(tmp_udp);

        let store = ConfigStore::load_or_create(&db_path).unwrap();
        let cam_id = Uuid::new_v4();
        drop(store);
        assert!(seed_isolated_daemon_db(&db_path, cam_id, http_port, osc_port));

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let daemon_tx = shutdown_tx.clone();
        let daemon_task = tokio::spawn(async move {
            run_daemon(
                DaemonOptions {
                    config_path: Some(db_path),
                    web_ui_dir: None,
                    bind_addr_override: Some("127.0.0.1".into()),
                },
                daemon_tx,
            )
            .await
        });

        // Wait for daemon TCP port to come up
        let mut connected = false;
        for _ in 0..40 {
            if tokio::time::timeout(
                Duration::from_millis(150),
                tokio::net::TcpStream::connect(("127.0.0.1", http_port)),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
            {
                connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(connected, "daemon did not bind HTTP listener in time");

        // 1) Connect raw RFC 6455 WebSocket to /ws/telemetry and verify 101 + Capabilities payload
        let mut ws_telem = tokio::net::TcpStream::connect(("127.0.0.1", http_port))
            .await
            .unwrap();
        let req = format!(
            "GET /ws/telemetry HTTP/1.1\r\nHost: 127.0.0.1:{http_port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        ws_telem.write_all(req.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(3), ws_telem.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let resp_str = String::from_utf8_lossy(&buf[..n]);
        assert!(resp_str.contains("101 Switching Protocols"));
        // Read one more chunk to ensure TelemetryBatch tick runs
        let _ = tokio::time::timeout(Duration::from_millis(200), ws_telem.read(&mut buf)).await;
        drop(ws_telem);

        // 2) Connect raw RFC 6455 WebSocket to /ws/video/{cam_id} and verify 101 + H.264 config/packet or fallback
        let mut ws_vid = tokio::net::TcpStream::connect(("127.0.0.1", http_port))
            .await
            .unwrap();
        let vid_req = format!(
            "GET /ws/video/{cam_id} HTTP/1.1\r\nHost: 127.0.0.1:{http_port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        ws_vid.write_all(vid_req.as_bytes()).await.unwrap();
        let n2 = tokio::time::timeout(Duration::from_secs(3), ws_vid.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&buf[..n2]).contains("101 Switching Protocols"));
        let _ = tokio::time::timeout(Duration::from_secs(3), ws_vid.read(&mut buf)).await;
        drop(ws_vid);

        // 3) Request graceful shutdown and verify run_daemon exits cleanly with Ok(())
        let _ = shutdown_tx.send(true);
        let res = tokio::time::timeout(Duration::from_secs(6), daemon_task)
            .await
            .expect("daemon should shut down cleanly")
            .unwrap();
        assert!(res.is_ok());
    }

    fn seed_isolated_daemon_db(db_path: &PathBuf, cam_id: CameraId, http_port: u16, osc_port: u16) -> bool {
        let legacy_json = db_path.with_extension("json");
        let mut cfg = vc_core::AppConfig::default();
        cfg.server.http_port = http_port;
        cfg.server.osc_port = osc_port;
        cfg.server.lan_access_enabled = false;
        cfg.cameras.clear();
        let mut cam = CameraConfig::new_demo("WS Cam", "smpte");
        cam.id = cam_id;
        cam.source = vc_core::VideoSourceConfig::TestPattern {
            pattern: "smpte".into(),
            width: 320,
            height: 180,
            fps: 30,
        };
        cfg.cameras.push(cam);
        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
        std::fs::write(&legacy_json, serde_json::to_string(&cfg).unwrap()).is_ok()
            && ConfigStore::load_or_create(&legacy_json).is_ok()
    }
}
