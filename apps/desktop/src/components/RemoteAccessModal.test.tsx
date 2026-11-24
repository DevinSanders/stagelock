import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { RemoteAccessModal } from "./RemoteAccessModal";

describe("RemoteAccessModal (QR code, multi-NIC selector, clipboard happy & failure paths)", () => {
  it("loads LAN URLs, allows interface switching, copies URL, and warns if PWA is not ready", async () => {
    vi.spyOn(api, "getRemoteInfo").mockResolvedValue({
      lan_access_enabled: true,
      http_port: 8765,
      osc_port: 9000,
      pwa_ready: false,
      lan_urls: ["http://10.7.21.2:8765", "http://192.168.1.50:8765"],
      local_url: "http://127.0.0.1:8765",
    });

    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });

    const onClose = vi.fn();
    render(<RemoteAccessModal onClose={onClose} />);

    expect(screen.getByText(/Detecting local network addresses/i)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("textbox")).toHaveValue("http://10.7.21.2:8765"));

    // Switch network interface
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "http://192.168.1.50:8765" } });
    expect(screen.getByRole("textbox")).toHaveValue("http://192.168.1.50:8765");

    // Copy URL happy path
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("http://192.168.1.50:8765"));
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();

    // Warning shown when pwa_ready is false
    expect(screen.getByText(/Built PWA assets were not detected yet/i)).toBeInTheDocument();

    // Close button works
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("handles clipboard denial gracefully and falls back to local_url when lan_urls is empty", async () => {
    vi.spyOn(api, "getRemoteInfo").mockResolvedValue({
      lan_access_enabled: true,
      http_port: 8765,
      osc_port: 9000,
      pwa_ready: true,
      lan_urls: [],
      local_url: "http://127.0.0.1:8765",
    });
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error("permission denied")) },
    });

    render(<RemoteAccessModal onClose={vi.fn()} />);
    await waitFor(() => expect(screen.getByDisplayValue("http://127.0.0.1:8765")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
  });
});
