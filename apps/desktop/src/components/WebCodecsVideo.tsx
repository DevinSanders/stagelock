import React, { useEffect, useRef, useState } from "react";
import { mjpegStreamUrl, videoH264WsUrl } from "../api";

interface Props {
  cameraId: string;
  cameraName: string;
  maxFps?: number;
  onTransportChange?: (mode: "h264" | "mjpeg", encoder?: string) => void;
}

/**
 * Ultra-low-latency GPU WebCodecs H.264 <canvas> video player over binary WebSocket (`/ws/video/{id}`),
 * with transparent automatic fallback to rate-limited MJPEG on browsers or hosts without WebCodecs/H.264.
 */
export const WebCodecsVideo: React.FC<Props> = ({
  cameraId,
  cameraName,
  maxFps,
  onTransportChange,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [useMjpeg, setUseMjpeg] = useState<boolean>(
    () => typeof window === "undefined" || typeof window.VideoDecoder === "undefined"
  );

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.VideoDecoder === "undefined") {
      setUseMjpeg(true);
      onTransportChange?.("mjpeg");
      return;
    }

    setUseMjpeg(false);
    let disposed = false;
    let decoder: VideoDecoder | null = null;
    let configuredCodec: string | null = null;
    let sawKeyframe = false;
    let gotAnyFrame = false;
    let encoderName = "";

    const fallback = () => {
      if (disposed) return;
      setUseMjpeg(true);
      onTransportChange?.("mjpeg");
    };

    // If first H.264 frame does not arrive within 2.5s (e.g., host without H.264 plugin), fall back smoothly
    const startupTimer = window.setTimeout(() => {
      if (!gotAnyFrame) fallback();
    }, 2500);

    const ws = new WebSocket(videoH264WsUrl(cameraId));
    ws.binaryType = "arraybuffer";

    const ensureDecoder = (codec: string, width: number, height: number) => {
      if (configuredCodec === codec && decoder && decoder.state === "configured") {
        return;
      }
      if (decoder) {
        try {
          decoder.close();
        } catch {
          // ignore
        }
      }
      sawKeyframe = false;
      configuredCodec = codec;
      decoder = new VideoDecoder({
        output: (frame) => {
          const canvas = canvasRef.current;
          if (canvas) {
            if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
              canvas.width = frame.displayWidth;
              canvas.height = frame.displayHeight;
            }
            const ctx = canvas.getContext("2d");
            if (ctx) {
              ctx.drawImage(frame, 0, 0, canvas.width, canvas.height);
            }
          }
          frame.close();
          if (!gotAnyFrame) {
            gotAnyFrame = true;
            window.clearTimeout(startupTimer);
            onTransportChange?.("h264", encoderName || "H.264");
          }
        },
        error: () => fallback(),
      });

      decoder.configure({
        codec,
        codedWidth: width,
        codedHeight: height,
        optimizeForLatency: true,
        hardwareAcceleration: "prefer-hardware",
      });
    };

    ws.onmessage = (evt) => {
      if (disposed) return;
      if (typeof evt.data === "string") {
        try {
          const msg = JSON.parse(evt.data) as {
            type: string;
            codec?: string;
            width?: number;
            height?: number;
            encoder?: string;
          };
          if (msg.type === "fallback") {
            fallback();
            return;
          }
          if (msg.type === "config" && msg.codec && msg.width && msg.height) {
            encoderName = msg.encoder ?? "H.264";
            ensureDecoder(msg.codec, msg.width, msg.height);
          }
        } catch {
          // ignore malformed control frame
        }
        return;
      }

      if (!(evt.data instanceof ArrayBuffer) || evt.data.byteLength <= 20) return;
      const view = new DataView(evt.data);
      // Verify 4-byte magic "VCH2"
      if (
        view.getUint8(0) !== 0x56 ||
        view.getUint8(1) !== 0x43 ||
        view.getUint8(2) !== 0x48 ||
        view.getUint8(3) !== 0x32
      ) {
        return;
      }
      const isKey = (view.getUint8(4) & 1) !== 0;
      const width = view.getUint16(8, true);
      const height = view.getUint16(10, true);
      const tsLo = view.getUint32(12, true);
      const tsHi = view.getUint32(16, true);
      const timestampUs = tsHi * 4294967296 + tsLo;

      if (!decoder || decoder.state !== "configured") {
        ensureDecoder(configuredCodec ?? "avc1.42e01f", width || 1280, height || 720);
      }
      if (!decoder || decoder.state !== "configured") return;

      if (!sawKeyframe) {
        if (!isKey) return;
        sawKeyframe = true;
      }

      // Drop stale queued frames if GPU decoder briefly lags so latency stays minimal
      if (decoder.decodeQueueSize > 3 && !isKey) {
        return;
      }

      try {
        const payload = new Uint8Array(evt.data, 20);
        decoder.decode(
          new EncodedVideoChunk({
            type: isKey ? "key" : "delta",
            timestamp: timestampUs,
            data: payload,
          })
        );
      } catch {
        fallback();
      }
    };

    ws.onerror = () => fallback();
    ws.onclose = () => {
      if (!disposed && !gotAnyFrame) fallback();
    };

    return () => {
      disposed = true;
      window.clearTimeout(startupTimer);
      try {
        ws.close();
      } catch {
        // ignore
      }
      if (decoder) {
        try {
          decoder.close();
        } catch {
          // ignore
        }
      }
    };
  }, [cameraId, onTransportChange]);

  if (useMjpeg) {
    return (
      <img
        className="monitor-img"
        src={mjpegStreamUrl(cameraId, maxFps)}
        alt={cameraName}
        draggable={false}
      />
    );
  }

  return <canvas ref={canvasRef} className="monitor-img" aria-label={cameraName} />;
};
