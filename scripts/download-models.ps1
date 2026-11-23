<#
.SYNOPSIS
    Downloads permissive Apache-2.0 / MIT ONNX vision models into ./models for VideoController.
#>
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$modelsDir = Join-Path $repoRoot "models"
New-Item -ItemType Directory -Force -Path $modelsDir | Out-Null

$downloads = @(
    @{
        Name = "RTMDet-s Person Detector (Apache-2.0)"
        File = "rtmdet-s.onnx"
        Url  = "https://huggingface.co/ziq/rtm/resolve/main/rtmdet-s.onnx"
    },
    @{
        Name = "RTMPose-s 17-Keypoint Pose (Apache-2.0)"
        File = "rtmpose-s.onnx"
        Url  = "https://huggingface.co/pzoltowski/rtmpose-s-onnx/resolve/main/rtmpose-s_working.onnx"
    }
)

Write-Host "=== VideoController Permissive ONNX Model Downloader ===" -ForegroundColor Cyan
foreach ($m in $downloads) {
    $dest = Join-Path $modelsDir $m.File
    if (Test-Path $dest) {
        Write-Host "[OK] Already present: $($m.File)" -ForegroundColor Green
        continue
    }
    Write-Host "Downloading $($m.Name) -> $($m.File)..." -ForegroundColor Yellow
    Invoke-WebRequest -Uri $m.Url -OutFile $dest
    $sizeMb = [math]::Round((Get-Item $dest).Length / 1MB, 2)
    Write-Host "[OK] Downloaded $($m.File) ($sizeMb MB)" -ForegroundColor Green
}

Write-Host ""
Write-Host "Note: Spatial 3-zone HSV Re-ID runs built-in with zero weights. Optionally drop osnet_x0_25.onnx (MIT) into models/ for neural 512-dim Re-ID." -ForegroundColor Gray
Write-Host "Build daemon with ONNX Runtime enabled: cargo build -p vc-daemon --features vc-vision/onnx" -ForegroundColor Gray
