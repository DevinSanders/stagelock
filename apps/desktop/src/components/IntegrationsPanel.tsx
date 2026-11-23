import React, { useEffect, useState } from "react";
import { api } from "../api";
import type { BitfocusButtonsConfig, CameraConfig, StreamDeckStatus, VmixRuntimeState } from "../types";

interface Props {
  selectedCamera: CameraConfig | null;
  onCameraUpdated: (cam: CameraConfig) => void;
}

export const IntegrationsPanel: React.FC<Props> = ({ selectedCamera, onCameraUpdated }) => {
  const [vmix, setVmix] = useState<VmixRuntimeState | null>(null);
  const [vmixHost, setVmixHost] = useState("127.0.0.1");
  const [vmixHttpPort, setVmixHttpPort] = useState(8088);
  const [vmixTcpPort, setVmixTcpPort] = useState(8099);
  const [selectedVmixInput, setSelectedVmixInput] = useState<string>("1");

  const [sd, setSd] = useState<StreamDeckStatus | null>(null);
  const [buttons, setButtons] = useState<BitfocusButtonsConfig>({
    enabled: false,
    base_url: "http://127.0.0.1:8000",
    api_key: "",
  });

  useEffect(() => {
    let active = true;
    const poll = async () => {
      try {
        const [v, s, b] = await Promise.all([
          api.getVmixState(),
          api.getStreamDeckStatus(),
          api.getButtonsConfig(),
        ]);
        if (!active) return;
        setVmix(v);
        setSd(s);
        setButtons(b);
        setVmixHost((prev) => (prev === "127.0.0.1" && v.host ? v.host : prev));
        setVmixHttpPort(v.http_port || 8088);
        setVmixTcpPort(v.tcp_port || 8099);
      } catch {
        // ignore transient poll error
      }
    };
    void poll();
    const id = setInterval(() => {
      void api.getVmixState().then((v) => active && setVmix(v)).catch(() => {});
    }, 2000);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, []);

  const saveVmix = async (enabled: boolean) => {
    const next = await api.updateVmixConfig({
      enabled,
      host: vmixHost,
      http_port: vmixHttpPort,
      tcp_port: vmixTcpPort,
    });
    setVmix(next);
  };

  const runVmix = async (fn: string, input?: string, duration_ms?: number) => {
    try {
      const next = await api.sendVmixCommand({ function: fn, input, duration_ms });
      setVmix(next);
    } catch {
      // error reflected in vmix state
    }
  };

  return (
    <>
      {/* vMix Live Switcher & Tally */}
      <div className="section">
        <div className="section-title">
          <span>vMix Live Switcher &amp; Tally</span>
          <button
            className={`btn small ${vmix?.enabled ? "track-on" : ""}`}
            onClick={() => void saveVmix(!vmix?.enabled)}
          >
            {vmix?.enabled ? (vmix.connected ? "Connected" : "Connecting...") : "Enable vMix"}
          </button>
        </div>

        <div style={{ display: "grid", gridTemplateColumns: "2fr 1fr 1fr", gap: 6 }}>
          <input type="text" value={vmixHost} onChange={(e) => setVmixHost(e.target.value)} placeholder="127.0.0.1" />
          <input
            type="number"
            value={vmixHttpPort}
            onChange={(e) => setVmixHttpPort(Number(e.target.value))}
            title="HTTP Port"
          />
          <input
            type="number"
            value={vmixTcpPort}
            onChange={(e) => setVmixTcpPort(Number(e.target.value))}
            title="TCP Tally Port"
          />
        </div>

        {vmix?.enabled && (
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap", alignItems: "center" }}>
            <button className="btn small" onClick={() => void saveVmix(true)}>
              Apply / Reconnect
            </button>
            {vmix.version && <span className="pill ok">vMix {vmix.version}</span>}
            {vmix.recording && <span className="pill warn">REC</span>}
            {vmix.streaming && <span className="pill ndi">LIVE STREAM</span>}
            {vmix.last_error && !vmix.connected && (
              <span style={{ fontSize: 11, color: "#fca5a5" }}>{vmix.last_error}</span>
            )}
          </div>
        )}

        {/* Map selected camera to vMix input for Red/Green tally sync */}
        {selectedCamera && (
          <div className="field">
            <label>Map &ldquo;{selectedCamera.name}&rdquo; to vMix Input (for Tally Sync)</label>
            <div style={{ display: "flex", gap: 6 }}>
              <input
                type="text"
                placeholder="Input # (e.g. 1) or vMix Title"
                value={selectedCamera.vmix?.input_ref ?? ""}
                onChange={async (e) => {
                  const val = e.target.value.trim() || null;
                  const updated = await api.updateCameraVmixBinding(selectedCamera.id, val);
                  onCameraUpdated(updated);
                }}
              />
            </div>
          </div>
        )}

        {/* Quick vMix Switching Bus */}
        <div className="btn-grid-3">
          <button className="btn danger" onClick={() => void runVmix("Cut", selectedVmixInput)}>
            CUT
          </button>
          <button className="btn primary" onClick={() => void runVmix("Fade", selectedVmixInput, 500)}>
            FADE 500ms
          </button>
          <button className="btn" onClick={() => void runVmix("Stinger1", selectedVmixInput)}>
            STINGER 1
          </button>
        </div>

        {vmix && vmix.inputs.length > 0 ? (
          <div className="btn-grid-2">
            {vmix.inputs.slice(0, 8).map((inp) => {
              const cls =
                inp.tally === "program"
                  ? "danger"
                  : inp.tally === "preview"
                    ? "track-on"
                    : selectedVmixInput === String(inp.number)
                      ? "active"
                      : "";
              return (
                <button
                  key={inp.key || inp.number}
                  className={`btn small ${cls}`}
                  onClick={() => {
                    setSelectedVmixInput(String(inp.number));
                    void runVmix("PreviewInput", String(inp.number));
                  }}
                >
                  #{inp.number} {inp.short_title || inp.title}
                </button>
              );
            })}
          </div>
        ) : (
          <div className="field">
            <label>Manual Target vMix Input (# or Title)</label>
            <input
              type="text"
              value={selectedVmixInput}
              onChange={(e) => setSelectedVmixInput(e.target.value)}
              placeholder="1"
            />
          </div>
        )}
      </div>

      {/* Stream Deck + Bitfocus Companion / Buttons */}
      <div className="section">
        <div className="section-title">
          <span>Stream Deck + Bitfocus Companion / Buttons</span>
          <button
            className={`btn small ${sd?.direct_hid_enabled ? "active" : ""}`}
            onClick={async () => {
              const updated = await api.updateStreamDeckConfig({
                direct_hid_enabled: !sd?.direct_hid_enabled,
                brightness: sd?.brightness ?? 80,
              });
              setSd(updated);
            }}
          >
            {sd?.direct_hid_enabled ? "Direct USB: ON" : "Direct USB: OFF"}
          </button>
        </div>

        <div style={{ fontSize: 11.5, color: "#94a3b8", lineHeight: 1.45 }}>
          {sd?.mode_note ?? "Loading Stream Deck status..."}
          <div style={{ marginTop: 6 }}>
            <strong>Bitfocus Module:</strong> <code>integrations/companion-module</code> · <strong>REST/WS:</strong>{" "}
            <code>:8765</code> · <strong>OSC UDP:</strong> <code>:9000</code>
          </div>
        </div>

        {sd && (
          <div className="btn-grid-2">
            {sd.key_bindings.slice(0, 6).map((b) => (
              <button
                key={b.slot}
                className="btn small"
                onClick={() => void api.triggerStreamDeckAction(b.action)}
                title={`Simulate ${b.slot}`}
              >
                {b.slot}: {b.label}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Optional Bitfocus Buttons Control API */}
      <div className="section">
        <div className="section-title">
          <span>Bitfocus Buttons Control API</span>
          <button
            className={`btn small ${buttons.enabled ? "track-on" : ""}`}
            onClick={async () => {
              const saved = await api.updateButtonsConfig({ ...buttons, enabled: !buttons.enabled });
              setButtons(saved);
            }}
          >
            {buttons.enabled ? "Enabled" : "Disabled"}
          </button>
        </div>
        <div className="field">
          <label>Buttons Server URL</label>
          <input
            type="text"
            value={buttons.base_url}
            onChange={(e) => setButtons({ ...buttons, base_url: e.target.value })}
          />
        </div>
        <div className="field">
          <label>Scoped Bearer API Key</label>
          <input
            type="password"
            value={buttons.api_key}
            onChange={(e) => setButtons({ ...buttons, api_key: e.target.value })}
            placeholder="Bearer token from Bitfocus Buttons"
          />
        </div>
        <button className="btn small" onClick={() => void api.updateButtonsConfig(buttons)}>
          Save Buttons API Settings
        </button>
      </div>
    </>
  );
};
