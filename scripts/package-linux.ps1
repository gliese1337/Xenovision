#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Builds the Linux release package of Xenovision through WSL.

.DESCRIPTION
  Runs scripts/package.sh (the Linux/macOS packaging script) inside Windows
  Subsystem for Linux, producing dist/xenovision-<version>-linux-<arch>.tar.gz
  with the user-guide PDF inside it.

  The WSL distro needs the same tools package.sh checks for: cargo, pandoc, and
  wkhtmltopdf (e.g. `sudo apt install cargo pandoc wkhtmltopdf`).

  Build artifacts go to -TargetDir, which lives on the Linux filesystem - never
  the repo's target/ - so Linux objects can't collide with a native Windows
  build's cache on /mnt/c, and compilation stays on ext4 (much faster than
  /mnt/c). Re-runs reuse that directory for incremental builds.

  The Windows repo path is converted to a WSL path via wslpath, and wsl.exe's
  exit code is propagated so callers see build failures. Uses bash -lc (login
  shell) so rustup's PATH setup from ~/.profile is in effect.

.PARAMETER Distro
  WSL distribution to run in (wsl --list). Defaults to WSL's default distro.

.PARAMETER TargetDir
  WSL-side directory used as CARGO_TARGET_DIR. Default:
  $HOME/xenovision-linux-target (the literal $HOME is expanded by bash inside
  WSL).

.PARAMETER RepoPath
  Windows path of the repo to build. Default: the repo this script lives in.

.EXAMPLE
  scripts/package-linux.ps1

.EXAMPLE
  scripts/package-linux.ps1 -Distro Ubuntu-26.04 -TargetDir /home/me/xeno-target
#>
[CmdletBinding()]
param(
    [string]$Distro,
    [string]$TargetDir = '$HOME/xenovision-linux-target',
    [string]$RepoPath = ''
)

$ErrorActionPreference = 'Stop'

if (-not $RepoPath) { $RepoPath = Split-Path -Parent $PSScriptRoot }
if (-not $RepoPath) {
    Write-Error "cannot determine the repo path; pass -RepoPath." -ErrorAction Stop
}

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
    Write-Error "wsl.exe not found - install WSL first (wsl --install)." -ErrorAction Stop
}

$RepoPath = (Resolve-Path -LiteralPath $RepoPath).Path

# Assemble the wsl.exe argv prefix (-d <distro> only when requested).
$wslArgv = @()
if ($Distro) { $wslArgv += @('-d', $Distro) }

# Translate the Windows path to the WSL path bash can actually cd to.
# Note: don't pipe wsl.exe through Select-Object -First 1 - early pipeline
# termination makes the native process report a bogus exit code (-1). Capture
# the output and trim afterwards instead.
$wslOut = & wsl.exe @wslArgv -e wslpath -a "$RepoPath"
if ($LASTEXITCODE -ne 0 -or -not $wslOut) {
    Write-Error "could not translate '$RepoPath' to a WSL path (wslpath failed)." -ErrorAction Stop
}
$wslRepo = ((@($wslOut) -join "`n") -split "`r?`n" |
        Where-Object { $_.Trim() } |
        Select-Object -First 1).Trim()

$bashCmd = "cd '$wslRepo' && export CARGO_TARGET_DIR=$TargetDir && bash scripts/package.sh"

Write-Host "==> Building Linux package in WSL"
Write-Host "    repo        : $wslRepo"
Write-Host "    target dir  : $TargetDir"
Write-Host "    command     : wsl.exe $($wslArgv -join ' ') -e bash -lc `"$bashCmd`""

& wsl.exe @wslArgv -e bash -lc $bashCmd
exit $LASTEXITCODE