<#
.SYNOPSIS
  Build, sign and package Postguy for Windows.

.DESCRIPTION
  Produces, in $env:DIST (default: dist/release):
    postguy-<version>-windows-<arch>.msi       per-machine installer (WiX v5): postguy.exe, postguy-cli.exe,
                                                licences, Start menu shortcut, optional PATH entry
    postguy-<version>-windows-<arch>.zip       portable app: postguy.exe + postguy-cli.exe + licences
    postguy-cli-<version>-windows-<arch>.zip   the headless CLI only, with the licences

  The binaries link the C runtime statically (+crt-static), so neither the MSI nor the zips need the
  Visual C++ redistributable. The .ico for the installer is built here from assets/logo/postguy-*.png
  (nothing binary is committed). Signing is delegated to sign.ps1 (skipped with a warning when no
  signing secrets are set).

  Version: $env:VERSION (set by the release workflow), else $env:POSTGUY_VERSION, else the
  [workspace.package] version in the root Cargo.toml.

  Needs: Rust (MSVC toolchain + the target), the Windows SDK (signtool.exe, only when signing), and
  WiX v5 with the UI extension: dotnet tool install --global wix --version 5.0.2 and
  wix extension add -g WixToolset.UI.wixext/5.0.2

.EXAMPLE
  pwsh packaging/windows/package.ps1 -Arch x64
  pwsh packaging/windows/package.ps1 -Arch x86 -SkipBuild
  pwsh packaging/windows/package.ps1 -Arch arm64     # cross-compiled; needs the MSVC ARM64 build tools
#>
param(
  [ValidateSet('x64', 'x86', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
$Version = if ($env:VERSION) { $env:VERSION } else { $env:POSTGUY_VERSION }
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
$Version = $Version.TrimStart('v')
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$') { throw "version '$Version' is not semver (X.Y.Z[-pre][+build])" }
# MSI ProductVersion is numeric (major.minor.build); pre-release and build tags are dropped there.
$MsiVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3])"

$Target = switch ($Arch) { 'x64' { 'x86_64-pc-windows-msvc' } 'x86' { 'i686-pc-windows-msvc' } 'arm64' { 'aarch64-pc-windows-msvc' } }
$Machine = switch ($Arch) { 'x64' { 0x8664 } 'x86' { 0x14C } 'arm64' { 0xAA64 } }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

# Same build metadata names as packaging/env.sh; the release workflow sets BUILD_SHA / BUILD_DATE.
if (-not $env:POSTGUY_BUILD_SHA) {
  $env:POSTGUY_BUILD_SHA = if ($env:BUILD_SHA) { $env:BUILD_SHA } else { (git -C $Root rev-parse HEAD 2>$null) }
}
if (-not $env:POSTGUY_BUILD_DATE) {
  $env:POSTGUY_BUILD_DATE = if ($env:BUILD_DATE) { $env:BUILD_DATE } else { (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }
}

Write-Output "Postguy $Version (MSI $MsiVersion) for Windows $Arch ($Target)"

if (-not $SkipBuild) {
  # Static CRT: no VC++ redistributable needed. Scoped to the target so host build scripts and
  # proc-macros are unaffected.
  $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
  $existing = [Environment]::GetEnvironmentVariable($flagVar)
  $flags = '-C target-feature=+crt-static'
  if ($existing) { $flags = "$existing $flags" }
  [Environment]::SetEnvironmentVariable($flagVar, $flags)
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p postguy -p postguy-cli --target $Target }
}

$Bin = Join-Path $TargetDir "$Target\release"
foreach ($exe in 'postguy.exe', 'postguy-cli.exe') {
  if (-not (Test-Path -LiteralPath (Join-Path $Bin $exe))) { throw "$exe not found in $Bin (run without -SkipBuild)" }
}

# Check both binaries' PE headers before packaging. Machine (COFF header) must match -Arch, so an
# x64 build can never ship labelled arm64. Subsystem (optional header): 2 = Windows GUI,
# 3 = console. The CLI must stay a console program so its output reaches the terminal; the
# app's subsystem is reported (a console subsystem opens a terminal window behind the GUI).
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}
foreach ($exe in 'postguy.exe', 'postguy-cli.exe') {
  $h = Get-PeHeader (Join-Path $Bin $exe)
  if ($h.Machine -ne $Machine) { throw "$exe is for machine 0x$('{0:X}' -f $h.Machine), expected 0x$('{0:X}' -f $Machine) ($Arch)" }
  if ($exe -eq 'postguy-cli.exe' -and $h.Subsystem -ne 3) { throw "$exe has PE subsystem $($h.Subsystem), expected 3 (console)" }
  Write-Output "ok ${exe}: $Arch, PE subsystem $($h.Subsystem)"
  if ($exe -eq 'postguy.exe' -and $h.Subsystem -ne 2) {
    $msg = 'postguy.exe is a console-subsystem program: Windows opens a terminal window with it. Add #![cfg_attr(windows, windows_subsystem = "windows")] to apps/postguy/src/main.rs.'
    if ($env:GITHUB_ACTIONS) { Write-Output "::warning::$msg" } else { Write-Warning $msg }
  }
}

