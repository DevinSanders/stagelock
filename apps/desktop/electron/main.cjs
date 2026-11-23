const { app, BrowserWindow, shell } = require("electron");
const path = require("path");
const http = require("http");
const { spawn } = require("child_process");
const fs = require("fs");

let mainWindow = null;
let daemonProcess = null;
let isQuitting = false;

function resolveDaemonBinary() {
  const exeName = process.platform === "win32" ? "vc-daemon.exe" : "vc-daemon";
  const packagedBin = path.resolve(process.resourcesPath || "", "bin", exeName);
  if (app.isPackaged) {
    return fs.existsSync(packagedBin) ? packagedBin : null;
  }
  const devCandidates = [
    path.resolve(__dirname, "..", "..", "..", "target", "debug", exeName),
    path.resolve(__dirname, "..", "..", "..", "target", "release", exeName),
    packagedBin,
  ].filter((p) => fs.existsSync(p));
  if (devCandidates.length === 0) return null;
  // In dev mode, prefer whichever binary was compiled most recently
  devCandidates.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
  return devCandidates[0];
}

function resolveWebDistDir() {
  const candidates = [
    path.resolve(process.resourcesPath || "", "web"),
    path.resolve(__dirname, "..", "dist"),
  ];
  return candidates.find((p) => fs.existsSync(path.join(p, "index.html"))) || null;
}

function maybeStartDaemon() {
  if (process.env.VC_EXTERNAL_DAEMON === "1") return;
  const binPath = resolveDaemonBinary();
  if (!binPath) {
    console.log("[electron] vc-daemon binary not found; assuming daemon is started externally on :8765");
    return;
  }
  const webDist = resolveWebDistDir();
  const env = { ...process.env };
  if (webDist && !env.VC_WEB_DIST_DIR) {
    env.VC_WEB_DIST_DIR = webDist;
  }
  console.log("[electron] Launching vc-daemon:", binPath, "webDist:", webDist);
  const child = spawn(binPath, [], {
    stdio: "inherit",
    windowsHide: true,
    env,
  });
  daemonProcess = child;

  child.on("error", (err) => {
    console.error("[electron] vc-daemon process error:", err.message);
    if (daemonProcess === child) daemonProcess = null;
  });

  child.on("exit", (code, signal) => {
    console.log("[electron] vc-daemon exited", signal ? `via ${signal}` : `with code ${code}`);
    if (daemonProcess === child) daemonProcess = null;
  });
}

function requestDaemonHttpShutdown() {
  return new Promise((resolve) => {
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port: Number(process.env.VC_HTTP_PORT || 8765),
        path: "/api/v1/shutdown",
        method: "POST",
        timeout: 600,
      },
      (res) => {
        res.resume();
        res.on("end", resolve);
      }
    );
    req.on("timeout", () => {
      req.destroy();
      resolve();
    });
    req.on("error", () => resolve());
    req.end();
  });
}

async function stopDaemonGracefully() {
  const child = daemonProcess;
  if (!child || child.exitCode !== null || child.killed) {
    daemonProcess = null;
    return;
  }

  const exited = new Promise((resolve) => {
    child.once("exit", resolve);
    child.once("error", resolve);
  });

  await requestDaemonHttpShutdown();

  const timer = new Promise((resolve) => setTimeout(() => resolve("timeout"), 1500));
  const outcome = await Promise.race([exited, timer]);

  if (outcome === "timeout" && daemonProcess === child && child.exitCode === null) {
    try {
      child.kill();
    } catch {
      // ignore if already exited
    }
  }
  if (daemonProcess === child) daemonProcess = null;
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1520,
    height: 940,
    minWidth: 1120,
    minHeight: 720,
    backgroundColor: "#0b0e14",
    title: "VideoController — Multi-Camera AI PTZ Studio",
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });

  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url);
    return { action: "deny" };
  });

  mainWindow.on("closed", () => {
    mainWindow = null;
  });

  const isDev = !app.isPackaged;
  if (isDev) {
    mainWindow.loadURL("http://127.0.0.1:5173");
  } else {
    mainWindow.loadFile(path.join(__dirname, "..", "dist", "index.html"));
  }
}

app.whenReady().then(() => {
  maybeStartDaemon();
  createWindow();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("before-quit", (event) => {
  if (isQuitting) return;
  if (daemonProcess && daemonProcess.exitCode === null) {
    event.preventDefault();
    isQuitting = true;
    stopDaemonGracefully().finally(() => {
      app.quit();
    });
  } else {
    isQuitting = true;
  }
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
