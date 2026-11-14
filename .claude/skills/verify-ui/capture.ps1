# Screenshot of a Kuality GPUI window without taking the focus, optionally after a click or a scroll.
# Run with `pwsh -NoProfile -File capture.ps1 -Out <png> [-Title <title>] [-ClickX x -ClickY y] [-Scroll n]`
# (a fresh process each time: the types below can only be added once per session).
# Click coordinates are pixels of a previous capture (window coordinates, frame included).
# -Scroll n: n wheel notches down (negative: up), at 70 % width / 50 % height of the window.
param([string]$Title = 'Kuality Settings', [string]$Out, [int]$Scroll = 0, [int]$ClickX = -1, [int]$ClickY = -1)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W2 {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  public struct RECT { public int L, T, R, B; }
  public struct POINT { public int X, Y; }
}
"@
[W2]::SetProcessDPIAware() | Out-Null
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 60 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $h = [W2]::FindWindow([NullString]::Value, $Title) }
if ($h -eq [IntPtr]::Zero) { throw "window '$Title' not found" }
[W2]::ShowWindow($h, 4) | Out-Null
$r = New-Object W2+RECT
[W2]::GetWindowRect($h, [ref]$r) | Out-Null
$o = New-Object W2+POINT
[W2]::ClientToScreen($h, [ref]$o) | Out-Null
$fx = $o.X - $r.L; $fy = $o.Y - $r.T
function lp([int]$x, [int]$y) { [IntPtr](($y -shl 16) -bor ($x -band 0xffff)) }
if ($ClickX -ge 0) {
  $cx = $ClickX - $fx; $cy = $ClickY - $fy
  [W2]::PostMessage($h, 0x200, [IntPtr]0, (lp $cx $cy)) | Out-Null
  Start-Sleep -Milliseconds 150
  [W2]::PostMessage($h, 0x201, [IntPtr]1, (lp $cx $cy)) | Out-Null
  Start-Sleep -Milliseconds 80
  [W2]::PostMessage($h, 0x202, [IntPtr]0, (lp $cx $cy)) | Out-Null
  Start-Sleep -Milliseconds 800
}
if ($Scroll -ne 0) {
  $sx = $r.L + [int](($r.R - $r.L) * 0.7); $sy = $r.T + [int](($r.B - $r.T) * 0.5)
  for ($k = 0; $k -lt [Math]::Abs($Scroll); $k++) {
    $delta = if ($Scroll -gt 0) { -120 } else { 120 }
    $w = [IntPtr]([int64](($delta -band 0xffff) -shl 16))
    [W2]::PostMessage($h, 0x20A, $w, (lp $sx $sy)) | Out-Null
    Start-Sleep -Milliseconds 60
  }
}
Start-Sleep -Seconds 2
$bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc(); [W2]::PrintWindow($h, $hdc, 2) | Out-Null; $g.ReleaseHdc($hdc)
$bmp.Save($Out); "saved $Out size $($r.R - $r.L)x$($r.B - $r.T) frame $fx,$fy"
