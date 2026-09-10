param([string]$Binary = '', [string]$OutputDirectory = '')
$ErrorActionPreference = 'Stop'
$portsRoot = Split-Path $PSScriptRoot -Parent
if (-not $Binary) { $Binary = Join-Path $portsRoot 'target\x86_64-pc-windows-msvc\release\ports.exe' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $portsRoot 'dist' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$portsStage = Join-Path $portsRoot ('artifacts\package-' + [Guid]::NewGuid().ToString('N'))
& (Join-Path $PSScriptRoot 'build_native.ps1') -SkipRust -OutputDirectory $portsStage
[IO.File]::Copy([IO.Path]::GetFullPath($Binary),(Join-Path $portsStage 'ports.exe'))
[IO.File]::Copy((Join-Path $portsRoot 'LICENSE'),(Join-Path $portsStage 'LICENSE.txt'))
[IO.File]::Copy((Join-Path $portsRoot 'docs\licenses\THIRD_PARTY_NOTICES.txt'),(Join-Path $portsStage 'THIRD_PARTY_NOTICES.txt'))
if ((& (Join-Path $portsStage 'ports.exe') --version) -ne 'ports 0.9.0' -or $LASTEXITCODE -ne 0) { throw 'Expected Ports 0.9.0.' }
function Get-PortsPackageHash([string]$Path) {
    $portsHasher = [Security.Cryptography.SHA256]::Create(); $portsInput = [IO.File]::OpenRead($Path)
    try { ([BitConverter]::ToString($portsHasher.ComputeHash($portsInput))).Replace('-','').ToLowerInvariant() }
    finally { $portsInput.Dispose(); $portsHasher.Dispose() }
}
$portsLines = foreach ($portsName in @('ports.exe','PortsFocus.exe','TerminalViews.exe','LICENSE.txt','THIRD_PARTY_NOTICES.txt')) { (Get-PortsPackageHash (Join-Path $portsStage $portsName)) + '  ' + $portsName }
[IO.File]::WriteAllText((Join-Path $portsStage 'SHA256SUMS'),(($portsLines -join "`n") + "`n"),(New-Object Text.UTF8Encoding($false)))
$portsArchive = Join-Path $OutputDirectory 'ports-x86_64-pc-windows-msvc.zip'
if (Test-Path -LiteralPath $portsArchive) { throw 'Release archive exists. Use another directory; immutable release bytes must not be overwritten.' }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory($portsStage,$portsArchive)
[IO.File]::WriteAllText(($portsArchive + '.sha256'),((Get-PortsPackageHash $portsArchive) + '  ' + [IO.Path]::GetFileName($portsArchive) + "`n"))
Write-Output $portsArchive
