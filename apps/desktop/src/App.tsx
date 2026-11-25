import React, { useCallback, useEffect, useMemo, useState } from "react";
import { api, IS_ELECTRON, mjpegStreamUrl, telemetryWsUrl } from "./api";
import { AddCameraModal } from "./components/AddCameraModal";
import { CameraMonitor } from "./components/CameraMonitor";
import { IntegrationsPanel } from "./components/IntegrationsPanel";
import { PtzJoystick } from "./components/PtzJoystick";
import { RemoteAccessModal } from "./components/RemoteAccessModal";
import { SetupWizardModal } from "./components/SetupWizardModal";
import type {
  AutoFocusStrategy,
  CalibrationReport,
  CameraConfig,
  CameraTelemetry,
  ColorAdjustments,
  DaemonWsEvent,
  FramingPreset,
  RuleOfThirds,
  SystemCapabilities,
  VisionEngineStatus,
} from "./types";

type InspectorTab = "tracking" | "ptz" | "multiview" | "video" | "integrations";

function useIsCompactViewport(): boolean {
  const [compact, setCompact] = useState<boolean>(() =>
    typeof window !== "undefined" ? window.matchMedia("(max-width: 960px)").matches : false
  );

  useEffect(() => {
    const mq = window.matchMedia("(max-width: 960px)");
    const listener = (e: MediaQueryListEvent) => setCompact(e.matches);
    setCompact(mq.matches);
    mq.addEventListener("change", listener);
    return () => mq.removeEventListener("change", listener);
  }, []);

  return compact;
}

