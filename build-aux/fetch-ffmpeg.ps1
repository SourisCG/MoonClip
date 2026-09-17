# Fetch the pinned BtbN static FFmpeg for Windows (embedded sidecar).
# End users never install FFmpeg separately (see docs/THIRD_PARTY.md).
# Usage:  pwsh -File build-aux/fetch-ffmpeg.ps1 [-Force]
# Env overrides: FFMPEG_TAG, FFMPEG_ASSET, FFMPEG_SHA256
param([switch]$Force)

$ErrorActionPreference = "Stop"

# Pinned monthly build (retained 2y by BtbN). Bump by updating these three
# + docs/THIRD_PARTY.md, then re-run with -Force.
$Tag   = if ($env:FFMPEG_TAG)   { $env:FFMPEG_TAG }   else { "autobuild-2026-08-31-13-27" }
$Asset = if ($env:FFMPEG_ASSET) { $env:FFMPEG_ASSET } else { "ffmpeg-N-126342-gf88b741dbf-win64-gpl.zip" }
$Sha   = if ($env:FFMPEG_SHA256){ $env:FFMPEG_SHA256 }else { "b4da332540eaebc6939181b59e267f163dd57407ef6596f7f3452845921d1d91" }

$Triple = "x86_64-pc-windows-msvc"
$Root = Split-Path -Parent $PSScriptRoot
$OutDir = Join-Path $Root "src-tauri/binaries/$Triple"
$OutExe = Join-Path $OutDir "ffmpeg-$Triple.exe"

if ((Test-Path $OutExe) -and (-not $Force)) {
  Write-Host "OK (cached): $OutExe"
  & $OutExe -hide_banner -version | Select-Object -First 1
  exit 0
}

New-Item -ItemType Directory -Force $OutDir | Out-Null
$Work = Join-Path ([IO.Path]::GetTempPath()) ("moonclip-ffmpeg-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $Work | Out-Null
try {
  $Url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$Tag/$Asset"
  $Zip = Join-Path $Work $Asset
  Write-Host "==> downloading $Url"
  Invoke-WebRequest -Uri $Url -OutFile $Zip
  Write-Host "==> verifying sha256"
  $Hash = (Get-FileHash -Algorithm SHA256 $Zip).Hash.ToLower()
  if ($Hash -ne $Sha.ToLower()) { throw "sha256 mismatch: got $Hash want $Sha" }
  Write-Host "==> extracting ffmpeg.exe"
  Expand-Archive -LiteralPath $Zip -DestinationPath (Join-Path $Work "unz")
  $Exe = Get-ChildItem -Recurse -Filter "ffmpeg.exe" (Join-Path $Work "unz") |
    Where-Object { $_.FullName -match "\\bin\\ffmpeg\.exe$" } |
    Select-Object -First 1
  if (-not $Exe) { throw "ffmpeg.exe not found inside $Asset" }
  Copy-Item -LiteralPath $Exe.FullName -Destination $OutExe -Force
  Write-Host "==> verifying encoders"
  $Enc = & $OutExe -hide_banner -encoders 2>&1 | Out-String
  foreach ($Need in @("h264_nvenc", "hevc_nvenc", "av1_nvenc", "h264_amf", "hevc_amf", "h264_qsv", "hevc_qsv", "libx264", "aac")) {
    if ($Enc -notmatch [regex]::Escape($Need)) { throw "pinned ffmpeg lacks $Need" }
  }
  Write-Host "==> verifying capture filter (gfxcapture = Windows.Graphics.Capture)"
  # The Windows engine captures through this filter (D3D11 zero-copy); a pin
  # without it breaks the recorder entirely.
  $Filters = & $OutExe -hide_banner -filters 2>&1 | Out-String
  if ($Filters -notmatch "gfxcapture") { throw "pinned ffmpeg lacks the gfxcapture filter" }
  & $OutExe -hide_banner -version | Select-Object -First 1
  Write-Host "OK: $OutExe"
} finally {
  Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
