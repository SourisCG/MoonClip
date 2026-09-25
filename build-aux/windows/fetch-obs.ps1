# Fetch the pinned embedded capture engine for Windows (MoonClip V3).
# The upstream zip is staged under a neutral binary name; end users never
# install anything separately (see docs/THIRD_PARTY.md).
# Control happens over obs-websocket v5 from MoonClip itself (`obws` crate);
# no obs-cmd CLI is shipped anymore.
# Usage:  pwsh -File build-aux/windows/fetch-obs.ps1 [-Force]
# Env overrides: OBS_VERSION, OBS_ASSET, OBS_SHA256
param([switch]$Force)

$ErrorActionPreference = "Stop"

# --- Pinned OBS Studio portable zip (official GitHub release) ---------------
$ObsVersion = if ($env:OBS_VERSION) { $env:OBS_VERSION } else { "32.2.2" }
$ObsAsset   = if ($env:OBS_ASSET)   { $env:OBS_ASSET }   else { "OBS-Studio-32.2.2-Windows-x64.zip" }
$ObsSha     = if ($env:OBS_SHA256)  { $env:OBS_SHA256 }  else { "4d6e40e3ab155f56b30de517380566a206d74b63cdf5ad49aa596924768f97e1" }

$Triple = "x86_64-pc-windows-msvc"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$OutDir = Join-Path $Root "src-tauri/binaries/$Triple"
$ObsRoot = Join-Path $OutDir "engine"
$ObsExe = Join-Path $ObsRoot "bin/64bit/moonclip-engine.exe"

if ((Test-Path $ObsExe) -and (-not $Force)) {
  Write-Host "OK (cached): $ObsExe"
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
    # Identity: rename the launcher so Task Manager (and other apps) never
    # see the upstream product name; all lookups are directory-relative.
    $RawExe = Join-Path $ObsRoot "bin/64bit/obs64.exe"
    if (Test-Path $RawExe) { Move-Item -Force $RawExe $ObsExe }
    Write-Host "==> verifying engine build"
    if (-not (Test-Path $ObsExe)) { throw "engine layout unexpected after extraction" }
    # NOTE: the script never executes OBS. Runtime validation happens through
    # MoonClip, which launches a writable portable copy (portable_mode.txt) —
    # never the user's own OBS install/config.
  }

  Write-Host "OK: $ObsRoot"
} finally {
  Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
