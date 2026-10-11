#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Screenshot a single window, cropped tight to its bounds.

.DESCRIPTION
  Finds a top-level window by title (wildcard or regex), optionally raises it
  to the foreground, then captures only that window's rectangle - no manual
  cropping. Windows only (uses Win32 + the DWM API).

  Two crop modes:
    Frame   the visible window frame (title bar + borders), via
            DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS) so the invisible
            drop-shadow padding is excluded. Falls back to GetWindowRect.
    Client  the content area only, without title bar or borders.

  The process is made DPI-aware first, so GetWindowRect/CopyFromScreen agree on
  physical pixels and the crop is not blurred by OS scaling.

.PARAMETER Title
  Title pattern to match. Wildcards by default (e.g. 'Xenovision*'), regex with
  -Regex. If several windows match, the largest is chosen and the rest listed.

.PARAMETER Output
  PNG path to write. Defaults to xenovision-capture.png in the temp directory.
  Parent directories are created if needed.

.PARAMETER Mode
  Frame (default) or Client. See above.

.PARAMETER Regex
  Treat Title as a regular expression instead of a wildcard pattern.

.PARAMETER ProcessId
  Restrict matches to one owning process id.

.PARAMETER List
  List matching windows (handle, pid, size, title) and exit without capturing.

.PARAMETER NoActivate
  Do not raise/restore the window before capturing. Use only when the window is
  already frontmost and fully visible, otherwise other windows occlude it.

.PARAMETER ActivateDelayMs
  Milliseconds to wait after raising the window for it to come forward and
  repaint. Default 350.

.EXAMPLE
  scripts/capture-window.ps1 -Title 'Xenovision*' -Output shot.png

.EXAMPLE
  scripts/capture-window.ps1 -Title 'Xenovision - Comparison' -Mode Client

.EXAMPLE
  scripts/capture-window.ps1 -List
#>
[CmdletBinding()]
param(
    [string]$Title = 'Xenovision*',
    [string]$Output,
    [ValidateSet('Frame', 'Client')][string]$Mode = 'Frame',
    [switch]$Regex,
    [int]$ProcessId = 0,
    [switch]$List,
    [switch]$NoActivate,
    [int]$ActivateDelayMs = 350
)

$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies 'System.Drawing' -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class Win32
{
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left, Top, Right, Bottom;
        public int Width { get { return Right - Left; } }
        public int Height { get { return Bottom - Top; } }
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X, Y; }

    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] private static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] private static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] private static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] private static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] private static extern bool SetProcessDPIAware();
    [DllImport("dwmapi.dll")] private static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT val, int size);

    private const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;
    private const int SW_RESTORE = 9;

    public class WindowInfo
    {
        public IntPtr Handle;
        public string Title;
        public uint Pid;
        public bool Minimized;
        public RECT Frame;
        public RECT Client;
        public int Area { get { return Frame.Width * Frame.Height; } }
        public override string ToString()
        {
            return string.Format("0x{0:X}  pid {1,-6}  {2}x{3}  {4}", (long)Handle, Pid, Frame.Width, Frame.Height, Title);
        }
    }

    public static void MakeDpiAware() { try { SetProcessDPIAware(); } catch { } }

    // Visible frame bounds, minus the invisible DWM resize/shadow padding.
    public static RECT GetFrame(IntPtr h)
    {
        RECT r;
        if (DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, out r, Marshal.SizeOf(typeof(RECT))) == 0
            && r.Right > r.Left && r.Bottom > r.Top)
            return r;
        GetWindowRect(h, out r);
        return r;
    }

    public static RECT GetClientOnScreen(IntPtr h)
    {
        RECT r;
        GetClientRect(h, out r);
        POINT tl = new POINT { X = r.Left, Y = r.Top };
        POINT br = new POINT { X = r.Right, Y = r.Bottom };
        ClientToScreen(h, ref tl);
        ClientToScreen(h, ref br);
        return new RECT { Left = tl.X, Top = tl.Y, Right = br.X, Bottom = br.Y };
    }

    public static List<WindowInfo> Enumerate()
    {
        var list = new List<WindowInfo>();
        EnumWindows((h, l) =>
        {
            if (!IsWindowVisible(h)) return true;
            int len = GetWindowTextLength(h);
            if (len == 0) return true;
            var sb = new StringBuilder(len + 1);
            GetWindowText(h, sb, sb.Capacity);
            uint pid; GetWindowThreadProcessId(h, out pid);
            var info = new WindowInfo
            {
                Handle = h, Title = sb.ToString(), Pid = pid,
                Minimized = IsIconic(h), Frame = GetFrame(h), Client = GetClientOnScreen(h)
            };
            if (info.Frame.Width > 0 && info.Frame.Height > 0) list.Add(info);
            return true;
        }, IntPtr.Zero);
        return list;
    }

    public static void Activate(IntPtr h)
    {
        if (IsIconic(h)) ShowWindow(h, SW_RESTORE);
        SetForegroundWindow(h);
    }
}
'@

[Win32]::MakeDpiAware()

$all = [Win32]::Enumerate() |
    Where-Object { $_.Title -and $_.Title.Trim().Length -gt 0 } |
    Where-Object { $ProcessId -eq 0 -or $_.Pid -eq $ProcessId }

$candidates =
    if ($Regex) { $all | Where-Object { $_.Title -match $Title } }
    else        { $all | Where-Object { $_.Title -like  $Title } }
$candidates = @($candidates | Sort-Object -Property Area -Descending)

if ($List) {
    if (-not $candidates) { Write-Warning "No window matches '$Title'."; exit 1 }
    $candidates | Format-Table Handle, Pid, @{ n = 'Size'; e = { "$($_.Frame.Width)x$($_.Frame.Height)" } }, Title -AutoSize
    exit 0
}

if (-not $candidates) {
    Write-Error "No visible window matches '$Title'. Run with -List to see what is open."
    exit 1
}

$win = $candidates[0]
if ($candidates.Count -gt 1) {
    Write-Warning "'$Title' matched $($candidates.Count) windows; using the largest. Others:"
    $candidates | Select-Object -Skip 1 | ForEach-Object { Write-Warning "  $_" }
}

if ($win.Minimized -and $NoActivate) {
    Write-Error "Window '$($win.Title)' is minimized; drop -NoActivate so it can be restored first."
    exit 1
}

if (-not $NoActivate) {
    [Win32]::Activate($win.Handle)
    Start-Sleep -Milliseconds $ActivateDelayMs
}

$rect = if ($Mode -eq 'Client') { $win.Client } else { $win.Frame }

if ($rect.Width -le 0 -or $rect.Height -le 0) {
    Write-Error "Window '$($win.Title)' has an empty $Mode rectangle."
    exit 1
}

Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap($rect.Width, $rect.Height)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
try {
    $gfx.CopyFromScreen($rect.Left, $rect.Top, 0, 0,
        (New-Object System.Drawing.Size($rect.Width, $rect.Height)),
        [System.Drawing.CopyPixelOperation]::SourceCopy)
}
finally { $gfx.Dispose() }

if (-not $Output) {
    $Output = Join-Path ([System.IO.Path]::GetTempPath()) 'xenovision-capture.png'
}
$dir = Split-Path -Parent $Output
if ($dir -and -not (Test-Path -LiteralPath $dir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}
$bmp.Save($Output, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

Write-Host "Captured '$($win.Title)' ($Mode, $($rect.Width)x$($rect.Height), pid $($win.Pid))"
Write-Host "  -> $Output"
Write-Output $Output
