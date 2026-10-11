#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Builds a native Windows release package of Xenovision.

.DESCRIPTION
  Parallel to scripts/package.sh on Linux/macOS: builds the release binary with
  cargo and packages it with the user-guide PDF into
  dist/xenovision-<version>-windows-x86_64.zip.

  Windows only. Needs: cargo, pandoc, wkhtmltopdf (the last two only to render
  the user guide PDF; pass -SkipPdf if they are not installed). The PDF is
  rendered exactly like package.sh does, header included.

  The exe is stripped with llvm-strip when one is on PATH (e.g. from `rustup
  component add llvm-tools-preview`); an MSVC toolchain has no built-in strip,
  so when neither llvm-strip nor strip exists the exe is shipped unstripped and
  the script carries on with a warning.

.EXAMPLE
  scripts/package.ps1                # full package (build + PDF + zip)

.EXAMPLE
  scripts/package.ps1 -SkipPdf       # binary-only package, no pandoc needed

.PARAMETER SkipPdf
  Skip rendering docs/user-guide.md to user-guide.pdf. Also skips the pandoc
  and wkhtmltopdf requirement checks, so a package can be built on a machine
  that lacks them.
#>
[CmdletBinding()]
param(
    [switch]$SkipPdf
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

# Re-merge the registry Machine + User PATH into this process. A tool installed
# in a previous session (e.g. a freshly downloaded portable pandoc) is on the
# user's PATH setting but not on this process's inherited environment block;
# this mirrors what a fresh login would inherit anyway.
$env:Path = @(
    [Environment]::GetEnvironmentVariable('Path', 'Machine'),
    [Environment]::GetEnvironmentVariable('Path', 'User'),
    $env:Path
) -join ';'

function need([string]$name, [string]$hint) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        Write-Error "missing required tool: $name ($hint)" -ErrorAction Stop
    }
}

need cargo "install Rust from https://rustup.rs"
if (-not $SkipPdf) {
    need pandoc "e.g. choco install pandoc / winget install JohnMacFarlane.Pandoc"
    need wkhtmltopdf "e.g. choco install wkhtmltopdf / winget install wkhtmltopdf.wkhtmltopdf"
} else {
    Write-Warning "Skipping the user-guide PDF (-SkipPdf)."
}

$version = Select-String -Path "$Root\crates\xenovision-app\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1
if (-not $version) { Write-Error "could not read version from crates/xenovision-app/Cargo.toml" -ErrorAction Stop }
$version = $version.Matches[0].Groups[1].Value

$Dist  = Join-Path $Root 'dist'
$Work  = Join-Path $env:TEMP ("xenovision-pkg-" + [guid]::NewGuid().ToString('N'))
$NAME  = "xenovision-$version-windows-x86_64"
$PKG   = Join-Path $Work $NAME
$Zip   = Join-Path $Dist "$NAME.zip"
New-Item -ItemType Directory -Force -Path $Dist, $Work, $PKG | Out-Null

try {
    if (-not $SkipPdf) {
        Write-Host "==> Rendering user guide PDF"
        $headHtml = Join-Path $Work 'head.html'
        "<style>" | Set-Content -LiteralPath $headHtml -Encoding utf8
        Get-Content -LiteralPath "$Root\docs\user-guide-print.css" -Raw |
            Add-Content -LiteralPath $headHtml -Encoding utf8
        "</style>" | Add-Content -LiteralPath $headHtml -Encoding utf8

        $quiet = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'   # native stderr must not be fatal here
        $code = 1
        try {
            # Render from inside docs/ so the guide's relative image paths
            # (screenshots/*.png) resolve for the PDF engine.
            Push-Location (Join-Path $Root 'docs')
            try {
                & pandoc user-guide.md -o (Join-Path $Work 'user-guide.pdf') `
                    --pdf-engine=wkhtmltopdf `
                    --metadata pagetitle="Xenovision User Guide" `
                    --include-in-header $headHtml `
                    -V margin-top=18mm -V margin-bottom=18mm -V margin-left=18mm -V margin-right=18mm `
                    2>&1 | ForEach-Object { "$_" } |
                        Where-Object { $_ -notmatch 'QStandardPaths|Loading pages|Printing pages|Done|\[=' }
                $code = $LASTEXITCODE
            }
            finally { Pop-Location }
        }
        finally { $ErrorActionPreference = $quiet }
        if ($code -ne 0) { Write-Error "pandoc failed (exit $code)" -ErrorAction Stop }
    }

    Write-Host "==> Building Xenovision (release)"
    # Native stderr (cargo logs to it) must not be a terminating error under
    # `$ErrorActionPreference = 'Stop'`; check the exit code instead.
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & cargo build --release -p xenovision-app
        $buildCode = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $prevEap }
    if ($buildCode -ne 0) { Write-Error "cargo build failed (exit $buildCode)" -ErrorAction Stop }

    $exe = Join-Path $Root "target\release\xenovision-app.exe"
    if (-not (Test-Path -LiteralPath $exe)) {
        Write-Error "build produced no target\release\xenovision-app.exe" -ErrorAction Stop
    }
    Copy-Item -LiteralPath $exe -Destination (Join-Path $PKG 'Xenovision.exe')

    $strip = Get-Command llvm-strip -ErrorAction SilentlyContinue
    if (-not $strip) { $strip = Get-Command strip -ErrorAction SilentlyContinue }
    if ($strip) {
        $ErrorActionPreference = 'Continue'
        try {
            & $strip.Source (Join-Path $PKG 'Xenovision.exe')
            $stripCode = $LASTEXITCODE
        }
        finally { $ErrorActionPreference = $prevEap }
        if ($stripCode -ne 0) { Write-Warning "strip exited $stripCode; keeping the exe unstripped" }
    } else {
        Write-Warning "no llvm-strip/strip on PATH; ship the exe unstripped (rustup component add llvm-tools-preview)."
    }

    if (-not $SkipPdf) {
        Copy-Item -LiteralPath (Join-Path $Work 'user-guide.pdf') -Destination $PKG
    }

    Write-Host "==> Packaging $Zip"
    if (Test-Path -LiteralPath $Zip) { Remove-Item -LiteralPath $Zip -Force }
    Compress-Archive -Path $PKG -DestinationPath $Zip -CompressionLevel Optimal

    $sizeMb = [math]::Round((Get-Item -LiteralPath $Zip).Length / 1MB, 1)
    Write-Host "==> $Zip ($sizeMb MB)"
    Write-Host "    contents:"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($Zip)
    try { $archive.Entries | ForEach-Object { Write-Host "      $($_.FullName)" } }
    finally { $archive.Dispose() }
}
finally {
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
}