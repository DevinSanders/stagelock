<#
.SYNOPSIS
    Measures real source-based unit test coverage for both Rust crates (LLVM coverage)
    and Frontend React/TypeScript UI (Vitest V8 coverage), enforcing >= 90% coverage.
#>
[CmdletBinding()]
param(
    [double]$MinCoverage = 90.0,
    [switch]$RustOnly,
    [switch]$UiOnly,
    [switch]$Offline
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

    $failed = $false

    if (-not $UiOnly) {
        Write-Host "=== Rust Workspace LLVM Coverage ===" -ForegroundColor Cyan
        $sysroot = (& rustc --print sysroot).Trim()
        $hostTriple = (& rustc -vV | Select-String "^host:").ToString().Split(":")[1].Trim()
        $llvmBin = Join-Path $sysroot "lib\rustlib\$hostTriple\bin"
        $llvmProfdata = Join-Path $llvmBin "llvm-profdata.exe"
        $llvmCov = Join-Path $llvmBin "llvm-cov.exe"

        if (-not (Test-Path $llvmCov)) {
            Write-Error "llvm-tools-preview not installed. Run: rustup component add llvm-tools-preview"
            exit 1
        }

        $covDir = Join-Path $repoRoot "target\llvm-cov-tmp"
        if (Test-Path $covDir) { Remove-Item -Recurse -Force $covDir }
        New-Item -ItemType Directory -Force -Path $covDir | Out-Null

        $prevRustFlags = $env:RUSTFLAGS
        $prevProfile = $env:LLVM_PROFILE_FILE
        $prevTarget = $env:CARGO_TARGET_DIR
        try {
            $env:RUSTFLAGS = "-C instrument-coverage"
            $env:LLVM_PROFILE_FILE = Join-Path $covDir "vc-%p-%m.profraw"
            $env:CARGO_TARGET_DIR = Join-Path $repoRoot "target\cov-target"

            $crates = @("vc-core", "vc-media", "vc-ptz", "vc-vision", "vc-tracking", "vc-integrations", "vc-daemon")
            $pkgArgs = @()
            foreach ($c in $crates) { $pkgArgs += @("-p", $c) }
            $offlineArgs = if ($Offline) { @("--offline") } else { @() }

            # Build test binaries and collect exact executable paths via --message-format json
            $buildArgs = @("test", "--no-run") + $offlineArgs + $pkgArgs + @("--message-format", "json")
            $buildJson = & cargo @buildArgs
            if ($LASTEXITCODE -ne 0) {
                Write-Error "Instrumented cargo test build failed"
                exit $LASTEXITCODE
            }

            $testBins = @()
            foreach ($line in $buildJson) {
                if ($line -match '^\{') {
                    try {
                        $obj = $line | ConvertFrom-Json
                        if ($obj.reason -eq "compiler-artifact" -and $obj.executable -and $obj.profile.test -eq $true) {
                            $testBins += $obj.executable
                        }
                    } catch {}
                }
            }

            # Run instrumented tests
            $runArgs = @("test") + $offlineArgs + $pkgArgs
            & cargo @runArgs
            if ($LASTEXITCODE -ne 0) {
                Write-Error "Instrumented Rust tests failed"
                exit $LASTEXITCODE
            }

            $profraws = Get-ChildItem -Path $covDir -Filter "*.profraw" | ForEach-Object { $_.FullName }
            $profdata = Join-Path $covDir "merged.profdata"
            & $llvmProfdata merge -sparse @profraws -o $profdata
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

            $objArgs = @()
            for ($i = 0; $i -lt $testBins.Count; $i++) {
                if ($i -eq 0) {
                    $objArgs += $testBins[$i]
                } else {
                    $objArgs += @("--object", $testBins[$i])
                }
            }

            $ignoreRegex = '(\.cargo|rustc|target\\|src\\main\.rs$)'
            $report = & $llvmCov report `
                --instr-profile=$profdata `
                --ignore-filename-regex=$ignoreRegex `
                @objArgs
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

            $report | ForEach-Object { Write-Host $_ }

            # Parse TOTAL line percentage (Line coverage is column 10: Regions Missed Cover | Functions Missed Cover | Lines Missed Cover)
            $totalLine = $report | Select-String "^TOTAL\s+" | Select-Object -Last 1
            if ($totalLine) {
                $parts = ($totalLine.ToString().Trim() -split '\s+')
                # Format: TOTAL regions missed pct funcs missed pct lines missed pct ...
                $linePctStr = $parts[9].TrimEnd('%')
                $linePct = [double]$linePctStr
                Write-Host ""
                Write-Host ("Rust Total Line Coverage: {0:N2}% (Target: >= {1:N1}%)" -f $linePct, $MinCoverage) -ForegroundColor $(if ($linePct -ge $MinCoverage) { "Green" } else { "Yellow" })
                if ($linePct -lt $MinCoverage) {
                    $failed = $true
                }
            }
        }
        finally {
            $env:RUSTFLAGS = $prevRustFlags
            $env:LLVM_PROFILE_FILE = $prevProfile
            $env:CARGO_TARGET_DIR = $prevTarget
        }
    }

    if (-not $RustOnly) {
        Write-Host ""
        Write-Host "=== Frontend UI Tests & V8 Coverage (Vitest + RTL) ===" -ForegroundColor Cyan
        & pnpm --filter @videocontroller/desktop test:coverage
        if ($LASTEXITCODE -ne 0) {
            $failed = $true
        }
    }

    if ($failed) {
        Write-Error "Coverage gate failed (< $MinCoverage%) or tests failed."
        exit 1
    }
    Write-Host ""
    Write-Host "ALL TESTS & >= $MinCoverage% COVERAGE GATES PASSED!" -ForegroundColor Green
}
finally {
    Pop-Location
}
