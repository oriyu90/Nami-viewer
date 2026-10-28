# Builds the portable Windows zip for NamiVision.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Get-Content Cargo.toml | Select-String '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
Write-Host "Building NamiVision $version (release)..."

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$exe = Join-Path $targetDir 'release\nami-vision.exe'
if (-not (Test-Path $exe)) { throw "missing $exe" }

$stageName = "NamiVision_${version}_windows-x64"
$stage = Join-Path $root "dist\$stageName"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
New-Item -ItemType Directory -Force (Join-Path $stage 'docs') | Out-Null

Copy-Item $exe (Join-Path $stage 'NamiVision.exe')
Copy-Item README.md, README.ja.md, LICENSE.txt, THIRD_PARTY_NOTICES.md $stage
Get-ChildItem 'docs' -Filter '*.md' |
  Where-Object { $_.Name -match '^(01|02|03)_' } |
  Copy-Item -Destination (Join-Path $stage 'docs')

# Portable marker so settings live next to the executable by default.
Set-Content -Path (Join-Path $stage 'portable.txt') -Value 'NamiVision portable mode' -Encoding UTF8

$zip = Join-Path $root "dist\$stageName.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

# Create entries manually so every name uses forward slashes (the zip
# standard) instead of the backslashes .NET Framework emits on Windows.
$zipStream = [System.IO.File]::Open($zip, [System.IO.FileMode]::Create)
$archive = New-Object System.IO.Compression.ZipArchive(
  $zipStream, [System.IO.Compression.ZipArchiveMode]::Create, $false)
try {
  Get-ChildItem $stage -Recurse -File | ForEach-Object {
    $rel = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
    $entry = $archive.CreateEntry(
      "$stageName/$rel", [System.IO.Compression.CompressionLevel]::Optimal)
    $entry.LastWriteTime = $_.LastWriteTime
    $input = [System.IO.File]::OpenRead($_.FullName)
    $output = $entry.Open()
    try { $input.CopyTo($output) } finally { $output.Dispose(); $input.Dispose() }
  }
} finally {
  $archive.Dispose()
  $zipStream.Dispose()
}

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $stageName.zip" | Set-Content "$zip.sha256" -Encoding ASCII

$size = [math]::Round((Get-Item $zip).Length / 1MB, 2)
Write-Host "Created $zip ($size MB)"
Write-Host "SHA256 $hash"

# Verify the archive is readable and contains the expected top-level entries.
$archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
try {
  $names = $archive.Entries | ForEach-Object { $_.FullName }
  Write-Host "Entries: $($names.Count)"
  $names | ForEach-Object { Write-Host "  $_" }
  if (-not ($names -contains "$stageName/NamiVision.exe")) { throw "zip verification failed" }
} finally {
  $archive.Dispose()
}
