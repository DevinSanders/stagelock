import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AddCameraModal } from "./AddCameraModal";

describe("AddCameraModal (all video & PTZ protocols, discovery happy & failure paths)", () => {
  it("discovers NDI sources, lets user pick one, and submits camera config", async () => {
    vi.spyOn(api, "discoverNdi").mockResolvedValue({
      available: true,
      sources: [
        { name: "STUDIO (CAM 1)", url_address: "10.0.0.10:5961" },
        { name: "STUDIO (CAM 2)", url_address: "10.0.0.11:5961" },
      ],
    });
    const onCreate = vi.fn().mockResolvedValue(undefined);
    const onClose = vi.fn();

    render(<AddCameraModal onClose={onClose} onCreate={onCreate} />);
    await waitFor(() => expect(screen.getByText(/STUDIO \(CAM 1\)/)).toBeInTheDocument());

    // Pick second NDI source -> updates name automatically
    const selects = screen.getAllByRole("combobox");
    fireEvent.change(selects[1], { target: { value: "STUDIO (CAM 2)" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));

    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(1));
    expect(onCreate.mock.calls[0][0].source).toEqual({
      kind: "ndi",
      source_name: "STUDIO (CAM 2)",
      low_bandwidth: false,
    });
    expect(onCreate.mock.calls[0][0].ptz).toEqual({ protocol: "ndi" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("handles ONVIF discovery, profile/RTSP resolution happy & failure paths, and PTZ credentials", async () => {
    vi.spyOn(api, "discoverNdi").mockRejectedValue(new Error("ndi offline"));
    vi.spyOn(api, "discoverOnvif").mockResolvedValue({
      devices: [
        { xaddr: "http://192.168.1.88/onvif", host: "192.168.1.88", port: 80, name: "PTZOptics 4K" },
        { xaddr: "http://192.168.1.89/onvif", host: "192.168.1.89", port: 8080, name: "BirdDog P200" },
      ],
    });
    vi.spyOn(api, "resolveOnvif")
      .mockRejectedValueOnce(new Error("HTTP 502: 401 Unauthorized"))
      .mockResolvedValueOnce({
        media_xaddr: "http://192.168.1.89:8080/media",
        ptz_xaddr: "http://192.168.1.89:8080/ptz",
        profile_token: "Profile_Main",
        rtsp_uri: "rtsp://192.168.1.89:554/main",
      });

    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(<AddCameraModal onClose={vi.fn()} onCreate={onCreate} />);

    const sourceSelect = screen.getAllByRole("combobox")[0];
    fireEvent.change(sourceSelect, { target: { value: "onvif" } });

    await waitFor(() => expect(screen.getByText(/Found 2 ONVIF camera/i)).toBeInTheDocument());

    // Pick second ONVIF camera
    const onvifDropdown = screen.getAllByRole("combobox")[1];
    fireEvent.change(onvifDropdown, { target: { value: "192.168.1.89:8080" } });

    // First resolve fails (401) -> displays clear error message
    fireEvent.click(screen.getByRole("button", { name: /Resolve RTSP & Profile/i }));
    await waitFor(() => expect(screen.getByText(/Resolve failed: HTTP 502: 401 Unauthorized/)).toBeInTheDocument());

    // Enter password & resolve succeeds
    fireEvent.change(screen.getAllByPlaceholderText("Password")[0], { target: { value: "secret" } });
    fireEvent.click(screen.getByRole("button", { name: /Resolve RTSP & Profile/i }));
    await waitFor(() => expect(screen.getByText(/Resolved profile 'Profile_Main'/)).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(1));
    expect(onCreate.mock.calls[0][0].source).toMatchObject({
      kind: "onvif",
      host: "192.168.1.89",
      port: 8080,
      resolved_rtsp_url: "rtsp://192.168.1.89:554/main",
    });
    expect(onCreate.mock.calls[0][0].ptz).toMatchObject({
      protocol: "onvif",
      profile_token: "Profile_Main",
    });
  });

  it("supports USB, RTSP, WebRTC, DeckLink, TestPattern sources and all PTZ protocols + empty discovery failure", async () => {
    vi.spyOn(api, "discoverNdi").mockResolvedValue({ available: false, sources: [] });
    vi.spyOn(api, "discoverUsb")
      .mockResolvedValueOnce({
        devices: [
          { device_id: "usb-0", name: "CamLink 4K", api: "mediafoundation", max_width: 3840, max_height: 2160 },
          { device_id: "usb-1", name: "vMix Video", api: "mediafoundation", max_width: 1920, max_height: 1080 },
        ],
      })
      .mockRejectedValueOnce(new Error("usb scan err"));
    vi.spyOn(api, "discoverOnvif")
      .mockResolvedValueOnce({ devices: [] })
      .mockRejectedValueOnce(new Error("ws-discovery err"));

    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(<AddCameraModal onClose={vi.fn()} onCreate={onCreate} />);

    const sourceSelect = screen.getAllByRole("combobox")[0];

    // USB happy path + device switch + submit (capped to 1920x1080)
    fireEvent.change(sourceSelect, { target: { value: "usb" } });
    await waitFor(() => expect(screen.getByText(/CamLink 4K/)).toBeInTheDocument());
    fireEvent.change(screen.getAllByRole("combobox")[1], { target: { value: "usb-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Rescan Capture" })); // triggers error branch safely
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(1));
    expect(onCreate.mock.calls[0][0].source).toMatchObject({ kind: "usb", device_id: "usb-1" });

    // RTSP + VISCA Sony UDP
    fireEvent.change(sourceSelect, { target: { value: "rtsp" } });
    fireEvent.change(screen.getByDisplayValue("rtsp://192.168.1.50:554/stream1"), {
      target: { value: "rtsp://10.0.0.9/live" },
    });
    const ptzSelect = () => screen.getAllByRole("combobox")[screen.getAllByRole("combobox").length - 1];
    fireEvent.change(ptzSelect(), { target: { value: "visca_ip_sony" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(2));
    expect(onCreate.mock.calls[1][0].ptz).toMatchObject({ protocol: "visca_ip_sony", port: 52381 });

    // WebRTC + VISCA Raw TCP
    fireEvent.change(sourceSelect, { target: { value: "web_rtc" } });
    fireEvent.change(ptzSelect(), { target: { value: "visca_ip_raw_tcp" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(3));
    expect(onCreate.mock.calls[2][0].source).toMatchObject({ kind: "web_rtc" });
    expect(onCreate.mock.calls[2][0].ptz).toMatchObject({ protocol: "visca_ip_raw_tcp", port: 5678 });

    // DeckLink + VISCA Raw UDP
    fireEvent.change(sourceSelect, { target: { value: "deck_link" } });
    fireEvent.change(ptzSelect(), { target: { value: "visca_ip_raw_udp" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(4));
    expect(onCreate.mock.calls[3][0].source).toMatchObject({ kind: "deck_link", device_index: 0 });
    expect(onCreate.mock.calls[3][0].ptz).toMatchObject({ protocol: "visca_ip_raw_udp", port: 1259 });

    // TestPattern + Canon XC + Panasonic AW + Mock
    fireEvent.change(sourceSelect, { target: { value: "test_pattern" } });
    fireEvent.change(ptzSelect(), { target: { value: "canon_xc" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(5));
    expect(onCreate.mock.calls[4][0].ptz).toMatchObject({ protocol: "canon_xc", port: 80 });

    fireEvent.change(ptzSelect(), { target: { value: "panasonic_aw" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(6));
    expect(onCreate.mock.calls[5][0].ptz).toMatchObject({ protocol: "panasonic_aw", port: 80 });

    fireEvent.change(ptzSelect(), { target: { value: "mock" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(7));
    expect(onCreate.mock.calls[6][0].ptz).toEqual({ protocol: "mock" });

    // ONVIF empty & error status
    fireEvent.change(sourceSelect, { target: { value: "onvif" } });
    await waitFor(() => expect(screen.getByText(/No ONVIF cameras responded/i)).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Scan ONVIF" }));
    await waitFor(() => expect(screen.getByText("ONVIF LAN scan failed")).toBeInTheDocument());
  });
});
