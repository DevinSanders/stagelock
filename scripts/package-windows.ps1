<#
.SYNOPSIS
    Builds an optimized Windows release bundle of VideoController:
    - Rust vc-daemon (--release)
    - Desktop Electron + Mobile/Tablet PWA static bundle
    - Bitfocus Companion / Buttons module
    - Standalone portable staging folder in dist-release/windows-x64/
#>
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

    Write-Host "1/4 Building Rust vc-daemon (release)..." -ForegroundColor Cyan
    & cargo build --release -p vc-daemon
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "2/4 Building Electron UI + Mobile/Tablet PWA..." -ForegroundColor Cyan
    & pnpm build:ui
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "3/4 Building Bitfocus Companion / Buttons module..." -ForegroundColor Cyan
    & pnpm --filter companion-module-videocontroller build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "4/4 Assembling portable staging bundle in dist-release/windows-x64/..." -ForegroundColor Cyan
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

    Write-Host ""
    Write-Host "Release bundle ready at: $stage" -ForegroundColor Green
    Write-Host "To build a signed Windows NSIS installer via electron-builder, run:" -ForegroundColor Gray
    Write-Host "  pnpm --filter @videocontroller/desktop exec electron-builder --win --x64" -ForegroundColor Gray
}
finally {
    Pop-Location
}
