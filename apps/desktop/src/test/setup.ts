import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

afterEach(() => {
  cleanup();
});

// Default desktop viewport matchMedia polyfill (override per-test to simulate mobile/tablet PWA!)
export function setViewportCompact(compact: boolean) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    configurable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: query.includes("max-width") ? compact : false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
}

setViewportCompact(false);

// Mock 2D Canvas context for WebCodecsVideo rendering assertions
Object.defineProperty(HTMLCanvasElement.prototype, "getContext", {
  writable: true,
  configurable: true,
  value: vi.fn(() => ({
    drawImage: vi.fn(),
    clearRect: vi.fn(),
  })),
});
