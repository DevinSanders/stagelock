import React, { useEffect, useState } from "react";
import { api } from "../api";
import type {
  CameraConfig,
  CaptureDeviceInfo,
  NdiDiscoveredSource,
  OnvifDiscoveredDevice,
  PtzConfig,
  VideoSourceConfig,
} from "../types";

interface Props {
  onClose: () => void;
  onCreate: (cam: CameraConfig) => Promise<void>;
}

export const AddCameraModal: React.FC<Props> = ({ onClose, onCreate }) => {
  const [name, setName] = useState("Camera 3");
  const [sourceKind, setSourceKind] = useState<string>("ndi");

  // NDI discovery
  const [ndiSources, setNdiSources] = useState<NdiDiscoveredSource[]>([]);
  const [ndiScanning, setNdiScanning] = useState(false);
  const [ndiName, setNdiName] = useState("");

  // USB / Capture discovery
  const [usbDevices, setUsbDevices] = useState<CaptureDeviceInfo[]>([]);
  const [usbScanning, setUsbScanning] = useState(false);
  const [usbId, setUsbId] = useState("");

  // ONVIF discovery & resolution
  const [onvifDevices, setOnvifDevices] = useState<OnvifDiscoveredDevice[]>([]);
  const [onvifScanning, setOnvifScanning] = useState(false);
  const [onvifHost, setOnvifHost] = useState("192.168.1.50");
  const [onvifPort, setOnvifPort] = useState(80);
  const [onvifUser, setOnvifUser] = useState("admin");
  const [onvifPass, setOnvifPass] = useState("");
  const [onvifProfileToken, setOnvifProfileToken] = useState<string | null>(null);
  const [onvifStatus, setOnvifStatus] = useState<string | null>(null);

  const [rtspUrl, setRtspUrl] = useState("rtsp://192.168.1.50:554/stream1");
  const [whepUrl, setWhepUrl] = useState("http://127.0.0.1:8889/cam1/whep");
  const [deckLinkIdx, setDeckLinkIdx] = useState(0);

  const [ptzKind, setPtzKind] = useState<string>("ndi");
  const [ptzHost, setPtzHost] = useState("192.168.1.50");
  const [ptzPort, setPtzPort] = useState(5678);
  const [ptzUser, setPtzUser] = useState("admin");
  const [ptzPass, setPtzPass] = useState("");

  const scanNdi = async () => {
    setNdiScanning(true);
    try {
      const res = await api.discoverNdi();
      setNdiSources(res.sources);
      if (res.sources.length > 0 && !ndiName) {
        setNdiName(res.sources[0].name);
      }
    } catch {
      setNdiSources([]);
    } finally {
      setNdiScanning(false);
    }
  };

  const scanUsb = async () => {
    setUsbScanning(true);
    try {
      const res = await api.discoverUsb();
      setUsbDevices(res.devices);
      if (res.devices.length > 0) {
        setUsbId(res.devices[0].device_id);
        setName(res.devices[0].name);
      }
    } catch {
      setUsbDevices([]);
    } finally {
      setUsbScanning(false);
    }
  };

  const scanOnvif = async () => {
    setOnvifScanning(true);
    setOnvifStatus("Scanning LAN via WS-Discovery...");
    try {
      const res = await api.discoverOnvif();
      setOnvifDevices(res.devices);
      if (res.devices.length > 0) {
        const first = res.devices[0];
        setOnvifHost(first.host);
        setOnvifPort(first.port);
        setPtzHost(first.host);
        setPtzPort(first.port);
        setName(first.name);
        setOnvifStatus(`Found ${res.devices.length} ONVIF camera(s)`);
      } else {
        setOnvifStatus("No ONVIF cameras responded on LAN (enter IP manually below)");
      }
    } catch {
      setOnvifStatus("ONVIF LAN scan failed");
    } finally {
      setOnvifScanning(false);
    }
  };

  const resolveOnvif = async () => {
    setOnvifStatus("Querying ONVIF GetProfiles & GetStreamUri...");
    try {
      const info = await api.resolveOnvif({
        host: onvifHost,
        port: onvifPort,
        username: onvifUser,
        password: onvifPass,
      });
      setOnvifProfileToken(info.profile_token);
      if (info.rtsp_uri) setRtspUrl(info.rtsp_uri);
      setPtzKind("onvif");
      setPtzHost(onvifHost);
      setPtzPort(onvifPort);
      setPtzUser(onvifUser);
      setPtzPass(onvifPass);
      setOnvifStatus(`Resolved profile '${info.profile_token}'${info.rtsp_uri ? ` → ${info.rtsp_uri}` : ""}`);
    } catch (err) {
      setOnvifStatus(`Resolve failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  };

  useEffect(() => {
    if (sourceKind === "ndi") {
      setPtzKind("ndi");
      void scanNdi();
    } else if (sourceKind === "usb") {
      setPtzKind("none");
      void scanUsb();
    } else if (sourceKind === "onvif") {
      setPtzKind("onvif");
      setPtzPort(80);
      void scanOnvif();
    }
  }, [sourceKind]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    let source: VideoSourceConfig;
    switch (sourceKind) {
      case "ndi":
        source = { kind: "ndi", source_name: ndiName || "NDI Source", low_bandwidth: false };
        break;
      case "onvif":
        source = {
          kind: "onvif",
          host: onvifHost,
          port: onvifPort,
          username: onvifUser,
          password: onvifPass,
          resolved_rtsp_url: rtspUrl || null,
        };
        break;
      case "rtsp":
        source = { kind: "rtsp", url: rtspUrl, latency_ms: 80, prefer_tcp: true };
        break;
      case "web_rtc":
        source = { kind: "web_rtc", whep_url: whepUrl };
        break;
      case "usb": {
        const picked = usbDevices.find((d) => d.device_id === usbId);
        source = {
          kind: "usb",
          device_id: usbId,
          device_name: picked?.name ?? name,
          width: Math.min(picked?.max_width ?? 1280, 1920),
          height: Math.min(picked?.max_height ?? 720, 1080),
          fps: 30,
        };
        break;
      }
      case "deck_link":
        source = { kind: "deck_link", device_index: deckLinkIdx, connection: "auto", mode: "auto" };
        break;
      default:
        source = { kind: "test_pattern", pattern: "smpte", width: 1280, height: 720, fps: 30 };
        break;
    }

    let ptz: PtzConfig;
    switch (ptzKind) {
      case "ndi":
        ptz = { protocol: "ndi" };
        break;
      case "visca_ip_sony":
        ptz = { protocol: "visca_ip_sony", host: ptzHost, port: ptzPort };
        break;
      case "visca_ip_raw_tcp":
        ptz = { protocol: "visca_ip_raw_tcp", host: ptzHost, port: ptzPort };
        break;
      case "visca_ip_raw_udp":
        ptz = { protocol: "visca_ip_raw_udp", host: ptzHost, port: ptzPort };
        break;
      case "onvif":
        ptz = {
          protocol: "onvif",
          host: ptzHost,
          port: ptzPort,
          username: ptzUser,
          password: ptzPass,
          profile_token: onvifProfileToken,
        };
        break;
      case "canon_xc":
        ptz = { protocol: "canon_xc", host: ptzHost, port: ptzPort, username: ptzUser, password: ptzPass };
        break;
      case "panasonic_aw":
        ptz = { protocol: "panasonic_aw", host: ptzHost, port: ptzPort };
        break;
      case "mock":
        ptz = { protocol: "mock" };
        break;
      default:
        ptz = { protocol: "none" };
    }

    const newCam: CameraConfig = {
      id: "00000000-0000-0000-0000-000000000000",
      name,
      enabled: true,
      source,
      ptz,
      ndi_output: {
        enabled: false,
        stream_name: `VC - ${name}`,
        include_color_adjustments: true,
      },
      framing: {
        auto_track_enabled: false,
        preset: "waist_up",
        rule_of_thirds: "center",
        auto_focus_enabled: true,
        focus_strategy: "one_push_af",
        deadzone_x: 0.08,
        deadzone_y: 0.06,
        deadzone_zoom: 0.12,
        max_speed: 0.65,
        tally_damping_enabled: true,
      },
      color: {
        white_balance_temp: 0,
        white_balance_tint: 0,
        exposure: 0,
        contrast: 1,
        saturation: 1,
        sharpness: 0,
        brightness: 0,
      },
      vmix: {},
      presets: [
        { slot: 1, name: "Wide" },
        { slot: 2, name: "Center" },
        { slot: 3, name: "Left" },
        { slot: 4, name: "Right" },
      ],
    };

    await onCreate(newCam);
    onClose();
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <form className="modal" onClick={(e) => e.stopPropagation()} onSubmit={handleSubmit}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <h3 style={{ margin: 0, fontSize: 15 }}>Add Video Source / PTZ Camera</h3>
          <button type="button" className="btn small" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="field">
          <label>Camera Name</label>
          <input type="text" value={name} onChange={(e) => setName(e.target.value)} required />
        </div>

        <div className="field">
          <label>Video Source Protocol</label>
          <select value={sourceKind} onChange={(e) => setSourceKind(e.target.value)}>
            <option value="ndi">NDI® / NDI|HX Network Source</option>
            <option value="onvif">ONVIF Profile S/T (Auto-Discover + RTSP)</option>
            <option value="rtsp">RTSP / RTP IP Camera</option>
            <option value="usb">USB (UVC) / HDMI Dongle / Virtual Capture (vMix)</option>
            <option value="deck_link">Blackmagic DeckLink (SDI / HDMI)</option>
            <option value="web_rtc">WebRTC (WHEP) Stream</option>
            <option value="test_pattern">Synthetic Stage Test Pattern (Demo)</option>
          </select>
        </div>

        {sourceKind === "ndi" && (
          <div className="field">
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <label>NDI® Source</label>
              <button type="button" className="btn small" onClick={scanNdi} disabled={ndiScanning}>
                {ndiScanning ? "Scanning LAN..." : "Rescan NDI"}
              </button>
            </div>
            {ndiSources.length > 0 ? (
              <select
                value={ndiName}
                onChange={(e) => {
                  setNdiName(e.target.value);
                  setName(e.target.value);
                }}
              >
                {ndiSources.map((s) => (
                  <option key={s.name} value={s.name}>
                    {s.name} ({s.url_address})
                  </option>
                ))}
              </select>
            ) : (
              <input
                type="text"
                placeholder="MACHINE (NDI Stream Name)"
                value={ndiName}
                onChange={(e) => setNdiName(e.target.value)}
              />
            )}
          </div>
        )}

        {sourceKind === "onvif" && (
          <div className="section" style={{ gap: 8 }}>
            <div className="section-title">
              <span>ONVIF LAN Discovery &amp; Stream Resolver</span>
              <button type="button" className="btn small" onClick={scanOnvif} disabled={onvifScanning}>
                {onvifScanning ? "Scanning..." : "Scan ONVIF"}
              </button>
            </div>
            {onvifDevices.length > 0 && (
              <select
                value={`${onvifHost}:${onvifPort}`}
                onChange={(e) => {
                  const picked = onvifDevices.find((d) => `${d.host}:${d.port}` === e.target.value);
                  if (picked) {
                    setOnvifHost(picked.host);
                    setOnvifPort(picked.port);
                    setPtzHost(picked.host);
                    setPtzPort(picked.port);
                    setName(picked.name);
                  }
                }}
              >
                {onvifDevices.map((d) => (
                  <option key={d.xaddr} value={`${d.host}:${d.port}`}>
                    {d.name} ({d.host}:{d.port})
                  </option>
                ))}
              </select>
            )}
            <div style={{ display: "grid", gridTemplateColumns: "2fr 1fr", gap: 8 }}>
              <input type="text" placeholder="IP / Host" value={onvifHost} onChange={(e) => setOnvifHost(e.target.value)} />
              <input type="number" placeholder="Port" value={onvifPort} onChange={(e) => setOnvifPort(Number(e.target.value))} />
            </div>
            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
              <input type="text" placeholder="Username" value={onvifUser} onChange={(e) => setOnvifUser(e.target.value)} />
              <input type="password" placeholder="Password" value={onvifPass} onChange={(e) => setOnvifPass(e.target.value)} />
            </div>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <button type="button" className="btn small active" onClick={resolveOnvif}>
                Resolve RTSP &amp; Profile
              </button>
              {onvifStatus && <span style={{ fontSize: 11, color: "#94a3b8" }}>{onvifStatus}</span>}
            </div>
            <input
              type="text"
              placeholder="Resolved RTSP URL"
              value={rtspUrl}
              onChange={(e) => setRtspUrl(e.target.value)}
            />
          </div>
        )}

        {sourceKind === "rtsp" && (
          <div className="field">
            <label>RTSP Stream URL</label>
            <input type="text" value={rtspUrl} onChange={(e) => setRtspUrl(e.target.value)} />
          </div>
        )}

        {sourceKind === "web_rtc" && (
          <div className="field">
            <label>WebRTC WHEP Endpoint URL</label>
            <input type="text" value={whepUrl} onChange={(e) => setWhepUrl(e.target.value)} />
          </div>
        )}

        {sourceKind === "usb" && (
          <div className="field">
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <label>Local Capture / USB Device</label>
              <button type="button" className="btn small" onClick={scanUsb} disabled={usbScanning}>
                {usbScanning ? "Scanning..." : "Rescan Capture"}
              </button>
            </div>
            {usbDevices.length > 0 ? (
              <select
                value={usbId}
                onChange={(e) => {
                  setUsbId(e.target.value);
                  const d = usbDevices.find((x) => x.device_id === e.target.value);
                  if (d) setName(d.name);
                }}
              >
                {usbDevices.map((d) => (
                  <option key={d.device_id} value={d.device_id}>
                    {d.name} ({d.max_width}×{d.max_height} · {d.api})
                  </option>
                ))}
              </select>
            ) : (
              <input
                type="text"
                placeholder="Device Path or 0 for default webcam"
                value={usbId}
                onChange={(e) => setUsbId(e.target.value)}
              />
            )}
          </div>
        )}

        {sourceKind === "deck_link" && (
          <div className="field">
            <label>DeckLink Device Index</label>
            <input
              type="number"
              min={0}
              value={deckLinkIdx}
              onChange={(e) => setDeckLinkIdx(Number(e.target.value))}
            />
          </div>
        )}

        <div className="field">
          <label>PTZ Control Protocol</label>
          <select
            value={ptzKind}
            onChange={(e) => {
              const v = e.target.value;
              setPtzKind(v);
              if (v === "visca_ip_sony") setPtzPort(52381);
              if (v === "visca_ip_raw_tcp") setPtzPort(5678);
              if (v === "visca_ip_raw_udp") setPtzPort(1259);
              if (v === "onvif" || v === "canon_xc" || v === "panasonic_aw") setPtzPort(80);
            }}
          >
            <option value="visca_ip_raw_tcp">VISCA over IP — Raw TCP (PTZOptics / BirdDog / Marshall)</option>
            <option value="visca_ip_sony">VISCA over IP — Sony UDP (Port 52381)</option>
            <option value="visca_ip_raw_udp">VISCA over IP — Raw UDP (Port 1259)</option>
            <option value="ndi">NDI® Native PTZ</option>
            <option value="onvif">ONVIF Profile S/T (PTZ + Imaging)</option>
            <option value="canon_xc">Canon XC Protocol (CR-N100/300/500/700)</option>
            <option value="panasonic_aw">Panasonic AW Protocol (AW-UE / AW-HE)</option>
            <option value="mock">Simulated Mock PTZ</option>
            <option value="none">None (Fixed Camera)</option>
          </select>
        </div>

        {(ptzKind.startsWith("visca") || ptzKind === "onvif" || ptzKind === "canon_xc" || ptzKind === "panasonic_aw") && (
          <>
            <div style={{ display: "grid", gridTemplateColumns: "2fr 1fr", gap: 10 }}>
              <div className="field">
                <label>Camera IP / Host</label>
                <input type="text" value={ptzHost} onChange={(e) => setPtzHost(e.target.value)} />
              </div>
              <div className="field">
                <label>Port</label>
                <input type="number" value={ptzPort} onChange={(e) => setPtzPort(Number(e.target.value))} />
              </div>
            </div>
            {(ptzKind === "onvif" || ptzKind === "canon_xc") && (
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10 }}>
                <div className="field">
                  <label>Username</label>
                  <input type="text" value={ptzUser} onChange={(e) => setPtzUser(e.target.value)} />
                </div>
                <div className="field">
                  <label>Password</label>
                  <input type="password" value={ptzPass} onChange={(e) => setPtzPass(e.target.value)} />
                </div>
              </div>
            )}
          </>
        )}

        <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 4 }}>
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn primary">
            Add Camera
          </button>
        </div>
      </form>
    </div>
  );
};
