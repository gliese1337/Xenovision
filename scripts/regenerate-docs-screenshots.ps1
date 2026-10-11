#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Regenerates the user-guide screenshots in docs/screenshots/ from a live build.

.DESCRIPTION
  Builds and runs xenovision-app in its `--screenshot` mode, which seeds the app
  with a sample-data folder (USGS reflectance spectra from its
  usgs_splib07/curves/json and ENVI hyperspectral cubes from its envi/),
  captures one PNG per panel straight from the GPU framebuffer, writes them to
  the output directory, and exits. Re-running this script after any UI change
  refreshes every image in docs/user-guide.md, so the guide cannot drift from
  the app.

  All seeding happens in memory: the app never touches your per-user data
  directory, so a run is reproducible and safe.

.PARAMETER OutputDir
  Where the PNGs are written. Default: docs/screenshots.

.PARAMETER DataDir
  The sample-data folder (must contain usgs_splib07/ and envi/). Default: data.
  It is not part of the repository; pass the path to your own copy.

.PARAMETER Release
  Use a release build (slower to compile, faster to run). Default: debug, which
  also keeps the console visible so this script can show the app's progress.

.EXAMPLE
  scripts/regenerate-docs-screenshots.ps1

.EXAMPLE
  scripts/regenerate-docs-screenshots.ps1 -OutputDir docs/screenshots -DataDir data -Release
#>
[CmdletBinding()]
param(
    [string]$OutputDir = 'docs/screenshots',
    [string]$DataDir = 'data',
    [switch]$Release
)

$ErrorActionPreference = 'Stop'

# Run from the repo root, whatever directory the caller was in.
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    if (-not (Test-Path -LiteralPath $DataDir)) {
        throw "Sample data directory '$DataDir' not found. Point -DataDir at a folder containing usgs_splib07/ and envi/ (the data/ folder is not part of the repository)."
    }

    $profileArgs = if ($Release) { @('--release') } else { @() }

    # Native stderr (cargo happily logs to stderr) must not be a terminating
    # error under `$ErrorActionPreference = 'Stop'` - that only bites when the
    # caller also redirects our output streams, but it bites hard then.
    $prevEap = $ErrorActionPreference

    Write-Host 'Building xenovision-app...'
    $ErrorActionPreference = 'Continue'
    try {
        & cargo build -p xenovision-app @profileArgs
        $buildCode = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $prevEap }
    if ($buildCode -ne 0) { throw "cargo build failed (exit $buildCode)" }

    $profileDir = if ($Release) { 'release' } else { 'debug' }
    $exe = Join-Path $root "target/$profileDir/xenovision-app.exe"
    if (-not (Test-Path -LiteralPath $exe)) {
        # Non-Windows layout (this file is Windows-only, but be forgiving).
        $exe = Join-Path $root "target/$profileDir/xenovision-app"
    }

    # Expected outputs, in capture order (matches the app's SCENES list).
    $expected = @(
        'main-window.png',
        'comparison.png',
        'stimulus-editor.png',
        'image-display.png',
        'stimulus-editor-import.png'
    )

    # Drop stale images so a failed run can't masquerade as success.
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
    foreach ($name in $expected) {
        $p = Join-Path $OutputDir $name
        if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Force }
    }

    Write-Host 'Capturing screenshots...'
    $ErrorActionPreference = 'Continue'
    try {
        & $exe --screenshot $OutputDir --demo-data $DataDir
        $code = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $prevEap }
    if ($code -ne 0) { throw "xenovision-app --screenshot failed (exit $code)" }

    $missing = @($expected | Where-Object { -not (Test-Path -LiteralPath (Join-Path $OutputDir $_)) })
    if ($missing.Count -gt 0) {
        throw "Capture reported success but these images are missing: $($missing -join ', ')"
    }

    Write-Host ''
    Write-Host "Wrote $($expected.Count) screenshot(s) to ${OutputDir}:"
    foreach ($name in $expected) {
        $p = Join-Path $OutputDir $name
        $size = [math]::Round((Get-Item -LiteralPath $p).Length / 1KB, 1)
        Write-Host ("  {0,-32} {1,8} KB" -f $name, $size)
    }
}
finally {
    Pop-Location
}
