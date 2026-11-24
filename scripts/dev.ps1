<#
.SYNOPSIS
    Builds the Remote PWA assets and launches the VideoController Tauri 2 desktop studio
    (with vc-daemon embedded in-process and LAN Remote PWA served on :8765).
#>
$ErrorActionPreference = "Continue"

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot

try {
    # Ensure freshly installed toolchains are on PATH even before opening a new shell
    $extraPaths = @(
        (Join-Path $env:USERPROFILE ".cargo\bin"),
        "C:\Program Files\nodejs",
        "C:\Program Files\gstreamer\1.0\msvc_x86_64\bin",
        (Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Packages\pnpm.pnpm_Microsoft.Winget.Source_8wekyb3d8bbwe"),
        (Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Links")
    )
    foreach ($p in $extraPaths) {
        if ((Test-Path $p) -and ($env:PATH -notlike "*$p*")) {
            $env:PATH = "$p;$env:PATH"
        }
    }

    # Enable zero-pipe in-process gstreamer-rs automatically when GStreamer MSVC dev headers are installed
    $gstPkgConfig = "C:\Program Files\gstreamer\1.0\msvc_x86_64\lib\pkgconfig"
    $tauriExtraArgs = @()
    if (Test-Path (Join-Path $gstPkgConfig "gstreamer-1.0.pc")) {
        $env:PKG_CONFIG_PATH = $gstPkgConfig
        $tauriExtraArgs = @("--features", "gst-native")
        Write-Host "GStreamer MSVC SDK detected: enabling in-process gst-native low-latency pipeline." -ForegroundColor Cyan
    }

    Write-Host "Building Remote Web / PWA assets (served on LAN :8765)..." -ForegroundColor Cyan
    & pnpm build:ui
    if ($LASTEXITCODE -ne 0) {
        Write-Error "pnpm build:ui failed with exit code $LASTEXITCODE"
        exit $LASTEXITCODE
    }

    Write-Host "Launching VideoController Tauri 2 Studio (embedded vc-daemon + LAN PWA on http://<LAN-IP>:8765)..." -ForegroundColor Green
    & pnpm --filter @videocontroller/desktop tauri dev @tauriExtraArgs
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
