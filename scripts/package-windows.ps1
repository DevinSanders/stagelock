<#
.SYNOPSIS
    Builds production Windows installers & bundles for VideoController:
    - Rust vc-daemon (--release) with embedded SQLite
    - Electron Desktop Studio + LAN Mobile/Tablet Remote PWA
    - Bitfocus Companion / Buttons module
    - Windows NSIS installer (.exe) + Portable (.exe) via electron-builder
#>
[CmdletBinding()]
param(
    [switch]$SkipInstaller
)

$ErrorActionPreference = "Continue"
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot

try {
    $extraPaths = @(
        (Join-Path $env:USERPROFILE ".cargo\bin"),
        "C:\Program Files\nodejs",
        "C:\Program Files\gstreamer\1.0\msvc_x86_64\bin",
        (Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Packages\pnpm.pnpm_Microsoft.Winget.Source_8wekyb3d8bbwe")
    )
    foreach ($p in $extraPaths) {
        if ((Test-Path $p) -and ($env:PATH -notlike "*$p*")) {
            $env:PATH = "$p;$env:PATH"
        }
    }

    Write-Host "1/5 Building Rust vc-daemon (release)..." -ForegroundColor Cyan
    & cargo build --release -p vc-daemon
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "2/5 Building Electron UI + Mobile/Tablet PWA..." -ForegroundColor Cyan
    & pnpm build:ui
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "3/5 Building Bitfocus Companion / Buttons module..." -ForegroundColor Cyan
    & pnpm --filter companion-module-videocontroller build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "4/5 Staging portable headless daemon & Companion bundle..." -ForegroundColor Cyan
    $stage = Join-Path $repoRoot "dist-release\windows-x64"
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "bin") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "web") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "companion-module") | Out-Null

    Copy-Item -Force (Join-Path $repoRoot "target\release\vc-daemon.exe") (Join-Path $stage "bin\vc-daemon.exe")
    Copy-Item -Recurse -Force (Join-Path $repoRoot "apps\desktop\dist\*") (Join-Path $stage "web\")
    Copy-Item -Recurse -Force (Join-Path $repoRoot "integrations\companion-module\dist") (Join-Path $stage "companion-module\")
    Copy-Item -Recurse -Force (Join-Path $repoRoot "integrations\companion-module\companion") (Join-Path $stage "companion-module\")
    Copy-Item -Force (Join-Path $repoRoot "integrations\companion-module\package.json") (Join-Path $stage "companion-module\")
    Copy-Item -Force (Join-Path $repoRoot "LICENSE-APACHE") $stage
    Copy-Item -Force (Join-Path $repoRoot "LICENSE-MIT") $stage
    Copy-Item -Force (Join-Path $repoRoot "docs\ndi-attribution.md") $stage

    if (-not $SkipInstaller) {
        # Ensure local Electron dist binary is intact (Node.js v26 extract-zip workaround)
        $electronDir = Join-Path $repoRoot "apps\desktop\node_modules\electron"
        $electronExe = Join-Path $electronDir "dist\electron.exe"
        if ((Test-Path $electronDir) -and (-not (Test-Path $electronExe))) {
            $cacheRoot = Join-Path $env:LOCALAPPDATA "electron\Cache"
            $cachedZip = Get-ChildItem -Path $cacheRoot -Recurse -Filter "electron-*-win32-x64.zip" -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($cachedZip) {
                Write-Host "Repairing Electron binary from local cache..." -ForegroundColor Yellow
                $distDir = Join-Path $electronDir "dist"
                if (Test-Path $distDir) { Remove-Item -Recurse -Force $distDir }
                Expand-Archive -Path $cachedZip.FullName -DestinationPath $distDir -Force
                Set-Content -Path (Join-Path $electronDir "path.txt") -Value "electron.exe" -NoNewline
            }
        }

        Write-Host "5/5 Building Windows NSIS (.exe) & Portable (.exe) installers via electron-builder..." -ForegroundColor Cyan
        $env:CSC_IDENTITY_AUTO_DISCOVERY = "false"
        & pnpm --filter @videocontroller/desktop package:win
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

        Write-Host ""
        Write-Host "Windows installers created in apps\desktop\release\:" -ForegroundColor Green
        Get-ChildItem -Path (Join-Path $repoRoot "apps\desktop\release") -Filter "*.exe" | ForEach-Object {
            $mb = [math]::Round($_.Length / 1MB, 1)
            Write-Host "  - $($_.Name) ($mb MB)" -ForegroundColor Green
        }
    } else {
        Write-Host "5/5 Skipping electron-builder (-SkipInstaller specified)." -ForegroundColor Yellow
    }

    Write-Host ""
    Write-Host "Headless staging bundle ready at: $stage" -ForegroundColor Gray
}
finally {
    Pop-Location
}
