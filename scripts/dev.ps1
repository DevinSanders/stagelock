<#
.SYNOPSIS
    Builds the Remote PWA bundle + vc-daemon and launches the VideoController desktop studio.
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

    # Guard against Node 26 + extract-zip bug leaving electron.exe unextracted
    $electronExe = Join-Path $repoRoot "apps\desktop\node_modules\electron\dist\electron.exe"
    if (-not (Test-Path $electronExe)) {
        Write-Host "Electron binary missing; attempting repair from local Electron cache..." -ForegroundColor Yellow
        $cacheZip = Get-ChildItem (Join-Path $env:LOCALAPPDATA "electron\Cache") -Recurse -Filter "electron-*-win32-x64.zip" -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($cacheZip) {
            $electronDir = (Get-Item (Join-Path $repoRoot "apps\desktop\node_modules\electron")).Target
            if (-not $electronDir) { $electronDir = Join-Path $repoRoot "apps\desktop\node_modules\electron" }
            $distDir = Join-Path $electronDir "dist"
            Remove-Item -Recurse -Force $distDir -ErrorAction SilentlyContinue
            Expand-Archive -Path $cacheZip.FullName -DestinationPath $distDir -Force
            Set-Content -Path (Join-Path $electronDir "path.txt") -Value "electron.exe" -NoNewline
        }
    }

    Write-Host "Building Remote Web / PWA assets (served on LAN :8765)..." -ForegroundColor Cyan
    & pnpm build:ui
    if ($LASTEXITCODE -ne 0) {
        Write-Error "pnpm build:ui failed with exit code $LASTEXITCODE"
        exit $LASTEXITCODE
    }

    Write-Host "Building vc-daemon..." -ForegroundColor Cyan
    & cargo build -p vc-daemon
    if ($LASTEXITCODE -ne 0) {
        Write-Error "cargo build failed with exit code $LASTEXITCODE"
        exit $LASTEXITCODE
    }

    Write-Host "Launching Electron Studio UI (and LAN Remote PWA on http://<LAN-IP>:8765)..." -ForegroundColor Green
    & pnpm dev:ui
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
