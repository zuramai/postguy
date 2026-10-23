<#
.SYNOPSIS
  Install a Postguy MSI silently, check what it put on the machine, run the CLI, and uninstall it.

.DESCRIPTION
  Needs an elevated shell (the package is per-machine) and a machine that can run the MSI's
  architecture. package.ps1 runs it for x64 and x86 builds; the ARM64 verification workflow runs it
  on ARM64 hardware. Installs into a temporary INSTALLFOLDER so a developer's real install is left alone.
  The log of each msiexec run is written next to the MSI's temp folder and printed when a step fails.

.EXAMPLE
  pwsh packaging/windows/test-msi.ps1 dist/release/postguy-0.1.0-windows-x64.msi
#>
param([Parameter(Mandatory)] [string] $Path)
$ErrorActionPreference = 'Stop'
$Msi = (Resolve-Path -LiteralPath $Path).Path
$Tmp = Join-Path ([IO.Path]::GetTempPath()) "postguy-test-msi-$PID"
$Install = Join-Path $Tmp 'Postguy'
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null

function Invoke-Msiexec([string] $What, [string] $Arguments, [string] $Log) {
  Write-Output "==> $What"
  $proc = Start-Process -FilePath 'msiexec.exe' -ArgumentList $Arguments -Wait -PassThru
  # 0 = success, 3010 = success, restart required.
  if ($proc.ExitCode -ne 0 -and $proc.ExitCode -ne 3010) {
    if (Test-Path -LiteralPath $Log) { Get-Content -LiteralPath $Log -Tail 80 }
    throw "$What failed with exit code $($proc.ExitCode)"
  }
}

function Assert-True([bool] $Condition, [string] $What) {
  if (-not $Condition) { throw "check failed: $What" }
  Write-Output "ok $What"
}

function Get-MachinePath { return [Environment]::GetEnvironmentVariable('Path', 'Machine') }

$installed = $false
try {
  $installLog = Join-Path $Tmp 'install.log'
  Invoke-Msiexec 'msiexec install' "/i `"$Msi`" /qn /norestart /l*v `"$installLog`" INSTALLFOLDER=`"$Install`"" $installLog
  $installed = $true

  foreach ($name in 'postguy.exe', 'postguy-cli.exe', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE') {
    Assert-True (Test-Path -LiteralPath (Join-Path $Install $name)) "installed $name"
  }
  $startMenu = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Postguy.lnk'
  Assert-True (Test-Path -LiteralPath $startMenu) 'Start menu shortcut'
  $onPath = (Get-MachinePath).Split(';') | Where-Object { $_.TrimEnd('\') -eq $Install.TrimEnd('\') }
  Assert-True ([bool] $onPath) 'install folder is on the machine PATH (CliPath feature)'

  $cli = Join-Path $Install 'postguy-cli.exe'
  # The CLI has no --version flag; `tools` prints the automation tool table and exits 0.
  Write-Output '==> postguy-cli tools'
  $tools = & $cli tools
  if ($LASTEXITCODE -ne 0) { throw "postguy-cli tools failed with exit code $LASTEXITCODE" }
  Assert-True (($tools | Measure-Object -Line).Lines -gt 0) 'postguy-cli tools lists tools'

  $uninstallLog = Join-Path $Tmp 'uninstall.log'
  Invoke-Msiexec 'msiexec uninstall' "/x `"$Msi`" /qn /norestart /l*v `"$uninstallLog`"" $uninstallLog
  $installed = $false
  Assert-True (-not (Test-Path -LiteralPath (Join-Path $Install 'postguy.exe'))) 'postguy.exe removed'
  Assert-True (-not (Test-Path -LiteralPath (Join-Path $Install 'postguy-cli.exe'))) 'postguy-cli.exe removed'
  Assert-True (-not (Test-Path -LiteralPath $startMenu)) 'Start menu shortcut removed'
  $stillOnPath = (Get-MachinePath).Split(';') | Where-Object { $_.TrimEnd('\') -eq $Install.TrimEnd('\') }
  Assert-True (-not [bool] $stillOnPath) 'install folder removed from the machine PATH'
}
finally {
  if ($installed) {
    # A failed check must not leave a test install (and its PATH entry) on the machine.
    Start-Process -FilePath 'msiexec.exe' -ArgumentList "/x `"$Msi`" /qn /norestart" -Wait | Out-Null
  }
  Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
Write-Output "MSI test passed: $Msi"
