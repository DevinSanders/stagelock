import React, { useCallback, useState } from "react";
import type { CameraConfig, CameraTelemetry } from "../types";
import { WebCodecsVideo } from "./WebCodecsVideo";

interface Props {
  camera: CameraConfig;
  telemetry?: CameraTelemetry;
  showGrid: boolean;
  showSkeleton: boolean;
  maxFps?: number;
  onSelectPoint: (normX: number, normY: number) => void;
}

export const CameraMonitor: React.FC<Props> = ({
  camera,
  telemetry,
  showGrid,
  showSkeleton,
  maxFps,
  onSelectPoint,
}) => {
  const [transport, setTransport] = useState<{ mode: "h264" | "mjpeg"; encoder?: string }>({
    mode: "h264",
  });
  const handleTransportChange = useCallback((mode: "h264" | "mjpeg", encoder?: string) => {
    setTransport({ mode, encoder });
  }, []);

  const handleClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const normX = (e.clientX - rect.left) / rect.width;
    const normY = (e.clientY - rect.top) / rect.height;
    onSelectPoint(Math.max(0, Math.min(1, normX)), Math.max(0, Math.min(1, normY)));
  };

  const tallyClass =
    telemetry?.tally === "program"
      ? "tally-program"
      : telemetry?.tally === "preview"
        ? "tally-preview"
        : "";

  const streamLabel =
    telemetry?.stream_status.state === "running"
      ? `${telemetry.stream_status.width}×${telemetry.stream_status.height} @ ${Math.round(telemetry.stream_status.fps)}fps · ${telemetry.stream_status.decoder_name}`
      : telemetry?.stream_status.state ?? "connecting";

  const anchor = telemetry?.target_anchor ?? [0.5, 0.3];
  const dz = telemetry?.deadzone ?? { x: 0.42, y: 0.24, width: 0.16, height: 0.12 };

  return (
    <div className={`monitor-card ${tallyClass}`}>
      <div className="monitor-stage">
        <div
          className="monitor-video-wrap"
          onClick={handleClick}
          title="Tap or click a subject in frame to lock tracking"
        >
          <WebCodecsVideo
            cameraId={camera.id}
            cameraName={camera.name}
            maxFps={maxFps}
            onTransportChange={handleTransportChange}
          />

          <svg className="monitor-overlay" viewBox="0 0 1000 562.5" preserveAspectRatio="none">
            {showGrid && (
              <g stroke="rgba(255,255,255,0.22)" strokeWidth="1" strokeDasharray="6 6">
                <line x1="333.3" y1="0" x2="333.3" y2="562.5" />
                <line x1="666.7" y1="0" x2="666.7" y2="562.5" />
                <line x1="0" y1="187.5" x2="1000" y2="187.5" />
                <line x1="0" y1="375" x2="1000" y2="375" />
              </g>
            )}

            {showGrid && (
              <g>
                <rect
                  x={dz.x * 1000}
                  y={dz.y * 562.5}
                  width={dz.width * 1000}
                  height={dz.height * 562.5}
                  fill="rgba(56, 189, 248, 0.06)"
                  stroke="rgba(56, 189, 248, 0.5)"
                  strokeWidth="1.5"
                  strokeDasharray="5 4"
                />
                <circle
                  cx={anchor[0] * 1000}
                  cy={anchor[1] * 562.5}
                  r="7"
                  fill="none"
                  stroke="#38bdf8"
                  strokeWidth="2"
                />
                <line
                  x1={anchor[0] * 1000 - 14}
                  y1={anchor[1] * 562.5}
                  x2={anchor[0] * 1000 + 14}
                  y2={anchor[1] * 562.5}
                  stroke="#38bdf8"
                  strokeWidth="1.5"
                />
                <line
                  x1={anchor[0] * 1000}
                  y1={anchor[1] * 562.5 - 14}
                  x2={anchor[0] * 1000}
                  y2={anchor[1] * 562.5 + 14}
                  stroke="#38bdf8"
                  strokeWidth="1.5"
                />
              </g>
            )}

            {(telemetry?.subjects ?? []).map((s) => {
              const bx = s.bbox.x * 1000;
              const by = s.bbox.y * 562.5;
              const bw = s.bbox.width * 1000;
              const bh = s.bbox.height * 562.5;
              const stroke = s.is_locked ? "#22c55e" : "rgba(248, 250, 252, 0.72)";

              return (
                <g key={s.track_id}>
                  <rect
                    x={bx}
                    y={by}
                    width={bw}
                    height={bh}
                    fill={s.is_locked ? "rgba(34, 197, 94, 0.1)" : "transparent"}
                    stroke={stroke}
                    strokeWidth={s.is_locked ? "3.5" : "2"}
                    rx="6"
                  />
                  <rect
                    x={bx}
                    y={Math.max(4, by - 26)}
                    width={s.is_locked && s.identity_similarity != null ? "158" : "124"}
                    height="24"
                    rx="4"
                    fill={s.is_locked ? "#16a34a" : "rgba(15,23,42,0.88)"}
                  />
                  <text x={bx + 8} y={Math.max(20, by - 9)} fill="#ffffff" fontSize="13" fontWeight="700">
                    {s.is_locked
                      ? `LOCK #${s.track_id}${s.identity_similarity != null ? ` · ${Math.round(s.identity_similarity * 100)}%` : ""}`
                      : `Subject #${s.track_id}`}
                  </text>

                  {showSkeleton &&
                    s.keypoints.map((kp, idx) => (
                      <circle
                        key={idx}
                        cx={kp.x * 1000}
                        cy={kp.y * 562.5}
                        r={s.is_locked ? "4.5" : "3.5"}
                        fill={s.is_locked ? "#4ade80" : "#38bdf8"}
                      />
                    ))}
                </g>
              );
            })}
          </svg>
        </div>
      </div>

      <div className="monitor-hud-top">
        <div className="hud-badge">
          <span>{camera.name}</span>
          <span className="hide-xs" style={{ color: "#94a3b8", fontWeight: 500 }}>
            {streamLabel}
          </span>
        </div>
        <div className="hud-badge">
          <span
            style={{
              color: transport.mode === "h264" ? "#38bdf8" : "#94a3b8",
              fontWeight: 600,
            }}
            title={
              transport.mode === "h264"
                ? `Zero-latency GPU WebCodecs H.264 (${transport.encoder ?? "HW"})`
                : "MJPEG preview fallback"
            }
          >
            {transport.mode === "h264" ? "H.264 LL" : "MJPEG"}
          </span>
          {telemetry?.ndi_output_active && <span style={{ color: "#38bdf8" }}>NDI OUT</span>}
          <span
            style={{
              color:
                telemetry?.tracking_state === "locked"
                  ? "#4ade80"
                  : telemetry?.tracking_state === "searching"
                    ? "#fcd34d"
                    : "#94a3b8",
            }}
          >
            {(telemetry?.tracking_state ?? "idle").toUpperCase()}
          </span>
        </div>
      </div>

      <div className="monitor-hud-bottom">
        <div className="hud-badge hide-xs">
          <span>Tap any person to lock tracking</span>
        </div>
        <div className="hud-badge">
          <span>AF: {Math.round((telemetry?.focus_sharpness ?? 0.85) * 100)}%</span>
          <span className="hide-xs" style={{ color: "#94a3b8" }}>
            ({telemetry?.focus_state ?? "sharp"})
          </span>
        </div>
      </div>
    </div>
  );
};
