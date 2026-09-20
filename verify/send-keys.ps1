# Send a real key combination to Windows, for testing global shortcuts.
#
# A CDP-dispatched key event reaches the webview, not the system, so a *global* shortcut
# (registered with the OS) never fires from one: it needs real input. keybd_event is
# deprecated but it is the whole mechanism this needs, and it needs no struct marshalling.
#
# Usage: powershell -File send-keys.ps1 "ctrl+alt+z"
param([Parameter(Mandatory = $true)][string]$Combo)

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class SendKeys2 {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
"@

$map = @{
  ctrl = 0x11; control = 0x11
  alt = 0x12; shift = 0x10; win = 0x5B
  z = 0x5A; v = 0x56; space = 0x20; a = 0x41
}

$names = $Combo.ToLower().Split('+') | ForEach-Object { $_.Trim() } | Where-Object { $_ }
$keys = @()
foreach ($name in $names) {
  if (-not $map.ContainsKey($name)) { "send-keys: unknown key '$name' in '$Combo'"; exit 2 }
  $keys += [byte]$map[$name]
}
if ($keys.Count -eq 0) { "send-keys: nothing to send"; exit 2 }

foreach ($k in $keys) { [SendKeys2]::keybd_event($k, 0, 0, [UIntPtr]::Zero) }
Start-Sleep -Milliseconds 60
[array]::Reverse($keys)
foreach ($k in $keys) { [SendKeys2]::keybd_event($k, 0, 2, [UIntPtr]::Zero) }
"send-keys: sent $Combo"
