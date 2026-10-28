# Captures the NamiVision main window to a PNG for visual QA.
# Usage: powershell -File scripts\capture-window.ps1 -Out shots\app.png [-ProcessId 1234]
param(
  [string]$Out = "shots\app.png",
  [int]$ProcessId = 0,
  [int]$WaitSeconds = 0
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinCap {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
[WinCap]::SetProcessDPIAware() | Out-Null

if ($WaitSeconds -gt 0) { Start-Sleep -Seconds $WaitSeconds }

$proc = $null
if ($ProcessId -gt 0) {
  $proc = Get-Process -Id $ProcessId -ErrorAction Stop
} else {
  $proc = Get-Process | Where-Object { $_.MainWindowTitle -like '*NamiVision*' } | Select-Object -First 1
}
if (-not $proc) { throw "NamiVision window not found" }

$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) {
  for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Milliseconds 250
    $proc.Refresh()
    $hwnd = $proc.MainWindowHandle
    if ($hwnd -ne [IntPtr]::Zero) { break }
  }
}
if ($hwnd -eq [IntPtr]::Zero) { throw "window handle not available" }

[WinCap]::ShowWindow($hwnd, 9) | Out-Null
[WinCap]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 600

$rect = [WinCap+RECT]::new()
[WinCap]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -le 0 -or $h -le 0) { throw "invalid window rect ${w}x${h}" }

$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# PW_RENDERFULLCONTENT (3) captures WebView2 content
$ok = [WinCap]::PrintWindow($hwnd, $hdc, 3)
$g.ReleaseHdc($hdc)
$g.Dispose()
if (-not $ok) {
  $bmp.Dispose()
  throw "PrintWindow failed"
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$bmp.Save((Join-Path (Get-Location) $Out), [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "captured $Out ($w x $h)"
