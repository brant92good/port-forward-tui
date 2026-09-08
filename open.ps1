$ErrorActionPreference = 'Stop'
$openPython = Join-Path $PSScriptRoot '.venv\Scripts\python.exe'
if (-not (Test-Path -LiteralPath $openPython)) { throw 'Run .\install.ps1 first, then .\open.ps1.' }
& $openPython -E -s (Join-Path $PSScriptRoot 'app.py') @args
exit $LASTEXITCODE
