# A borderless window covering one screen exactly, to test the presence rule.
#
# Per-monitor DPI aware on purpose: without it the coordinates are virtualised (a 2880x1920
# screen reports 1440x960) and the window would not actually cover the screen, so the rule
# would correctly decide nothing and the test would prove nothing.
#
# Usage: powershell -File fullscreen-probe.ps1 <screen-index>
param([int]$Index = 0)

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class FsProbe {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
"@
Add-Type -AssemblyName System.Windows.Forms

# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4, before any window exists
[void][FsProbe]::SetProcessDpiAwarenessContext([IntPtr](-4))

$status = Join-Path $env:TEMP "floaty-probe\probe-status.txt"
"pid=$PID starting" | Set-Content -Path $status -Encoding ascii

$all = [System.Windows.Forms.Screen]::AllScreens
if ($Index -ge $all.Count) { "screen $Index does not exist ($($all.Count) present)"; exit 2 }
$bounds = $all[$Index].Bounds
"opening a fullscreen probe on screen $Index : $($bounds.Width)x$($bounds.Height) at $($bounds.X),$($bounds.Y)"

$form = New-Object System.Windows.Forms.Form
$form.FormBorderStyle = 'None'
$form.ShowInTaskbar = $false
$form.Text = 'floaty fullscreen probe'
$form.BackColor = [System.Drawing.Color]::FromArgb(20, 20, 24)
$form.Show()
$form.Activate()

$h = $form.Handle
# HWND_TOPMOST = -1, SWP_SHOWWINDOW = 0x40
[void][FsProbe]::SetWindowPos($h, [IntPtr](-1), $bounds.X, $bounds.Y, $bounds.Width, $bounds.Height, 0x40)

# The presence rule reads GetForegroundWindow, so covering the screen is not enough: the
# window has to be *in front*. Windows refuses to let a background process steal the
# foreground, and the documented unlock is a benign ALT tap (VK_MENU 0x12) first. Without
# this the window covers the screen, is not foreground, and the rule correctly decides
# nothing - which looks exactly like the feature being broken.
[FsProbe]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
[FsProbe]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
$got = $false
for ($i = 0; $i -lt 20; $i++) {
  [void][FsProbe]::SetForegroundWindow($h)
  Start-Sleep -Milliseconds 150
  if ([FsProbe]::GetForegroundWindow() -eq $h) { $got = $true; break }
}
"probe open - pid $PID, hwnd $h, foreground $got"
# A status file as well as stdout: when this is launched without a console (from a script),
# stdout can vanish, and the caller still needs to know whether the window really is in
# front - a probe that covers the screen but is not foreground tests nothing.
"pid=$PID screen=$Index foreground=$got hwnd=$h size=$($bounds.Width)x$($bounds.Height) at=$($bounds.X),$($bounds.Y)" |
  Set-Content -Path $status -Encoding ascii

while ($true) { Start-Sleep -Seconds 1 }
