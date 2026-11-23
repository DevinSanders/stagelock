import React, { useRef, useState } from "react";
import type { CameraPreset } from "../types";

interface Props {
  presets: CameraPreset[];
  onMove: (pan: number, tilt: number, zoom: number) => void;
  onStop: () => void;
  onPreset: (action: "recall" | "store", slot: number) => void;
}

export const PtzJoystick: React.FC<Props> = ({ presets, onMove, onStop, onPreset }) => {
  const padRef = useRef<HTMLDivElement>(null);
  const [knob, setKnob] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const [storeMode, setStoreMode] = useState(false);

  const updateFromPointer = (clientX: number, clientY: number) => {
    const el = padRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const cx = rect.left + rect.width / 2;
    const cy = rect.top + rect.height / 2;
    const maxR = rect.width / 2 - 20;
    let dx = clientX - cx;
    let dy = clientY - cy;
    const dist = Math.hypot(dx, dy);
    if (dist > maxR) {
      dx = (dx / dist) * maxR;
      dy = (dy / dist) * maxR;
    }
    setKnob({ x: dx, y: dy });
    const pan = Number((dx / maxR).toFixed(2));
    const tilt = Number((-dy / maxR).toFixed(2));
    onMove(pan, tilt, 0);
  };

  const handlePointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDragging(true);
    updateFromPointer(e.clientX, e.clientY);
  };

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    e.preventDefault();
    updateFromPointer(e.clientX, e.clientY);
  };

  const handlePointerUp = () => {
    setDragging(false);
    setKnob({ x: 0, y: 0 });
    onStop();
  };

  const preventContext = (e: React.MouseEvent) => e.preventDefault();

  return (
    <>
      <div className="section">
        <div className="section-title">
          <span>Pan / Tilt / Zoom</span>
          <button className="btn small danger" onClick={onStop}>
            Stop All
          </button>
        </div>

        <div className="joystick-wrap">
          <div
            ref={padRef}
            className="joystick-pad"
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
            onPointerCancel={handlePointerUp}
            onContextMenu={preventContext}
          >
            <div
              className="joystick-knob"
              style={{ transform: `translate(calc(-50% + ${knob.x}px), calc(-50% + ${knob.y}px))` }}
            />
          </div>

          <div className="zoom-col">
            <button
              className="btn touch-hold"
              onContextMenu={preventContext}
              onPointerDown={(e) => {
                e.preventDefault();
                e.currentTarget.setPointerCapture(e.pointerId);
                onMove(0, 0, 0.55);
              }}
              onPointerUp={onStop}
              onPointerCancel={onStop}
            >
              Zoom In (+)
            </button>
            <button
              className="btn touch-hold"
              onContextMenu={preventContext}
              onPointerDown={(e) => {
                e.preventDefault();
                e.currentTarget.setPointerCapture(e.pointerId);
                onMove(0, 0, -0.55);
              }}
              onPointerUp={onStop}
              onPointerCancel={onStop}
            >
              Zoom Out (-)
            </button>
            <div style={{ fontSize: 11, color: "#64748b", textAlign: "center" }}>
              Touch &amp; hold to zoom
            </div>
          </div>
        </div>
      </div>

      <div className="section">
        <div className="section-title">
          <span>Camera Presets</span>
          <button
            className={`btn small ${storeMode ? "danger" : ""}`}
            onClick={() => setStoreMode((s) => !s)}
          >
            {storeMode ? "Mode: STORE" : "Mode: Recall"}
          </button>
        </div>
        <div className="btn-grid-2">
          {presets.map((p) => (
            <button
              key={p.slot}
              className="btn"
              onClick={() => {
                onPreset(storeMode ? "store" : "recall", p.slot);
                if (storeMode) setStoreMode(false);
              }}
            >
              #{p.slot} {p.name}
            </button>
          ))}
        </div>
      </div>
    </>
  );
};
