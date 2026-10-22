param(
  [string]$Version = "0.1.0",
  [string]$XpGo = "",
  [string]$ModernGo = "",
  [string]$OutDir = "dist",
  [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
Push-Location $PSScriptRoot
try {
  $repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
  if (-not $XpGo) {
    $candidate = Join-Path $repoRoot ".tools\go\bin\go.exe"
    if (Test-Path $candidate) { $XpGo = $candidate } else { $XpGo = "go" }
  }
  if (-not $ModernGo) {
    $candidate = Join-Path $repoRoot ".tools\go-modern\go\bin\go.exe"
    if (Test-Path $candidate) { $ModernGo = $candidate } else { $ModernGo = "go" }
  }
  New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

  $savedGOOS = $env:GOOS
  $savedGOARCH = $env:GOARCH
  $savedCGO = $env:CGO_ENABLED
  $savedModule = $env:GO111MODULE
  $icon = Join-Path $PSScriptRoot "assets\mode-agent.ico"
  $iconScript = Join-Path $PSScriptRoot "set-icon.ps1"

  function Set-AppIcon([string]$Path, [string]$Description) {
    if (-not (Test-Path $Path)) {
      Write-Warning "executable not found; skipping icon for $Path"
      return
    }
    try {
      & powershell -NoProfile -ExecutionPolicy Bypass -File $iconScript -Exe $Path -Ico $icon | Out-Null
      if ($LASTEXITCODE -ne 0) { throw "icon updater exited $LASTEXITCODE" }
      Write-Host "icon: $Path ($Description)"
    } catch {
      Write-Warning "icon injection failed for ${Path}: $_"
    }
  }

  function Restore-GoEnv {
    if ($null -eq $savedGOOS) { Remove-Item Env:GOOS -ErrorAction SilentlyContinue } else { $env:GOOS = $savedGOOS }
    if ($null -eq $savedGOARCH) { Remove-Item Env:GOARCH -ErrorAction SilentlyContinue } else { $env:GOARCH = $savedGOARCH }
    if ($null -eq $savedCGO) { Remove-Item Env:CGO_ENABLED -ErrorAction SilentlyContinue } else { $env:CGO_ENABLED = $savedCGO }
    if ($null -eq $savedModule) { Remove-Item Env:GO111MODULE -ErrorAction SilentlyContinue } else { $env:GO111MODULE = $savedModule }
  }

  function Invoke-Go([string]$Go, [string]$GoOs, [string]$GoArch, [string]$Output, [string[]]$ExtraArgs) {
    try {
      $env:GOOS = $GoOs
      $env:GOARCH = $GoArch
      $env:CGO_ENABLED = "0"
      $ldflags = "-s -w -X main.version=$Version"
      if ($GoOs -eq "windows") { $ldflags = "$ldflags -H windowsgui" }
      & $Go @ExtraArgs build -ldflags $ldflags -o (Join-Path $OutDir $Output) .
      if ($LASTEXITCODE -ne 0) { throw "build failed: $GoOs/$GoArch" }
    } finally {
      Restore-GoEnv
    }
  }

  Write-Host "XP toolchain: $XpGo"
  & $XpGo version
  if ($LASTEXITCODE -ne 0) { throw "XP Go toolchain is not runnable" }

  $env:GO111MODULE = "off"
  Invoke-Go $XpGo "windows" "386" "mode-agent-xp-x86.exe" @()
  Write-Host "WARNING: mode-agent-xp-x86.exe embeds Go 1.10 (EOL). Use it only on a trusted LAN or VPN."

  if (-not $SkipTests) {
    try {
      $env:GOOS = "windows"
      $env:GOARCH = "386"
      $env:CGO_ENABLED = "0"
      $env:GO111MODULE = "off"
      & $XpGo test .
      if ($LASTEXITCODE -ne 0) { throw "Go 1.10 tests failed" }
    } finally {
      Restore-GoEnv
    }
  }
  Remove-Item Env:GO111MODULE -ErrorAction SilentlyContinue

  Set-AppIcon (Join-Path $OutDir "mode-agent-xp-x86.exe") "Mode Agent remote runtime"

  Write-Host "Modern toolchain: $ModernGo"
  & $ModernGo version
  if ($LASTEXITCODE -eq 0) {
    Invoke-Go $ModernGo "windows" "amd64" "mode-agent-win-x64.exe" @()
    Invoke-Go $ModernGo "windows" "386" "mode-agent-win-x86.exe" @()
    Invoke-Go $ModernGo "linux" "amd64" "mode-remote-linux-amd64" @()
    Invoke-Go $ModernGo "linux" "arm64" "mode-remote-linux-arm64" @()
    Invoke-Go $ModernGo "darwin" "amd64" "mode-remote-darwin-amd64" @()
    Invoke-Go $ModernGo "darwin" "arm64" "mode-remote-darwin-arm64" @()
    Set-AppIcon (Join-Path $OutDir "mode-agent-win-x64.exe") "Mode Agent remote runtime"
    Set-AppIcon (Join-Path $OutDir "mode-agent-win-x86.exe") "Mode Agent remote runtime"
    if (-not $SkipTests) {
      Restore-GoEnv
      & $ModernGo test .
      if ($LASTEXITCODE -ne 0) { throw "modern Go tests failed" }
    }
  } else {
    Write-Warning "Modern Go toolchain not available; skipped modern Windows and client builds."
  }

  $bootstrap = Join-Path $OutDir "mode-agent-setup-bootstrap.exe"
  try {
    $env:GOOS = "windows"
    $env:GOARCH = "386"
    $env:CGO_ENABLED = "0"
    $env:GO111MODULE = "off"
    & $XpGo build -ldflags "-s -w -H windowsgui" -o $bootstrap ./setup
    if ($LASTEXITCODE -ne 0) { throw "bootstrap build failed" }
  } finally {
    Restore-GoEnv
  }
  Set-AppIcon $bootstrap "Mode Agent installer"
  $installer = Join-Path $OutDir "ModeAgent-Setup.exe"
  & $ModernGo run ./setup/pack/main.go `
    -bootstrap $bootstrap `
    -x86 (Join-Path $OutDir "mode-agent-xp-x86.exe") `
    -x64 (Join-Path $OutDir "mode-agent-win-x64.exe") `
    -out $installer
  if ($LASTEXITCODE -ne 0) { throw "single installer packaging failed" }
  Remove-Item $bootstrap -Force
  Remove-Item (Join-Path $OutDir "ModeAgent-Setup-x86.exe") -ErrorAction SilentlyContinue
  Remove-Item (Join-Path $OutDir "ModeAgent-Setup-x64.exe") -ErrorAction SilentlyContinue
  Get-ChildItem $OutDir | Sort-Object Name | Format-Table Name, Length
} finally {
  Pop-Location
}