# ---- staging -----------------------------------------------------------------------------------
# $Work\bin: the executables; $Work\docs: licences and notices (also what the MSI's docs group reads).
$Work = Join-Path $TargetDir "windows-package\$Arch"
$Stage = Join-Path $Work 'bin'
$Docs = Join-Path $Work 'docs'
Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage, $Docs | Out-Null
foreach ($exe in 'postguy.exe', 'postguy-cli.exe') { Copy-Item -LiteralPath (Join-Path $Bin $exe) -Destination $Stage }

# Project licences, notices and the licence of the Poppins font the app embeds (same set as
# copy_docs in packaging/env.sh).
foreach ($f in 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md') {
  $p = Join-Path $Root $f
  if (Test-Path -LiteralPath $p) { Copy-Item -LiteralPath $p -Destination $Docs }
}
$poppins = Join-Path $Root 'assets\fonts\LICENSE-poppins.txt'
if (Test-Path -LiteralPath $poppins) { Copy-Item -LiteralPath $poppins -Destination (Join-Path $Docs 'OFL-Poppins.txt') }
foreach ($f in 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE') {
  if (-not (Test-Path -LiteralPath (Join-Path $Docs $f))) { throw "$f is missing from the repository root" }
}

& (Join-Path $PSScriptRoot 'sign.ps1') (Join-Path $Stage 'postguy.exe') (Join-Path $Stage 'postguy-cli.exe')

# ---- icon: PNG-in-ICO built from the committed logo PNGs ---------------------------------------
# Vista and later read PNG-compressed icon images (including sizes under 256), so the ICO is a
# 6-byte header, 16-byte directory entries and the PNG files themselves.
function New-IcoFromPng([string[]] $PngPaths, [string] $OutPath) {
  $images = @()
  foreach ($p in $PngPaths) {
    $bytes = [System.IO.File]::ReadAllBytes($p)
    # PNG signature (8) + IHDR length/type (8), then big-endian width and height.
    if ($bytes.Length -lt 24 -or $bytes[0] -ne 0x89 -or $bytes[1] -ne 0x50) { throw "$p is not a PNG" }
    $w = ($bytes[16] -shl 24) -bor ($bytes[17] -shl 16) -bor ($bytes[18] -shl 8) -bor $bytes[19]
    $h = ($bytes[20] -shl 24) -bor ($bytes[21] -shl 16) -bor ($bytes[22] -shl 8) -bor $bytes[23]
    if ($w -lt 1 -or $w -gt 256 -or $h -lt 1 -or $h -gt 256) { throw "$p is ${w}x${h}; ICO images are at most 256x256" }
    $images += , @{ Width = $w; Height = $h; Bytes = $bytes }
  }
  $stream = New-Object System.IO.MemoryStream
  $writer = New-Object System.IO.BinaryWriter($stream)
  try {
    $writer.Write([uint16] 0)               # reserved
    $writer.Write([uint16] 1)               # type: icon
    $writer.Write([uint16] $images.Count)
    $offset = 6 + 16 * $images.Count
    foreach ($img in $images) {
      $writer.Write([byte] $(if ($img.Width -ge 256) { 0 } else { $img.Width }))
      $writer.Write([byte] $(if ($img.Height -ge 256) { 0 } else { $img.Height }))
      $writer.Write([byte] 0)               # palette colours
      $writer.Write([byte] 0)               # reserved
      $writer.Write([uint16] 1)             # colour planes
      $writer.Write([uint16] 32)            # bits per pixel
      $writer.Write([uint32] $img.Bytes.Length)
      $writer.Write([uint32] $offset)
      $offset += $img.Bytes.Length
    }
    foreach ($img in $images) { $writer.Write([byte[]] $img.Bytes) }
    $writer.Flush()
    [System.IO.File]::WriteAllBytes($OutPath, $stream.ToArray())
  } finally {
    $writer.Dispose()
    $stream.Dispose()
  }
}
$Icon = Join-Path $Work 'postguy.ico'
$pngs = foreach ($size in 16, 32, 64, 128, 256) {
  $p = Join-Path $Root "assets\logo\postguy-$size.png"
  if (-not (Test-Path -LiteralPath $p)) { throw "$p not found" }
  $p
}
New-IcoFromPng $pngs $Icon

