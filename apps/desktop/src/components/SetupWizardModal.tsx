import React, { useEffect, useState } from "react";
import { api } from "../api";
import type {
  CaptureDeviceInfo,
  NdiDiscoveredSource,
  OnvifDiscoveredDevice,
  RemoteInfo,
  SystemCapabilities,
  VisionEngineStatus,
} from "../types";

interface Props {
  capabilities: SystemCapabilities | null;
  visionStatus: VisionEngineStatus | null;
  onClose: () => void;
  onOpenAddCamera: () => void;
}

export const SetupWizardModal: React.FC<Props> = ({
  capabilities,
  visionStatus,
  onClose,
  onOpenAddCamera,
}) => {
  const [remote, setRemote] = useState<RemoteInfo | null>(null);
  const [scanning, setScanning] = useState(false);
  const [ndiFound, setNdiFound] = useState<NdiDiscoveredSource[] | null>(null);
  const [onvifFound, setOnvifFound] = useState<OnvifDiscoveredDevice[] | null>(null);
  const [usbFound, setUsbFound] = useState<CaptureDeviceInfo[] | null>(null);

  useEffect(() => {
    void api.getRemoteInfo().then(setRemote).catch(() => {});
  }, []);

  const runFullDiscovery = async () => {
    setScanning(true);
    try {
      const [ndi, onvif, usb] = await Promise.all([
        api.discoverNdi().catch(() => ({ available: false, sources: [] })),
        api.discoverOnvif().catch(() => ({ devices: [] })),
        api.discoverUsb().catch(() => ({ devices: [] })),
      ]);
      setNdiFound(ndi.sources);
      setOnvifFound(onvif.devices);
      setUsbFound(usb.devices);
    } finally {
      setScanning(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" style={{ width: 620 }} onClick={(e) => e.stopPropagation()}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <h3 style={{ margin: 0, fontSize: 16 }}>System Readiness, Discovery &amp; About</h3>
          <button type="button" className="btn small" onClick={onClose}>
            Close
          </button>
        </div>

        {/* 1. Hardware & Media Engine Diagnostics */}
        <div className="section" style={{ gap: 8 }}>
          <div className="section-title">
            <span>1. GPU &amp; Broadcast Runtime Checklist</span>
            <span className="pill ok">{capabilities?.os ?? "..."} / {capabilities?.arch ?? "..."}</span>
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, fontSize: 12 }}>
            <div className="pill ok" style={{ justifyContent: "space-between", borderRadius: 8, padding: "7px 10px" }}>
              <span>GPU</span>
              <strong>{capabilities?.gpu_names?.[0] ?? "CPU Fallback"}</strong>
            </div>
            <div className="pill ok" style={{ justifyContent: "space-between", borderRadius: 8, padding: "7px 10px" }}>
              <span>Video HW</span>
              <strong>{(capabilities?.decode_backend ?? "cpu").toUpperCase()}</strong>
            </div>
            <div
              className={`pill ${capabilities?.gstreamer_available ? "ok" : "warn"}`}
              style={{ justifyContent: "space-between", borderRadius: 8, padding: "7px 10px" }}
            >
              <span>GStreamer</span>
              <strong>{capabilities?.gstreamer_version ?? "Not Found"}</strong>
            </div>
            <div
              className={`pill ${capabilities?.ndi_runtime_available ? "ndi" : "warn"}`}
              style={{ justifyContent: "space-between", borderRadius: 8, padding: "7px 10px" }}
            >
              <span>NDI® 6 Runtime</span>
              <strong>{capabilities?.ndi_runtime_available ? "Loaded" : "Missing"}</strong>
            </div>
          </div>

          {!capabilities?.ndi_runtime_available && (
            <div className="ndi-footer">
              Install the free NDI® Runtime / Tools from{" "}
              <a href="https://ndi.video/tools" target="_blank" rel="noreferrer">
                ndi.video/tools
              </a>{" "}
              to enable NDI ingest, PTZ, and NDI re-broadcasting.
            </div>
          )}
        </div>

        {/* 2. Permissive Open-Source AI Vision Models */}
        <div className="section" style={{ gap: 8 }}>
          <div className="section-title">
            <span>2. AI Vision Models (Apache-2.0 / MIT)</span>
            <span style={{ color: "#38bdf8", fontSize: 10.5 }}>
              {visionStatus?.active_execution_provider ?? "Checking..."}
            </span>
          </div>
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
            {(visionStatus?.models ?? []).map((m) => (
              <span key={m.filename} className={`pill ${m.installed ? "ok" : ""}`}>
                {m.filename}: {m.installed ? "Installed" : "Fallback Mode"} ({m.license})
              </span>
            ))}
          </div>
          <div style={{ fontSize: 11.5, color: "#94a3b8" }}>
            Run <code>.\scripts\download-models.ps1</code> to fetch permissive RTMDet-s &amp; RTMPose-s ONNX weights into{" "}
            <code>models/</code>.
          </div>
        </div>

        {/* 3. Simultaneous Broadcast Discovery */}
        <div className="section" style={{ gap: 8 }}>
          <div className="section-title">
            <span>3. One-Click Studio Source Discovery</span>
            <button type="button" className="btn small primary" disabled={scanning} onClick={runFullDiscovery}>
              {scanning ? "Scanning LAN & USB..." : "Scan NDI + ONVIF + USB"}
            </button>
          </div>
          {ndiFound !== null && (
            <div style={{ fontSize: 12, color: "#cbd5e1", lineHeight: 1.55 }}>
              <div>
                <strong>NDI® LAN Sources:</strong> {ndiFound.length} found
                {ndiFound.length > 0 ? ` (${ndiFound.slice(0, 3).map((s) => s.name).join(", ")})` : ""}
              </div>
              <div>
                <strong>ONVIF IP Cameras:</strong> {onvifFound?.length ?? 0} found
              </div>
              <div>
                <strong>Local USB / Virtual Capture:</strong> {usbFound?.length ?? 0} found
                {usbFound && usbFound.length > 0
                  ? ` (${usbFound.slice(0, 3).map((d) => d.name).join(", ")}${usbFound.length > 3 ? ", ..." : ""})`
                  : ""}
              </div>
            </div>
          )}
          <div style={{ display: "flex", gap: 8 }}>
            <button
              type="button"
              className="btn small"
              onClick={() => {
                onClose();
                onOpenAddCamera();
              }}
            >
              + Open Add Camera Wizard
            </button>
            {remote?.lan_urls?.[0] && (
              <span className="pill ok">Remote PWA Ready: {remote.lan_urls[0]}</span>
            )}
          </div>
        </div>

        {/* 4. Licensing & Mandatory NDI Attribution */}
        <div className="section" style={{ gap: 6 }}>
          <div className="section-title">
            <span>About &amp; Trademark Attribution</span>
            <span>MIT / Apache-2.0</span>
          </div>
          <div className="ndi-footer">
            <strong>VideoController v0.1.0</strong> — Cross-platform GPU-accelerated multi-camera AI PTZ controller.
            <br />
            NDI® is a registered trademark of Vizrt NDI AB. Learn more at{" "}
            <a href="https://ndi.video" target="_blank" rel="noreferrer">
              ndi.video
            </a>{" "}
            or download NDI Tools at{" "}
            <a href="https://ndi.video/tools" target="_blank" rel="noreferrer">
              ndi.video/tools
            </a>
            .
          </div>
        </div>
      </div>
    </div>
  );
};
