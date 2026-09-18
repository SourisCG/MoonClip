# Fetch the pinned embedded OBS Studio + obs-cmd for Windows (MoonClip V3
# capture engine). End users never install OBS separately (see
# docs/THIRD_PARTY.md).
# Usage:  pwsh -File build-aux/fetch-obs.ps1 [-Force]
# Env overrides: OBS_VERSION, OBS_ASSET, OBS_SHA256,
#                OBSCMD_VERSION, OBSCMD_ASSET, OBSCMD_SHA256
param([switch]$Force)

$ErrorActionPreference = "Stop"

# --- Pinned OBS Studio portable zip (official GitHub release) ---------------
$ObsVersion = if ($env:OBS_VERSION) { $env:OBS_VERSION } else { "32.2.2" }
$ObsAsset   = if ($env:OBS_ASSET)   { $env:OBS_ASSET }   else { "OBS-Studio-32.2.2-Windows-x64.zip" }
$ObsSha     = if ($env:OBS_SHA256)  { $env:OBS_SHA256 }  else { "4d6e40e3ab155f56b30de517380566a206d74b63cdf5ad49aa596924768f97e1" }

# --- Pinned obs-cmd (the replay control CLI) --------------------------------
$CmdVersion = if ($env:OBSCMD_VERSION) { $env:OBSCMD_VERSION } else { "v1.0.2" }
$CmdAsset   = if ($env:OBSCMD_ASSET)   { $env:OBSCMD_ASSET }   else { "obs-cmd-x64-windows.tar.gz" }
$CmdSha     = if ($env:OBSCMD_SHA256)  { $env:OBSCMD_SHA256 }  else { "5cfc474f15e851d5323d94a705212c6d1f181a02bd23cfacfb7e7900613c9001" }

$Triple = "x86_64-pc-windows-msvc"
$Root = Split-Path -Parent $PSScriptRoot
$OutDir = Join-Path $Root "src-tauri/binaries/$Triple"
$ObsRoot = Join-Path $OutDir "obs"
$ObsExe = Join-Path $ObsRoot "bin/64bit/obs64.exe"
$CmdExe = Join-Path $OutDir "obs-cmd.exe"

if ((Test-Path $ObsExe) -and (Test-Path $CmdExe) -and (-not $Force)) {
  Write-Host "OK (cached): $ObsExe"
  Write-Host "OK (cached): $CmdExe"
  exit 0
}

New-Item -ItemType Directory -Force $OutDir | Out-Null
$Work = Join-Path ([IO.Path]::GetTempPath()) ("moonclip-obs-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $Work | Out-Null
try {
  # --- OBS Studio -----------------------------------------------------------
  if (-not (Test-Path $ObsExe)) {
    $Url = "https://github.com/obsproject/obs-studio/releases/download/$ObsVersion/$ObsAsset"
    $Zip = Join-Path $Work $ObsAsset
    Write-Host "==> downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $Zip
    Write-Host "==> verifying sha256"
    $Hash = (Get-FileHash -Algorithm SHA256 $Zip).Hash.ToLower()
    if ($ObsSha -and ($Hash -ne $ObsSha.ToLower())) {
      throw "sha256 mismatch: got $Hash want $ObsSha"
    }
    Write-Host "==> extracting OBS"
    $Unz = Join-Path $Work "obs-unz"
    Expand-Archive -LiteralPath $Zip -DestinationPath $Unz
    $Exe = Get-ChildItem -Recurse -Filter "obs64.exe" $Unz |
      Where-Object { $_.FullName -match "\\bin\\64bit\\obs64\.exe$" } |
      Select-Object -First 1
    if (-not $Exe) { throw "obs64.exe not found inside $ObsAsset" }
    # Root = the directory that contains bin/ (portable layout).
    $SrcRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $Exe.FullName))
    if (Test-Path $ObsRoot) { Remove-Item -Recurse -Force $ObsRoot }
    New-Item -ItemType Directory -Force $ObsRoot | Out-Null
    Copy-Item -Path (Join-Path $SrcRoot "*") -Destination $ObsRoot -Recurse -Force
    Write-Host "==> verifying OBS build"
    if (-not (Test-Path $ObsExe)) { throw "OBS layout unexpected after extraction" }
    # NOTE: the script never executes OBS. Runtime validation happens through
    # MoonClip, which launches a writable portable copy (portable_mode.txt) —
    # never the user's own OBS install/config.
  }

  # --- obs-cmd --------------------------------------------------------------
  if (-not (Test-Path $CmdExe)) {
    $Url = "https://github.com/grigio/obs-cmd/releases/download/$CmdVersion/$CmdAsset"
    $Tgz = Join-Path $Work $CmdAsset
    Write-Host "==> downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $Tgz
    Write-Host "==> verifying sha256"
    $Hash = (Get-FileHash -Algorithm SHA256 $Tgz).Hash.ToLower()
    if ($CmdSha -and ($Hash -ne $CmdSha.ToLower())) {
      throw "sha256 mismatch: got $Hash want $CmdSha"
    }
    Write-Host "==> extracting obs-cmd"
    $Unz = Join-Path $Work "cmd-unz"
    New-Item -ItemType Directory -Force $Unz | Out-Null
    tar -xzf $Tgz -C $Unz
    $Bin = Get-ChildItem -Recurse -File $Unz |
      Where-Object { $_.Name -match "^obs-cmd(\.exe)?$" } |
      Select-Object -First 1
    if (-not $Bin) { throw "obs-cmd binary not found inside $CmdAsset" }
    Copy-Item -LiteralPath $Bin.FullName -Destination $CmdExe -Force
  }
  Write-Host "OK: $ObsRoot"
  Write-Host "OK: $CmdExe"
} finally {
  Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