export const App: React.FC = () => {
  const isCompact = useIsCompactViewport();
  const [connected, setConnected] = useState(false);
  const [capabilities, setCapabilities] = useState<SystemCapabilities | null>(null);
  const [visionStatus, setVisionStatus] = useState<VisionEngineStatus | null>(null);
  const [cameras, setCameras] = useState<CameraConfig[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [telemetryByCam, setTelemetryByCam] = useState<Record<string, CameraTelemetry>>({});
  const [tab, setTab] = useState<InspectorTab>("tracking");
  const [showGrid, setShowGrid] = useState(true);
  const [showSkeleton, setShowSkeleton] = useState(true);
  const [showAddModal, setShowAddModal] = useState(false);
  const [showRemoteModal, setShowRemoteModal] = useState(false);
  const [showSetupModal, setShowSetupModal] = useState(false);
  const [calibrating, setCalibrating] = useState(false);
  const [calReport, setCalReport] = useState<CalibrationReport | null>(null);
  const [calError, setCalError] = useState<string | null>(null);

  // When returning from compact to desktop, switch off the mobile-only "multiview" inspector tab
  useEffect(() => {
    if (!isCompact && tab === "multiview") {
      setTab("tracking");
    }
  }, [isCompact, tab]);

  const loadCameras = useCallback(async () => {
    try {
      const list = await api.listCameras();
      setCameras(list);
      setSelectedId((prev) => (prev && list.some((c) => c.id === prev) ? prev : list[0]?.id ?? null));
    } catch {
      // retry via WS lifecycle
    }
  }, []);

  useEffect(() => {
    let ws: WebSocket | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let disposed = false;

    const connect = () => {
      if (disposed) return;
      ws = new WebSocket(telemetryWsUrl());
      ws.onopen = () => {
        setConnected(true);
        void loadCameras();
        void api.getCapabilities().then(setCapabilities).catch(() => {});
        void api.getVisionStatus().then(setVisionStatus).catch(() => {});
      };
      ws.onmessage = (evt) => {
        try {
          const msg = JSON.parse(evt.data) as DaemonWsEvent;
          if (msg.type === "capabilities") {
            setCapabilities(msg);
          } else if (msg.type === "telemetry_batch") {
            const next: Record<string, CameraTelemetry> = {};
            for (const item of msg.cameras) next[item.camera_id] = item;
            setTelemetryByCam(next);
          } else if (msg.type === "cameras_updated") {
            void loadCameras();
          }
        } catch {
          // ignore malformed ws frame
        }
      };
      ws.onclose = () => {
        setConnected(false);
        if (!disposed) {
          timer = setTimeout(connect, 1500);
        }
      };
      ws.onerror = () => ws?.close();
    };

    connect();
    return () => {
      disposed = true;
      if (timer) clearTimeout(timer);
      ws?.close();
    };
  }, [loadCameras]);

  const selectedCamera = useMemo(
    () => cameras.find((c) => c.id === selectedId) ?? cameras[0] ?? null,
    [cameras, selectedId]
  );

  const selectedTelemetry = selectedCamera ? telemetryByCam[selectedCamera.id] : undefined;

  const patchCameraLocal = (updated: CameraConfig) => {
    setCameras((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
  };

  const updateFraming = async (patch: Partial<CameraConfig["framing"]>) => {
    if (!selectedCamera) return;
    const next = { ...selectedCamera.framing, ...patch };
    patchCameraLocal({ ...selectedCamera, framing: next });
    await api.updateFraming(selectedCamera.id, next);
  };

  const updateColor = async (patch: Partial<ColorAdjustments>) => {
    if (!selectedCamera) return;
    const next = { ...selectedCamera.color, ...patch };
    patchCameraLocal({ ...selectedCamera, color: next });
    await api.updateColor(selectedCamera.id, next);
  };

  const updateNdiOut = async (patch: Partial<CameraConfig["ndi_output"]>) => {
    if (!selectedCamera) return;
    const next = { ...selectedCamera.ndi_output, ...patch };
    patchCameraLocal({ ...selectedCamera, ndi_output: next });
    await api.updateNdiOutput(selectedCamera.id, next);
  };

  const gpuLabel = capabilities?.gpu_names?.[0]
    ? `${capabilities.gpu_names[0]} · ${capabilities.decode_backend.replace(/_/g, " ").toUpperCase()}`
    : capabilities?.decode_backend ?? "Probing GPU...";

  const renderMultiviewTiles = (thumbFps: number) => (
    <div className="multiview-strip">
      {cameras.map((cam) => {
        const t = telemetryByCam[cam.id];
        const isSelected = cam.id === selectedCamera?.id;
        return (
          <div
            key={cam.id}
            className={`cam-tile ${isSelected ? "selected" : ""} ${t?.tally === "program" ? "tally-program" : ""}`}
            onClick={() => {
              setSelectedId(cam.id);
              if (isCompact && tab === "multiview") {
                setTab("tracking");
              }
            }}
          >
            <div className="cam-tile-video">
              <img src={mjpegStreamUrl(cam.id, thumbFps)} alt={cam.name} draggable={false} />
            </div>
            <div className="cam-tile-meta">
              <span className="cam-tile-title">{cam.name}</span>
              <div style={{ display: "flex", gap: 4 }}>
                {t?.ndi_output_active && <span className="badge-mini ndi">NDI</span>}
                {cam.framing.auto_track_enabled && <span className="badge-mini track">AI</span>}
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );

  return (
    <div className={`app-shell ${isCompact ? "is-compact" : ""}`}>
      <header className="topbar">
        <div className="brand">
          <span className={`brand-dot ${connected ? "online" : "offline"}`} />
          <span>StageLock</span>
          <span className="brand-sub">{IS_ELECTRON ? "AI PTZ Tracking & Broadcast Studio" : "Remote PWA"}</span>
        </div>

        <div className="status-pills">
          <span className={`pill hide-md ${connected ? "ok" : "warn"}`}>
            {connected ? "Engine Online" : "Reconnecting..."}
          </span>
          <span className="pill hide-lg">{gpuLabel}</span>
          <span className={`pill hide-lg ${capabilities?.ndi_runtime_available ? "ndi" : "warn"}`}>
            {capabilities?.ndi_runtime_available ? "NDI® 6 Ready" : "NDI® Missing"} ·{" "}
            <a href="https://ndi.video" target="_blank" rel="noreferrer">
              ndi.video
            </a>
          </span>
          <button className="btn small" onClick={() => setShowGrid((g) => !g)}>
            {showGrid ? "Guides" : "No Guides"}
          </button>
          <button className="btn small hide-xs" onClick={() => setShowSkeleton((s) => !s)}>
            {showSkeleton ? "Pose On" : "Pose Off"}
          </button>
          <button className="btn small" onClick={() => setShowRemoteModal(true)}>
            Remote QR
          </button>
          <button className="btn small hide-xs" onClick={() => setShowSetupModal(true)}>
            Setup
          </button>
          <button className="btn primary small" onClick={() => setShowAddModal(true)}>
            + Cam
          </button>
        </div>
      </header>

      <main className="workspace">
        <section className="stage-col">
          {selectedCamera ? (
            <CameraMonitor
              camera={selectedCamera}
              telemetry={selectedTelemetry}
              showGrid={showGrid}
              showSkeleton={showSkeleton}
              maxFps={isCompact ? 18 : undefined}
              onSelectPoint={(norm_x, norm_y) => {
                void api.selectSubject(selectedCamera.id, { norm_x, norm_y });
              }}
            />
          ) : (
            <div className="monitor-card" style={{ color: "#64748b", padding: 24, textAlign: "center" }}>
              {connected ? "Tap + Cam to add a video source." : "Connecting to StageLock engine..."}
            </div>
          )}

          {/* Compact mobile/tablet 1-tap camera switcher pill bar */}
          {isCompact ? (
            <div className="mobile-cam-pills">
              {cameras.map((cam) => {
                const t = telemetryByCam[cam.id];
                const active = cam.id === selectedCamera?.id;
                return (
                  <button
                    key={cam.id}
                    className={`cam-pill-btn ${active ? "active" : ""} ${t?.tally === "program" ? "tally-program" : ""}`}
                    onClick={() => setSelectedId(cam.id)}
                  >
                    <span className="cam-pill-name">{cam.name}</span>
                    {cam.framing.auto_track_enabled && <span className="badge-mini track">AI</span>}
                    {t?.ndi_output_active && <span className="badge-mini ndi">NDI</span>}
                  </button>
                );
              })}
            </div>
          ) : (
            renderMultiviewTiles(10)
          )}
        </section>

        <aside className="inspector">
          {!isCompact && (
            <div className="tabs tabs-4">
              <button className={`tab-btn ${tab === "tracking" ? "active" : ""}`} onClick={() => setTab("tracking")}>
                AI Track
              </button>
              <button className={`tab-btn ${tab === "ptz" ? "active" : ""}`} onClick={() => setTab("ptz")}>
                PTZ
              </button>
              <button className={`tab-btn ${tab === "video" ? "active" : ""}`} onClick={() => setTab("video")}>
                Color / NDI
              </button>
              <button
                className={`tab-btn ${tab === "integrations" ? "active" : ""}`}
                onClick={() => setTab("integrations")}
              >
                vMix / Deck
              </button>
            </div>
          )}

          {selectedCamera && (
            <div className="inspector-body">
              {tab === "tracking" && (
                <>
                  <div className="section">
                    <div className="section-title">
                      <span>Auto Tracking</span>
                      <span style={{ color: "#38bdf8" }}>{selectedCamera.name}</span>
                    </div>
                    <button
                      className={`btn btn-hero ${selectedCamera.framing.auto_track_enabled ? "track-on" : ""}`}
                      onClick={() =>
                        void updateFraming({ auto_track_enabled: !selectedCamera.framing.auto_track_enabled })
                      }
                    >
                      {selectedCamera.framing.auto_track_enabled
                        ? "AUTO TRACKING: ACTIVE (Tap to Pause)"
                        : "ENABLE AUTO TRACKING"}
                    </button>
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Framing Shot Preset</span>
                    </div>
                    <div className="btn-grid-3">
                      {(
                        [
                          ["close_up", "Close-Up"],
                          ["waist_up", "Waist Up"],
                          ["full_height", "Full Height"],
                        ] as [FramingPreset, string][]
                      ).map(([val, label]) => (
                        <button
                          key={val}
                          className={`btn ${selectedCamera.framing.preset === val ? "active" : ""}`}
                          onClick={() => void updateFraming({ preset: val })}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Rule of Thirds Composition</span>
                    </div>
                    <div className="btn-grid-4">
                      {(
                        [
                          ["left_third", "Left 1/3"],
                          ["center", "Center"],
                          ["right_third", "Right 1/3"],
                          ["auto_lead_room", "Auto Lead"],
                        ] as [RuleOfThirds, string][]
                      ).map(([val, label]) => (
                        <button
                          key={val}
                          className={`btn small ${selectedCamera.framing.rule_of_thirds === val ? "active" : ""}`}
                          onClick={() => void updateFraming({ rule_of_thirds: val })}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Subject Selection ({selectedTelemetry?.subjects.length ?? 0} in frame)</span>
                      <button
                        className="btn small"
                        onClick={() => void api.selectSubject(selectedCamera.id, { clear: true })}
                      >
                        Clear Lock
                      </button>
                    </div>
                    <div className="btn-grid-2">
                      {(selectedTelemetry?.subjects ?? []).map((s) => (
                        <button
                          key={s.track_id}
                          className={`btn ${s.is_locked ? "track-on" : ""}`}
                          onClick={() => void api.selectSubject(selectedCamera.id, { track_id: s.track_id })}
                        >
                          {s.is_locked ? `Locked #${s.track_id}` : `Follow #${s.track_id}`}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Subject Auto-Focus</span>
                      <button
                        className={`btn small ${selectedCamera.framing.auto_focus_enabled ? "active" : ""}`}
                        onClick={() =>
                          void updateFraming({ auto_focus_enabled: !selectedCamera.framing.auto_focus_enabled })
                        }
                      >
                        {selectedCamera.framing.auto_focus_enabled ? "Auto-AF On" : "Auto-AF Off"}
                      </button>
                    </div>
                    <div className="btn-grid-3">
                      <button
                        className="btn"
                        onClick={() => void api.ptzFocus(selectedCamera.id, { action: "one_push" })}
                      >
                        One-Push AF
                      </button>
                      <button
                        className="btn touch-hold"
                        onContextMenu={(e) => e.preventDefault()}
                        onPointerDown={(e) => {
                          e.preventDefault();
                          e.currentTarget.setPointerCapture(e.pointerId);
                          void api.ptzFocus(selectedCamera.id, { action: "near", speed: 0.4 });
                        }}
                        onPointerUp={() => void api.ptzFocus(selectedCamera.id, { action: "stop" })}
                        onPointerCancel={() => void api.ptzFocus(selectedCamera.id, { action: "stop" })}
                      >
                        Focus Near
                      </button>
                      <button
                        className="btn touch-hold"
                        onContextMenu={(e) => e.preventDefault()}
                        onPointerDown={(e) => {
                          e.preventDefault();
                          e.currentTarget.setPointerCapture(e.pointerId);
                          void api.ptzFocus(selectedCamera.id, { action: "far", speed: 0.4 });
                        }}
                        onPointerUp={() => void api.ptzFocus(selectedCamera.id, { action: "stop" })}
                        onPointerCancel={() => void api.ptzFocus(selectedCamera.id, { action: "stop" })}
                      >
                        Focus Far
                      </button>
                    </div>
                    <div className="btn-grid-2" style={{ marginTop: 2 }}>
                      {(
                        [
                          ["one_push_af", "Tier 1: One-Push AF"],
                          ["active_hill_climb", "Tier 2: Hill-Climb AF"],
                        ] as [AutoFocusStrategy, string][]
                      ).map(([val, label]) => (
                        <button
                          key={val}
                          className={`btn small ${selectedCamera.framing.focus_strategy === val ? "active" : ""}`}
                          onClick={() => void updateFraming({ focus_strategy: val })}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Smooth Motion &amp; Calibration</span>
                      <span style={{ color: "#94a3b8", fontSize: 10.5 }}>
                        P:{(selectedTelemetry?.ptz_velocity.pan ?? 0).toFixed(2)} T:
                        {(selectedTelemetry?.ptz_velocity.tilt ?? 0).toFixed(2)} Z:
                        {(selectedTelemetry?.ptz_velocity.zoom ?? 0).toFixed(2)}
                      </span>
                    </div>

                    <div className="slider-row">
                      <div className="slider-label">
                        <span>Max Tracking Speed</span>
                        <span>{Math.round(selectedCamera.framing.max_speed * 100)}%</span>
                      </div>
                      <input
                        type="range"
                        min={0.15}
                        max={1.0}
                        step={0.05}
                        value={selectedCamera.framing.max_speed}
                        onChange={(e) => void updateFraming({ max_speed: Number(e.target.value) })}
                      />
                    </div>

                    <div className="slider-row">
                      <div className="slider-label">
                        <span>Quiet Deadzone Width</span>
                        <span>±{Math.round(selectedCamera.framing.deadzone_x * 100)}%</span>
                      </div>
                      <input
                        type="range"
                        min={0.03}
                        max={0.22}
                        step={0.01}
                        value={selectedCamera.framing.deadzone_x}
                        onChange={(e) =>
                          void updateFraming({
                            deadzone_x: Number(e.target.value),
                            deadzone_y: Number((Number(e.target.value) * 0.75).toFixed(3)),
                          })
                        }
                      />
                    </div>

                    <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, color: "#cbd5e1" }}>
                      <input
                        type="checkbox"
                        checked={selectedCamera.framing.tally_damping_enabled}
                        onChange={(e) => void updateFraming({ tally_damping_enabled: e.target.checked })}
                      />
                      Tally-aware on-air damping (softer motion on vMix Program)
                    </label>

                    <button
                      className="btn small"
                      disabled={calibrating}
                      onClick={async () => {
                        setCalibrating(true);
                        setCalError(null);
                        try {
                          const rep = await api.ptzCalibrate(selectedCamera.id);
                          setCalReport(rep);
                        } catch (err) {
                          setCalError(err instanceof Error ? err.message : String(err));
                        } finally {
                          setCalibrating(false);
                        }
                      }}
                    >
                      {calibrating ? "Calibrating Pan Pulse..." : "Run PTZ Speed & Latency Calibration"}
                    </button>

                    {calError && (
                      <div role="alert" style={{ fontSize: 11, color: "#fca5a5", lineHeight: 1.4 }}>
                        Calibration failed: {calError}
                      </div>
                    )}

                    {calReport && calReport.camera_id === selectedCamera.id && (
                      <div style={{ fontSize: 11, color: "#86efac", lineHeight: 1.4 }}>
                        Calibrated ({calReport.protocol}): latency {Math.round(calReport.calibration.latency_ms)} ms ·
                        pan×{calReport.calibration.pan_scale.toFixed(2)} · deadband{" "}
                        {calReport.calibration.min_effective_speed.toFixed(2)}
                      </div>
                    )}
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>AI Vision &amp; Re-ID Engine</span>
                      <span style={{ color: "#38bdf8" }}>
                        {Math.round(selectedTelemetry?.inference_fps ?? 0)} fps
                      </span>
                    </div>
                    <div style={{ fontSize: 11.5, color: "#94a3b8", lineHeight: 1.45 }}>
                      <div>
                        <strong>Tracker:</strong> BoT-SORT + CMC + Overlap-Frozen Re-ID
                      </div>
                      <div>
                        <strong>EP:</strong> {visionStatus?.active_execution_provider ?? "Checking..."}
                      </div>
                      <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 6 }}>
                        {(visionStatus?.models ?? []).map((m) => (
                          <span
                            key={m.filename}
                            className={`pill ${m.installed ? "ok" : ""}`}
                            style={{ fontSize: 10.5, padding: "2px 7px" }}
                          >
                            {m.role}: {m.installed ? "ONNX" : "Fallback"} ({m.license})
                          </span>
                        ))}
                      </div>
                    </div>
                  </div>
                </>
              )}

              {tab === "ptz" && (
                <PtzJoystick
                  presets={selectedCamera.presets}
                  onMove={(p, t, z) => void api.ptzMove(selectedCamera.id, p, t, z)}
                  onStop={() => void api.ptzStop(selectedCamera.id)}
                  onPreset={(action, slot) => void api.ptzPreset(selectedCamera.id, action, slot)}
                />
              )}

              {tab === "multiview" && isCompact && (
                <div className="section">
                  <div className="section-title">
                    <span>All Cameras Live Multiview</span>
                    <button className="btn small primary" onClick={() => setShowAddModal(true)}>
                      + Add Camera
                    </button>
                  </div>
                  {renderMultiviewTiles(8)}
                </div>
              )}

              {tab === "video" && (
                <>
                  <div className="section">
                    <div className="section-title">
                      <span>NDI® Re-Broadcast Output</span>
                      <button
                        className={`btn small ${selectedCamera.ndi_output.enabled ? "track-on" : ""}`}
                        onClick={() => void updateNdiOut({ enabled: !selectedCamera.ndi_output.enabled })}
                      >
                        {selectedCamera.ndi_output.enabled ? "NDI LIVE" : "Start NDI Out"}
                      </button>
                    </div>
                    <div className="field">
                      <label>NDI Stream Name</label>
                      <input
                        type="text"
                        value={selectedCamera.ndi_output.stream_name}
                        onChange={(e) => void updateNdiOut({ stream_name: e.target.value })}
                      />
                    </div>
                    <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, color: "#cbd5e1" }}>
                      <input
                        type="checkbox"
                        checked={selectedCamera.ndi_output.include_color_adjustments}
                        onChange={(e) => void updateNdiOut({ include_color_adjustments: e.target.checked })}
                      />
                      Bake color adjustments into NDI output
                    </label>
                    <div className="ndi-footer">
                      NDI® is a registered trademark of Vizrt NDI AB.{" "}
                      <a href="https://ndi.video" target="_blank" rel="noreferrer">
                        ndi.video
                      </a>
                    </div>
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Camera Hardware Imaging (Layer 1)</span>
                      <span style={{ color: "#38bdf8", fontSize: 10 }}>
                        {selectedCamera.ptz.protocol.replace(/_/g, " ").toUpperCase()}
                      </span>
                    </div>
                    <div className="btn-grid-4">
                      <button
                        className="btn small"
                        onClick={() => void api.cameraImaging(selectedCamera.id, { control: "white_balance_auto" })}
                      >
                        WB Auto
                      </button>
                      <button
                        className="btn small"
                        onClick={() => void api.cameraImaging(selectedCamera.id, { control: "white_balance_indoor" })}
                      >
                        Indoor
                      </button>
                      <button
                        className="btn small"
                        onClick={() => void api.cameraImaging(selectedCamera.id, { control: "white_balance_outdoor" })}
                      >
                        Outdoor
                      </button>
                      <button
                        className="btn small active"
                        onClick={() => void api.cameraImaging(selectedCamera.id, { control: "white_balance_one_push" })}
                      >
                        1-Push WB
                      </button>
                    </div>
                    <div className="btn-grid-3">
                      <button
                        className="btn small"
                        onClick={() => void api.cameraImaging(selectedCamera.id, { control: "exposure_auto" })}
                      >
                        AE Auto
                      </button>
                      <button
                        className="btn small"
                        onClick={() =>
                          void api.cameraImaging(selectedCamera.id, { control: "backlight_comp", enabled: true })
                        }
                      >
                        BLC On
                      </button>
                      <button
                        className="btn small"
                        onClick={() =>
                          void api.cameraImaging(selectedCamera.id, { control: "backlight_comp", enabled: false })
                        }
                      >
                        BLC Off
                      </button>
                    </div>
                    <div className="slider-row">
                      <div className="slider-label">
                        <span>Camera Hardware Sharpness</span>
                      </div>
                      <input
                        type="range"
                        min={0}
                        max={1}
                        step={0.05}
                        defaultValue={0.5}
                        onMouseUp={(e) =>
                          void api.cameraImaging(selectedCamera.id, {
                            control: "sharpness",
                            level: Number(e.currentTarget.value),
                          })
                        }
                        onTouchEnd={(e) =>
                          void api.cameraImaging(selectedCamera.id, {
                            control: "sharpness",
                            level: Number(e.currentTarget.value),
                          })
                        }
                      />
                    </div>
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Live Video Grading &amp; Sharpness (Layer 2)</span>
                      <button
                        className="btn small"
                        onClick={() =>
                          void updateColor({
                            white_balance_temp: 0,
                            white_balance_tint: 0,
                            exposure: 0,
                            contrast: 1,
                            saturation: 1,
                            sharpness: 0,
                            brightness: 0,
                          })
                        }
                      >
                        Reset
                      </button>
                    </div>

                    {(
                      [
                        ["white_balance_temp", "WB Temperature", -1, 1, 0.05],
                        ["white_balance_tint", "WB Tint", -1, 1, 0.05],
                        ["exposure", "Exposure (EV)", -2, 2, 0.05],
                        ["contrast", "Contrast", 0.2, 2, 0.05],
                        ["saturation", "Saturation", 0, 2, 0.05],
                        ["brightness", "Brightness", -0.5, 0.5, 0.02],
                        ["sharpness", "Sharpness", 0, 2, 0.05],
                      ] as [keyof ColorAdjustments, string, number, number, number][]
                    ).map(([key, label, min, max, step]) => (
                      <div className="slider-row" key={key}>
                        <div className="slider-label">
                          <span>{label}</span>
                          <span>{selectedCamera.color[key].toFixed(2)}</span>
                        </div>
                        <input
                          type="range"
                          min={min}
                          max={max}
                          step={step}
                          value={selectedCamera.color[key]}
                          onChange={(e) => void updateColor({ [key]: Number(e.target.value) })}
                        />
                      </div>
                    ))}
                  </div>

                  <div className="section">
                    <div className="section-title">
                      <span>Camera Maintenance</span>
                    </div>
                    <button
                      className="btn danger small"
                      onClick={async () => {
                        await api.deleteCamera(selectedCamera.id);
                        await loadCameras();
                      }}
                    >
                      Remove Camera Source
                    </button>
                  </div>
                </>
              )}
              {tab === "integrations" && (
                <IntegrationsPanel selectedCamera={selectedCamera} onCameraUpdated={patchCameraLocal} />
              )}
            </div>
          )}
        </aside>
      </main>

      {/* Mobile / Tablet Thumb-Reachable Bottom Navigation Bar */}
      {isCompact && (
        <nav className="mobile-bottom-nav mobile-nav-5">
          <button className={`mobile-nav-btn ${tab === "tracking" ? "active" : ""}`} onClick={() => setTab("tracking")}>
            Track
          </button>
          <button className={`mobile-nav-btn ${tab === "ptz" ? "active" : ""}`} onClick={() => setTab("ptz")}>
            PTZ
          </button>
          <button className={`mobile-nav-btn ${tab === "multiview" ? "active" : ""}`} onClick={() => setTab("multiview")}>
            Cams
          </button>
          <button className={`mobile-nav-btn ${tab === "video" ? "active" : ""}`} onClick={() => setTab("video")}>
            Color
          </button>
          <button
            className={`mobile-nav-btn ${tab === "integrations" ? "active" : ""}`}
            onClick={() => setTab("integrations")}
          >
            vMix
          </button>
        </nav>
      )}

      {showAddModal && (
        <AddCameraModal
          onClose={() => setShowAddModal(false)}
          onCreate={async (cam) => {
            const created = await api.createCamera(cam);
            await loadCameras();
            setSelectedId(created.id);
          }}
        />
      )}

      {showRemoteModal && <RemoteAccessModal onClose={() => setShowRemoteModal(false)} />}

      {showSetupModal && (
        <SetupWizardModal
          capabilities={capabilities}
          visionStatus={visionStatus}
          onClose={() => setShowSetupModal(false)}
          onOpenAddCamera={() => setShowAddModal(true)}
        />
      )}
    </div>
  );
};
