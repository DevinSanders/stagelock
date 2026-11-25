import { render, screen, act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WebCodecsVideo } from "./WebCodecsVideo";

class MockWebSocket {
  static instances: MockWebSocket[] = [];
  binaryType = "blob";
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  close = vi.fn();
  constructor(public url: string) {
    MockWebSocket.instances.push(this);
  }
}

class MockEncodedVideoChunk {
  constructor(public init: { type: string; timestamp: number; data: Uint8Array }) {}
}

function makeSlh2Packet(isKey: boolean, width = 1280, height = 720, tsUs = 33333): ArrayBuffer {
  const buf = new ArrayBuffer(26);
  const view = new DataView(buf);
  view.setUint8(0, 0x53); // S
  view.setUint8(1, 0x4c); // L
  view.setUint8(2, 0x48); // H
  view.setUint8(3, 0x32); // 2
  view.setUint8(4, isKey ? 1 : 0);
  view.setUint8(5, 1);
  view.setUint16(8, width, true);
  view.setUint16(10, height, true);
  view.setUint32(12, tsUs >>> 0, true);
  view.setUint32(16, 0, true);
  view.setUint8(20, 0x00);
  view.setUint8(21, 0x00);
  view.setUint8(22, 0x00);
  view.setUint8(23, 0x01);
  view.setUint8(24, isKey ? 0x65 : 0x41);
  view.setUint8(25, 0x88);
  return buf;
}

describe("WebCodecsVideo (GPU H.264 happy path & automatic MJPEG failure fallbacks)", () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
    vi.stubGlobal("EncodedVideoChunk", MockEncodedVideoChunk);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("decodes H.264 keyframe + delta packets onto canvas via WebCodecs VideoDecoder", () => {
    let outputCb: ((frame: { displayWidth: number; displayHeight: number; close: () => void }) => void) | null = null;
    const decodeSpy = vi.fn();
    const configureSpy = vi.fn();
    const closeSpy = vi.fn();
    let queueSize = 0;

    class MockVideoDecoder {
      state = "unconfigured";
      get decodeQueueSize() {
        return queueSize;
      }
      constructor(init: { output: typeof outputCb }) {
        outputCb = init.output;
      }
      configure(cfg: unknown) {
        this.state = "configured";
        configureSpy(cfg);
      }
      decode(chunk: unknown) {
        decodeSpy(chunk);
        outputCb?.({ displayWidth: 1280, displayHeight: 720, close: closeSpy });
      }
      close = vi.fn();
    }
    vi.stubGlobal("VideoDecoder", MockVideoDecoder);

    const onTransport = vi.fn();
    const { unmount } = render(
      <WebCodecsVideo cameraId="cam-1" cameraName="Stage Cam" onTransportChange={onTransport} />
    );

    const ws = MockWebSocket.instances[0];
    expect(ws.url).toContain("/ws/video/cam-1");

    act(() => {
      // Send config JSON + malformed JSON (ignored safely)
      ws.onmessage?.({ data: "{bad_json" });
      ws.onmessage?.({
        data: JSON.stringify({ type: "config", codec: "avc1.42c01f", width: 1280, height: 720, encoder: "nvh264enc" }),
      });
      // Ignore bad magic & delta before first keyframe
      ws.onmessage?.({ data: new ArrayBuffer(24) });
      ws.onmessage?.({ data: makeSlh2Packet(false) });
      // Feed real IDR keyframe + delta frame
      ws.onmessage?.({ data: makeSlh2Packet(true, 1280, 720, 1000) });
      ws.onmessage?.({ data: makeSlh2Packet(false, 1280, 720, 34333) });
      // Backpressure drops stale delta frame when decodeQueueSize > 3
      queueSize = 5;
      ws.onmessage?.({ data: makeSlh2Packet(false, 1280, 720, 67666) });
    });

    expect(configureSpy).toHaveBeenCalledWith(
      expect.objectContaining({ codec: "avc1.42c01f", optimizeForLatency: true })
    );
    expect(decodeSpy).toHaveBeenCalledTimes(2);
    expect(closeSpy).toHaveBeenCalledTimes(2);
    expect(onTransport).toHaveBeenCalledWith("h264", "nvh264enc");
    expect(screen.getByLabelText("Stage Cam").tagName).toBe("CANVAS");
    unmount();
    expect(ws.close).toHaveBeenCalled();
  });

  it("falls back immediately to MJPEG when browser lacks VideoDecoder", () => {
    vi.stubGlobal("VideoDecoder", undefined);
    const onTransport = vi.fn();
    render(<WebCodecsVideo cameraId="cam-2" cameraName="Cam 2" maxFps={12} onTransportChange={onTransport} />);
    const img = screen.getByAltText("Cam 2") as HTMLImageElement;
    expect(img.tagName).toBe("IMG");
    expect(img.src).toContain("/api/v1/cameras/cam-2/stream.mjpeg?max_fps=12");
    expect(onTransport).toHaveBeenCalledWith("mjpeg");
  });

  it("falls back to MJPEG on server fallback message, decoder error, WS error, or startup timeout", () => {
    vi.useFakeTimers();
    let errorCb: (() => void) | null = null;
    class FailingDecoder {
      state = "configured";
      decodeQueueSize = 0;
      constructor(init: { error: () => void }) {
        errorCb = init.error;
      }
      configure() {}
      decode() {
        throw new Error("decode failed");
      }
      close() {}
    }
    vi.stubGlobal("VideoDecoder", FailingDecoder);

    // 1. Server sends explicit fallback JSON
    const onTransport1 = vi.fn();
    render(<WebCodecsVideo cameraId="c1" cameraName="C1" onTransportChange={onTransport1} />);
    act(() => {
      MockWebSocket.instances[0].onmessage?.({ data: JSON.stringify({ type: "fallback", reason: "no encoder" }) });
    });
    expect(screen.getByAltText("C1")).toBeInTheDocument();
    expect(onTransport1).toHaveBeenCalledWith("mjpeg");

    // 2. Startup timeout (2.5s without frame)
    render(<WebCodecsVideo cameraId="c2" cameraName="C2" />);
    act(() => {
      vi.advanceTimersByTime(2600);
    });
    expect(screen.getByAltText("C2")).toBeInTheDocument();

    // 3. WebSocket error before first frame
    render(<WebCodecsVideo cameraId="c3" cameraName="C3" />);
    act(() => {
      MockWebSocket.instances[2].onerror?.();
    });
    expect(screen.getByAltText("C3")).toBeInTheDocument();

    // 4. Decode exception triggers fallback
    render(<WebCodecsVideo cameraId="c4" cameraName="C4" />);
    act(() => {
      MockWebSocket.instances[3].onmessage?.({ data: makeSlh2Packet(true) });
    });
    expect(screen.getByAltText("C4")).toBeInTheDocument();

    // 5. Decoder async error callback triggers fallback
    render(<WebCodecsVideo cameraId="c5" cameraName="C5" />);
    act(() => {
      MockWebSocket.instances[4].onmessage?.({
        data: JSON.stringify({ type: "config", codec: "avc1.42c01f", width: 640, height: 360 }),
      });
      errorCb?.();
    });
    expect(screen.getByAltText("C5")).toBeInTheDocument();
  });
});
