<#
.SYNOPSIS
    Installs Windows MSVC development prerequisites for StageLock.
    Run in an elevated (Administrator) PowerShell terminal.
#>
#Requires -RunAsAdministrator
[CmdletBinding()]
param(
    [string]$GStreamerVersion = "1.28.9",
    [switch]$SkipVSBuildTools
)

$ErrorActionPreference = "Stop"

Write-Host "=== StageLock Windows Toolchain Setup ===" -ForegroundColor Cyan

# 1. Winget packages (installed sequentially)
$packages = @(
    @{ Id = "Git.Git"; Name = "Git"; ExtraArgs = @("--silent") },
    @{ Id = "OpenJS.NodeJS.LTS"; Name = "Node.js LTS"; ExtraArgs = @("--silent") },
    @{ Id = "pnpm.pnpm"; Name = "pnpm"; ExtraArgs = @("--silent") },
    @{ Id = "Rustlang.Rustup"; Name = "Rustup (MSVC)"; ExtraArgs = @("--silent", "--override", "-y") }
)

foreach ($pkg in $packages) {
    Write-Host "Installing $($pkg.Name) ($($pkg.Id))..." -ForegroundColor Yellow
    $wingetArgs = @("install", "--id", $pkg.Id, "--exact", "--accept-package-agreements", "--accept-source-agreements") + $pkg.ExtraArgs
    & winget @wingetArgs
}

if (-not $SkipVSBuildTools) {
    Write-Host "Installing Visual Studio 2022 C++ Build Tools (MSVC Linker + Windows SDK)..." -ForegroundColor Yellow
    winget install --id Microsoft.VisualStudio.2022.BuildTools --exact --accept-package-agreements --accept-source-agreements `
        --override "--quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
}

# 2. GStreamer 1.28+ unified MSVC installer (includes Runtime + Dev headers/libs)
$gstBaseUrl = "https://gstreamer.freedesktop.org/data/pkg/windows/$GStreamerVersion/msvc"
$tmpDir = Join-Path $env:TEMP "stagelock-gst"
New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

$file = "gstreamer-1.0-msvc-x86_64-$GStreamerVersion.exe"
$dest = Join-Path $tmpDir $file
$url = "$gstBaseUrl/$file"
if (-not (Test-Path $dest)) {
    Write-Host "Downloading $file..." -ForegroundColor Yellow
    Invoke-WebRequest -Uri $url -OutFile $dest
}
Write-Host "Installing $file (Unified Runtime + Dev)..." -ForegroundColor Yellow
Start-Process -FilePath $dest -ArgumentList "/SILENT /SUPPRESSMSGBOXES /NORESTART /SP-" -Wait

Write-Host ""
Write-Host "Setup complete! Open a NEW terminal and verify:" -ForegroundColor Green
Write-Host "  rustc --version; cargo --version; node --version; pnpm --version; gst-inspect-1.0 --version" -ForegroundColor Gray
