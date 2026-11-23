import React, { useEffect, useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import { api } from "../api";
import type { RemoteInfo } from "../types";

interface Props {
  onClose: () => void;
}

export const RemoteAccessModal: React.FC<Props> = ({ onClose }) => {
  const [info, setInfo] = useState<RemoteInfo | null>(null);
  const [selectedUrl, setSelectedUrl] = useState<string>("");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    void api.getRemoteInfo().then((res) => {
      setInfo(res);
      setSelectedUrl(res.lan_urls[0] ?? res.local_url);
    });
  }, []);

  const copyUrl = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // clipboard unavailable
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <h3 style={{ margin: 0, fontSize: 15 }}>Remote Control PWA (iPhone / iPad / Android)</h3>
          <button type="button" className="btn small" onClick={onClose}>
            Close
          </button>
        </div>

        {!info ? (
          <div style={{ color: "#94a3b8", padding: "16px 0" }}>Detecting local network addresses...</div>
        ) : (
          <>
            <div style={{ display: "flex", gap: 18, alignItems: "center", flexWrap: "wrap", justifyContent: "center" }}>
              <div
                style={{
                  background: "#ffffff",
                  padding: 12,
                  borderRadius: 12,
                  display: "inline-flex",
                  boxShadow: "0 8px 24px rgba(0,0,0,0.45)",
                }}
              >
                <QRCodeSVG value={selectedUrl || info.local_url} size={172} level="M" />
              </div>

              <div style={{ flex: 1, minWidth: 220, display: "flex", flexDirection: "column", gap: 10 }}>
                <div style={{ fontSize: 12, color: "#cbd5e1", lineHeight: 1.5 }}>
                  Scan with your phone or tablet camera on the <strong>same Wi-Fi / LAN</strong> to open live preview &amp; touch PTZ control.
                </div>

                {info.lan_urls.length > 1 && (
                  <div className="field">
                    <label>Select Network Interface</label>
                    <select value={selectedUrl} onChange={(e) => setSelectedUrl(e.target.value)}>
                      {info.lan_urls.map((u) => (
                        <option key={u} value={u}>
                          {u}
                        </option>
                      ))}
                    </select>
                  </div>
                )}

                <div style={{ display: "flex", gap: 8 }}>
                  <input type="text" readOnly value={selectedUrl} />
                  <button type="button" className="btn primary" onClick={() => void copyUrl(selectedUrl)}>
                    {copied ? "Copied" : "Copy"}
                  </button>
                </div>

                {!info.pwa_ready && (
                  <div style={{ fontSize: 11.5, color: "#fcd34d" }}>
                    Built PWA assets were not detected yet. Run <code>pnpm build:ui</code> and restart the daemon so port {info.http_port} serves the mobile web app directly.
                  </div>
                )}
              </div>
            </div>

            <div className="section" style={{ gap: 6 }}>
              <div className="section-title">Install as Full-Screen App (PWA)</div>
              <div style={{ fontSize: 12, color: "#cbd5e1", lineHeight: 1.55 }}>
                <div>
                  <strong>iPhone / iPad (Safari):</strong> Open the URL → tap <strong>Share</strong> → <strong>Add to Home Screen</strong>.
                </div>
                <div>
                  <strong>Android (Chrome):</strong> Open the URL → tap <strong>⋮ Menu</strong> → <strong>Add to Home screen / Install app</strong>.
                </div>
              </div>
            </div>

            <div className="section" style={{ gap: 6 }}>
              <div className="section-title">Windows Firewall Note (If Phone Cannot Reach PC)</div>
              <div style={{ fontSize: 11.5, color: "#94a3b8", lineHeight: 1.45 }}>
                If your phone times out on LAN, allow TCP port <strong>{info.http_port}</strong> in an Administrator PowerShell:
              </div>
              <code
                style={{
                  fontSize: 11,
                  background: "#0a0d12",
                  border: "1px solid #283347",
                  borderRadius: 6,
                  padding: "7px 9px",
                  color: "#7dd3fc",
                  userSelect: "text",
                  wordBreak: "break-all",
                }}
              >
                New-NetFirewallRule -DisplayName &quot;VideoController Remote&quot; -Direction Inbound -Protocol TCP -LocalPort {info.http_port} -Action Allow
              </code>
            </div>
          </>
        )}
      </div>
    </div>
  );
};
