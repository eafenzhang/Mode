param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][string]$Ico
)

$ErrorActionPreference = "Stop"

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class PeIcon {
  [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  public static extern IntPtr BeginUpdateResource(string fileName, bool deleteExistingResources);
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern bool UpdateResource(IntPtr hUpdate, IntPtr lpType, IntPtr lpName, ushort wLanguage, byte[] lpData, uint cbData);
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern bool EndUpdateResource(IntPtr hUpdate, bool fDiscard);
}
"@

if (-not (Test-Path -LiteralPath $Exe)) { throw "exe not found: $Exe" }
if (-not (Test-Path -LiteralPath $Ico)) { throw "ico not found: $Ico" }

$bytes = [IO.File]::ReadAllBytes($Ico)
if ($bytes.Length -lt 6) { throw "invalid ico: $Ico" }
if ([BitConverter]::ToUInt16($bytes, 0) -ne 0 -or [BitConverter]::ToUInt16($bytes, 2) -ne 1) {
  throw "not an ico file: $Ico"
}
$count = [BitConverter]::ToUInt16($bytes, 4)
if ($count -lt 1 -or (6 + $count * 16) -gt $bytes.Length) { throw "invalid ico directory: $Ico" }

$entries = New-Object System.Collections.ArrayList
for ($i = 0; $i -lt $count; $i++) {
  $offset = 6 + $i * 16
  $size = [BitConverter]::ToUInt32($bytes, $offset + 8)
  $imageOffset = [BitConverter]::ToUInt32($bytes, $offset + 12)
  if ($size -le 0 -or ($imageOffset + $size) -gt $bytes.Length) { throw "invalid ico entry $i" }
  $image = New-Object byte[] $size
  [Array]::Copy($bytes, $imageOffset, $image, 0, $size)
  [void]$entries.Add([pscustomobject]@{
    Width      = $bytes[$offset]
    Height     = $bytes[$offset + 1]
    ColorCount = $bytes[$offset + 2]
    Reserved   = $bytes[$offset + 3]
    Planes     = [BitConverter]::ToUInt16($bytes, $offset + 4)
    BitCount   = [BitConverter]::ToUInt16($bytes, $offset + 6)
    Size       = $size
    Image      = $image
  })
}

$handle = [PeIcon]::BeginUpdateResource($Exe, $false)
if ($handle -eq [IntPtr]::Zero) {
  throw "BeginUpdateResource failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
}
try {
  for ($i = 0; $i -lt $entries.Count; $i++) {
    $entry = $entries[$i]
    $ok = [PeIcon]::UpdateResource($handle, [IntPtr]3, [IntPtr]($i + 1), 0, $entry.Image, [uint32]$entry.Size)
    if (-not $ok) { throw "UpdateResource RT_ICON failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
  }

  $group = New-Object IO.MemoryStream
  $writer = New-Object IO.BinaryWriter($group)
  try {
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$entries.Count)
    for ($i = 0; $i -lt $entries.Count; $i++) {
      $entry = $entries[$i]
      $writer.Write([byte]$entry.Width)
      $writer.Write([byte]$entry.Height)
      $writer.Write([byte]$entry.ColorCount)
      $writer.Write([byte]$entry.Reserved)
      $writer.Write([uint16]$entry.Planes)
      $writer.Write([uint16]$entry.BitCount)
      $writer.Write([uint32]$entry.Size)
      $writer.Write([uint16]($i + 1))
    }
    $groupData = $group.ToArray()
  } finally {
    $writer.Dispose()
    $group.Dispose()
  }

  $ok = [PeIcon]::UpdateResource($handle, [IntPtr]14, [IntPtr]1, 0, $groupData, [uint32]$groupData.Length)
  if (-not $ok) { throw "UpdateResource RT_GROUP_ICON failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
  if (-not [PeIcon]::EndUpdateResource($handle, $false)) {
    throw "EndUpdateResource failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
  }
} catch {
  [void][PeIcon]::EndUpdateResource($handle, $true)
  throw
}

Write-Output "icon=$Ico -> $Exe ($count images)"