# ---- licence text for the installer's licence page ---------------------------------------------
# WixUI shows an RTF file. Build one from the dual-licence statement plus the MIT text; the full
# Apache-2.0 text and the notices are installed beside the program.
function ConvertTo-Rtf([string] $Text) {
  $sb = New-Object System.Text.StringBuilder
  [void] $sb.Append('{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\f0\fs20 ')
  foreach ($ch in $Text.ToCharArray()) {
    $code = [int] $ch
    if ($ch -eq '\') { [void] $sb.Append('\\') }
    elseif ($ch -eq '{') { [void] $sb.Append('\{') }
    elseif ($ch -eq '}') { [void] $sb.Append('\}') }
    elseif ($ch -eq "`n") { [void] $sb.Append('\par ') }
    elseif ($ch -eq "`r") { }
    elseif ($code -gt 126) {
      # RTF \uN takes a signed 16-bit value, followed by a one-character ASCII fallback.
      $signed = if ($code -gt 32767) { $code - 65536 } else { $code }
      [void] $sb.Append('\u' + $signed + '?')
    }
    else { [void] $sb.Append($ch) }
  }
  [void] $sb.Append('}')
  return $sb.ToString()
}
$licenseText = "Postguy is dual-licensed under the MIT license or the Apache License 2.0, at your option. The MIT license text follows; the Apache-2.0 text (LICENSE-APACHE) and third-party notices (NOTICE) are installed with the program.`n`n" +
  (Get-Content -Raw -LiteralPath (Join-Path $Docs 'LICENSE-MIT'))
$LicenseRtf = Join-Path $Work 'license.rtf'
[System.IO.File]::WriteAllText($LicenseRtf, (ConvertTo-Rtf $licenseText), [System.Text.Encoding]::ASCII)

# ---- MSI ---------------------------------------------------------------------------------------
$Msi = Join-Path $Dist "postguy-$Version-windows-$Arch.msi"
Remove-Item -Force -ErrorAction SilentlyContinue $Msi
Invoke-Native 'wix build' {
  wix build (Join-Path $PSScriptRoot 'postguy.wxs') -ext WixToolset.UI.wixext -arch $Arch `
    -d "Version=$MsiVersion" -d "BinDir=$Stage" -d "DocsDir=$Docs" -d "IconPath=$Icon" -d "LicenseRtf=$LicenseRtf" `
    -o $Msi
}
# wix writes its debug symbols (.wixpdb) next to the MSI; keep them out of the release assets.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($Msi, '.wixpdb'))

# Install, check and uninstall the MSI when this machine can run the build. An ARM64 build made
# on an x64 runner can't run here; the ARM64 verification workflow does it on ARM64 hardware.
$HostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$CanRun = ($Arch -ne 'arm64') -or ($HostArch -eq 'arm64')
if ($CanRun) {
  # In a child process, so its handles are gone before signtool opens the MSI.
  Invoke-Native 'test-msi' { & (Get-Process -Id $PID).Path -NoProfile -File (Join-Path $PSScriptRoot 'test-msi.ps1') $Msi }
} else {
  Write-Output "skipping test-msi: an $Arch build doesn't run on this $HostArch machine"
}
& (Join-Path $PSScriptRoot 'sign.ps1') $Msi

# ---- zips --------------------------------------------------------------------------------------
function New-ZipPackage([string] $Name, [string[]] $Exes) {
  $dir = Join-Path $Work $Name
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  foreach ($exe in $Exes) { Copy-Item -LiteralPath (Join-Path $Stage $exe) -Destination $dir }
  Copy-Item -Path (Join-Path $Docs '*') -Destination $dir
  $zip = Join-Path $Dist "$Name.zip"
  Remove-Item -Force $zip -ErrorAction SilentlyContinue
  Compress-Archive -Path $dir -DestinationPath $zip
  return $zip
}
$AppZip = New-ZipPackage "postguy-$Version-windows-$Arch" @('postguy.exe', 'postguy-cli.exe')
$CliZip = New-ZipPackage "postguy-cli-$Version-windows-$Arch" @('postguy-cli.exe')

# Smoke-test the CLI when this machine can run it.
if ($CanRun) {
  Invoke-Native 'postguy-cli tools' { & (Join-Path $Stage 'postguy-cli.exe') tools | Out-Null }
} else {
  Write-Output "skipping postguy-cli tools: an $Arch build doesn't run on this $HostArch machine"
}
Get-Item $Msi, $AppZip, $CliZip | Format-Table Name, Length
