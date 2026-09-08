# Forward arguments literally; Python receives an argument list, never shell code.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'scripts\python_bootstrap.ps1')
$portsCommandPython = Join-Path $PSScriptRoot '.venv\Scripts\python.exe'
if (-not (Test-Path -LiteralPath $portsCommandPython)) {
    $portsCommandPython = (Resolve-AppPython).executable
}
& $portsCommandPython -E -s (Join-Path $PSScriptRoot 'ports.py') @args
exit $LASTEXITCODE
