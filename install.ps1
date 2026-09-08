param(
    [string]$HostName = '',
    [string]$ProfileName = 'Port Forward TUI',
    [string]$Python = '',
    [string]$FocusShortcut = '',
    [switch]$NoTerminalProfile,
    [switch]$NonInteractive
)
$ErrorActionPreference = 'Stop'
$portsRoot = $PSScriptRoot
. (Join-Path $portsRoot 'scripts\python_bootstrap.ps1')
. (Join-Path $portsRoot 'scripts\setup_helpers.ps1')
$HostName = Get-SetupHost -Value $HostName -SettingsPath (Join-Path $env:LOCALAPPDATA 'PortForwardTUI\forwards.json') -NonInteractive:$NonInteractive
Write-Host '1/4 Checking this computer and preparing a private Python environment...'
if (-not (Get-Command ssh.exe -ErrorAction SilentlyContinue)) {
    throw 'Install the Windows OpenSSH Client optional feature first.'
}
$portsPython = Initialize-AppPython -Root $portsRoot -Python $Python
Write-Host '2/4 Installing the screen interface and preparing the shortcut...'
& $portsPython -E -s -m pip install --no-input -r (Join-Path $portsRoot 'requirements.txt')
if ($LASTEXITCODE -ne 0) { throw 'Dependency installation failed.' }
& $portsPython -E -s (Join-Path $portsRoot 'port_forward_tui\build_focus_helper.py')
if ($LASTEXITCODE -ne 0) { throw 'Could not build the fast return shortcut helper.' }
Write-Host '3/4 Saving your remote computer. No SSH connection is opened during setup.'
& $portsPython -E -s (Join-Path $portsRoot 'app.py') --host $HostName --check
if ($LASTEXITCODE -ne 0) { throw 'Could not configure the SSH host.' }
if (-not $NoTerminalProfile) {
    Write-Host '4/4 Adding the app to the Windows Terminal dropdown...'
    $portsProfileArguments = @((Join-Path $portsRoot 'port_forward_tui\terminal_profile.py'), '--name', $ProfileName)
    if ($FocusShortcut) { $portsProfileArguments += @('--focus-shortcut', $FocusShortcut) }
    & $portsPython -E -s @portsProfileArguments
    if ($LASTEXITCODE -ne 0) { throw 'Windows Terminal profile registration failed.' }
}
Write-Output 'Ready. Open Port Forward TUI in the Terminal dropdown, or run .\.venv\Scripts\python.exe -E -s app.py'
Write-Output 'Press A for an add-connection form. Enter the port of your remote app, such as 8000.'
Write-Output 'Need help? Run .\doctor.ps1. Agents and scripts can use .\ports.ps1 --help.'
