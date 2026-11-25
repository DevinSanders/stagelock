<#
.SYNOPSIS
    Builds production Windows installers & bundles for StageLock (Tauri 2):
    - Rust standalone headless stagelock-daemon (--release) with embedded SQLite
    - React/Vite Desktop Studio + LAN Mobile/Tablet Remote PWA
    - Bitfocus Companion / Buttons module
    - Compact native Tauri 2 Windows NSIS (.exe) & MSI (.msi) installers with embedded stagelock-daemon
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

    $gstPkgConfig = "C:\Program Files\gstreamer\1.0\msvc_x86_64\lib\pkgconfig"
    $cargoFeatureArgs = @()
    if (Test-Path (Join-Path $gstPkgConfig "gstreamer-1.0.pc")) {
        $env:PKG_CONFIG_PATH = $gstPkgConfig
        $cargoFeatureArgs = @("--features", "gst-native")
        Write-Host "GStreamer MSVC SDK detected: enabling in-process gst-native low-latency pipeline." -ForegroundColor Cyan
    }

    Write-Host "1/5 Building standalone headless stagelock-daemon (release)..." -ForegroundColor Cyan
    & cargo build --release -p stagelock-daemon @cargoFeatureArgs
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "2/5 Building Desktop UI + Mobile/Tablet PWA..." -ForegroundColor Cyan
    & pnpm build:ui
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "3/5 Building Bitfocus Companion / Buttons module..." -ForegroundColor Cyan
    & pnpm --filter companion-module-stagelock build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "4/5 Staging portable headless daemon & Companion bundle..." -ForegroundColor Cyan
    $stage = Join-Path $repoRoot "dist-release\windows-x64"
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "bin") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "web") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "companion-module") | Out-Null

    Copy-Item -Force (Join-Path $repoRoot "target\release\stagelock-daemon.exe") (Join-Path $stage "bin\stagelock-daemon.exe")
    Copy-Item -Recurse -Force (Join-Path $repoRoot "apps\desktop\dist\*") (Join-Path $stage "web\")
    Copy-Item -Recurse -Force (Join-Path $repoRoot "integrations\companion-module\dist") (Join-Path $stage "companion-module\")
    Copy-Item -Recurse -Force (Join-Path $repoRoot "integrations\companion-module\companion") (Join-Path $stage "companion-module\")
    Copy-Item -Force (Join-Path $repoRoot "integrations\companion-module\package.json") (Join-Path $stage "companion-module\")
    Copy-Item -Force (Join-Path $repoRoot "LICENSE") $stage
    Copy-Item -Force (Join-Path $repoRoot "LICENSE-EXCEPTION") $stage
    Copy-Item -Force (Join-Path $repoRoot "docs\ndi-attribution.md") $stage

    if (-not $SkipInstaller) {
        Write-Host "5/5 Building Tauri 2 Windows NSIS (.exe) & MSI (.msi) installers..." -ForegroundColor Cyan
        & pnpm --filter @stagelock/desktop tauri build --bundles nsis,msi @cargoFeatureArgs
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

        Write-Host ""
        Write-Host "Windows installers created in target\release\bundle\:" -ForegroundColor Green
        Get-ChildItem -Path (Join-Path $repoRoot "target\release\bundle") -Recurse -Include "*.exe", "*.msi" | ForEach-Object {
            $mb = [math]::Round($_.Length / 1MB, 1)
            Write-Host "  - $($_.FullName) ($mb MB)" -ForegroundColor Green
        }
    } else {
        Write-Host "5/5 Skipping Tauri installer (-SkipInstaller specified)." -ForegroundColor Yellow
    }

    Write-Host ""
    Write-Host "Headless staging bundle ready at: $stage" -ForegroundColor Gray
}
finally {
    Pop-Location
}
