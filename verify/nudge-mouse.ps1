# Nudge the mouse, to count as real input without clicking anything.
#
# GetLastInputInfo only moves on real input: a programmatic SetCursorPos does not count,
# so waking a machine from "idle" needs an actual input event. A one-pixel move and back
# is the least disruptive one there is.
param([int]$Pixels = 2)

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Nudge {
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
}
"@

$MOVE = 0x0001
[Nudge]::mouse_event($MOVE, $Pixels, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 40
[Nudge]::mouse_event($MOVE, -$Pixels, 0, 0, [UIntPtr]::Zero)
"nudge: moved the pointer by $Pixels px and back"
