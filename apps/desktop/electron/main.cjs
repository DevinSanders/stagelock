const { app, BrowserWindow, shell } = require("electron");
const path = require("path");
const { spawn } = require("child_process");
const fs = require("fs");

let mainWindow = null;
let daemonProcess = null;

function resolveDaemonBinary() {
  const exeName = process.platform === "win32" ? "vc-daemon.exe" : "vc-daemon";
  const packagedBin = path.resolve(process.resourcesPath || "", "bin", exeName);
  const devRelease = path.resolve(__dirname, "..", "..", "..", "target", "release", exeName);
  const devDebug = path.resolve(__dirname, "..", "..", "..", "target", "debug", exeName);
  const candidates = app.isPackaged
    ? [packagedBin, devRelease, devDebug]
    : [devRelease, devDebug, packagedBin];
  return candidates.find((p) => fs.existsSync(p)) || null;
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
  daemonProcess = spawn(binPath, [], {
    stdio: "inherit",
    windowsHide: true,
    env,
  });
  daemonProcess.on("exit", (code) => {
    console.log("[electron] vc-daemon exited with code", code);
    daemonProcess = null;
  });
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

app.on("window-all-closed", () => {
  if (daemonProcess) {
    daemonProcess.kill();
    daemonProcess = null;
  }
  if (process.platform !== "darwin") app.quit();
});
