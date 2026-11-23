import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles.css";

// Register PWA service worker when running in mobile/desktop browsers (skip inside Electron)
if (!window.vcHost && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch(() => {
      // Service worker requires HTTPS or localhost; LAN HTTP still works cleanly as a web app / iOS Home Screen app
    });
  });
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
