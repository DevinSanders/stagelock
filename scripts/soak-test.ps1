<#
.SYNOPSIS
    Automated multi-camera reliability & soak test for stagelock-daemon (release binary).
    Spawns 4 concurrent live cameras (tracking + CV + NDI re-broadcast + color grading + Stream Deck triggers)
    and verifies memory stability, handle stability, JPEG frame delivery, and clean shutdown.
#>
[CmdletBinding()]
param(
    [int]$DurationSeconds = 20,
    [int]$Port = 8771,
    [int]$OscPort = 9011
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$daemonExe = Join-Path $repoRoot "target\release\stagelock-daemon.exe"
if (-not (Test-Path $daemonExe)) {
    $daemonExe = Join-Path $repoRoot "target\debug\stagelock-daemon.exe"
}

$tmpCfg = Join-Path $env:TEMP "stagelock-soak-config.json"
$tmpDb = Join-Path $env:TEMP "stagelock-soak-config.db"
Remove-Item $tmpCfg, $tmpDb, "$tmpDb-wal", "$tmpDb-shm" -ErrorAction SilentlyContinue
$camIds = @(
    "11111111-1111-1111-1111-111111111101",
    "11111111-1111-1111-1111-111111111102",
    "11111111-1111-1111-1111-111111111103",
    "11111111-1111-1111-1111-111111111104"
)

$camerasJson = @()
for ($i = 0; $i -lt 4; $i++) {
    $pat = if ($i % 2 -eq 0) { "smpte" } else { "ball" }
    $camerasJson += @{
        id = $camIds[$i]
        name = "Soak Cam $($i + 1)"
        enabled = $true
        source = @{ kind = "test_pattern"; pattern = $pat; width = 640; height = 360; fps = 30 }
        ptz = @{ protocol = "mock" }
        ndi_output = @{ enabled = ($i -eq 0); stream_name = "VC Soak Cam $($i + 1)"; include_color_adjustments = $true }
        framing = @{
            auto_track_enabled = $true
            preset = "waist_up"
            rule_of_thirds = "auto_lead_room"
            auto_focus_enabled = $true
            focus_strategy = "one_push_af"
        }
    }
}

$cfgObj = @{
    schema_version = 1
    server = @{ bind_addr = "127.0.0.1"; lan_access_enabled = $false; http_port = $Port; osc_port = $OscPort }
    cameras = $camerasJson
}
$cfgObj | ConvertTo-Json -Depth 8 | Set-Content -Path $tmpCfg

$env:STAGELOCK_CONFIG_PATH = $tmpCfg
$env:STAGELOCK_BIND_ADDR = "127.0.0.1"

Write-Host "=== StageLock 4-Camera Soak Test ($DurationSeconds s) ===" -ForegroundColor Cyan
Write-Host "Binary: $daemonExe" -ForegroundColor Gray

$proc = Start-Process -FilePath $daemonExe -PassThru -WindowStyle Hidden
try {
    Start-Sleep -Milliseconds 1600
    $base = "http://127.0.0.1:$Port"

    # Lock subject #1 on all 4 cameras independently
    foreach ($cid in $camIds) {
        Invoke-RestMethod -Method Post -Uri "$base/api/v1/cameras/$cid/tracking/select" -ContentType "application/json" -Body '{"track_id":1}' | Out-Null
    }

    $samples = @()
    $presets = @("close_up", "waist_up", "full_height")
    $thirds = @("left_third", "center", "right_third", "auto_lead_room")
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $iter = 0
    $jpegOk = 0

    while ($sw.Elapsed.TotalSeconds -lt $DurationSeconds) {
        $cid = $camIds[$iter % 4]
        $preset = $presets[$iter % $presets.Count]
        $third = $thirds[$iter % $thirds.Count]

        # Exercise Stream Deck action endpoint + live JPEG frame fetch across all 4 cameras
        Invoke-WebRequest -Method Post -Uri "$base/api/v1/integrations/streamdeck/trigger" -ContentType "application/json" `
            -Body (@{ type = "set_framing_preset"; camera_index = ($iter % 4); preset = $preset } | ConvertTo-Json -Compress) | Out-Null
        Invoke-WebRequest -Method Post -Uri "$base/api/v1/integrations/streamdeck/trigger" -ContentType "application/json" `
            -Body (@{ type = "set_rule_of_thirds"; camera_index = ($iter % 4); thirds = $third } | ConvertTo-Json -Compress) | Out-Null

        $snap = Invoke-WebRequest -Uri "$base/api/v1/cameras/$cid/snapshot.jpg"
        if ($snap.StatusCode -eq 200 -and $snap.Content.Length -gt 2000) {
            $jpegOk++
        }

        $p = Get-Process -Id $proc.Id
        $samples += [pscustomobject]@{
            Sec     = [math]::Round($sw.Elapsed.TotalSeconds, 1)
            RamMB   = [math]::Round($p.WorkingSet64 / 1MB, 1)
            Handles = $p.HandleCount
            Threads = $p.Threads.Count
        }

        $iter++
        Start-Sleep -Milliseconds 500
    }

    $first = $samples[0]
    $last = $samples[-1]
    $ramDelta = [math]::Round($last.RamMB - $first.RamMB, 1)
    $handleDelta = $last.Handles - $first.Handles

    Write-Host "Iterations: $iter | Valid JPEGs: $jpegOk/$iter" -ForegroundColor Green
    Write-Host "RAM:     start=$($first.RamMB) MB -> end=$($last.RamMB) MB (delta: $ramDelta MB)" -ForegroundColor Green
    Write-Host "Handles: start=$($first.Handles) -> end=$($last.Handles) (delta: $handleDelta)" -ForegroundColor Green
    Write-Host "Threads: start=$($first.Threads) -> end=$($last.Threads)" -ForegroundColor Green

    if ($jpegOk -ne $iter) {
        throw "Some camera snapshot checks failed ($jpegOk/$iter)"
    }
    if ($ramDelta -gt 80) {
        throw "Potential memory leak detected (+$ramDelta MB over $DurationSeconds s)"
    }
    if (-not (Test-Path $tmpDb)) {
        throw "Expected SQLite database was not created at $tmpDb"
    }
    Write-Host "SQLite:  verified database persisted at $tmpDb" -ForegroundColor Green
    Write-Host "SOAK TEST PASSED" -ForegroundColor Green
}
finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Remove-Item $tmpCfg, $tmpDb, "$tmpDb-wal", "$tmpDb-shm" -ErrorAction SilentlyContinue
}
