param(
    [Parameter(Mandatory = $true)][string]$HostName,
    [string]$ProfileName = 'Port Forward TUI',
    [string]$Python = '',
    [string]$FocusShortcut = '',
    [switch]$NoTerminalProfile
)
$ErrorActionPreference = 'Stop'
$portsRoot = $PSScriptRoot
. (Join-Path $portsRoot 'python_bootstrap.ps1')
if (-not (Get-Command ssh.exe -ErrorAction SilentlyContinue)) {
    throw 'Install the Windows OpenSSH Client optional feature first.'
}
$portsPython = Initialize-AppPython -Root $portsRoot -Python $Python
& $portsPython -E -s -m pip install -r (Join-Path $portsRoot 'requirements.txt')
if ($LASTEXITCODE -ne 0) { throw 'Dependency installation failed.' }
& $portsPython -E -s (Join-Path $portsRoot 'build_focus_helper.py')
if ($LASTEXITCODE -ne 0) { throw 'Could not build the fast return shortcut helper.' }
& $portsPython -E -s (Join-Path $portsRoot 'app.py') --host $HostName --check
if ($LASTEXITCODE -ne 0) { throw 'Could not configure the SSH host.' }
if (-not $NoTerminalProfile) {
    $portsProfileArguments = @((Join-Path $portsRoot 'terminal_profile.py'), '--name', $ProfileName)
    if ($FocusShortcut) { $portsProfileArguments += @('--focus-shortcut', $FocusShortcut) }
    & $portsPython -E -s @portsProfileArguments
    if ($LASTEXITCODE -ne 0) { throw 'Windows Terminal profile registration failed.' }
}
Write-Output 'Installed. Open the Terminal profile or run .\.venv\Scripts\python.exe app.py'
