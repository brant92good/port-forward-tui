param(
    [Parameter(Mandatory = $true)][string]$HostName,
    [string]$ProfileName = 'Port Forward TUI',
    [string]$Python = 'python',
    [string]$FocusShortcut = '',
    [switch]$NoTerminalProfile
)
$ErrorActionPreference = 'Stop'
$portsRoot = $PSScriptRoot
$portsPython = Join-Path $portsRoot '.venv\Scripts\python.exe'
if (-not (Get-Command ssh.exe -ErrorAction SilentlyContinue)) {
    throw 'Install the Windows OpenSSH Client optional feature first.'
}
if (-not (Test-Path -LiteralPath $portsPython)) {
    & $Python -m venv (Join-Path $portsRoot '.venv')
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the environment. Use Python 3.12 or newer.' }
}
& $portsPython -m pip install -r (Join-Path $portsRoot 'requirements.txt')
if ($LASTEXITCODE -ne 0) { throw 'Dependency installation failed.' }
& $portsPython (Join-Path $portsRoot 'app.py') --host $HostName --check
if ($LASTEXITCODE -ne 0) { throw 'Could not configure the SSH host.' }
if (-not $NoTerminalProfile) {
    $portsProfileArguments = @((Join-Path $portsRoot 'terminal_profile.py'), '--name', $ProfileName)
    if ($FocusShortcut) { $portsProfileArguments += @('--focus-shortcut', $FocusShortcut) }
    & $portsPython @portsProfileArguments
    if ($LASTEXITCODE -ne 0) { throw 'Windows Terminal profile registration failed.' }
}
Write-Output 'Installed. Open the Terminal profile or run .\.venv\Scripts\python.exe app.py'
