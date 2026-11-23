const { contextBridge } = require("electron");

contextBridge.exposeInMainWorld("vcHost", {
  daemonBaseUrl: process.env.VC_DAEMON_URL || "http://127.0.0.1:8765",
  platform: process.platform,
});
