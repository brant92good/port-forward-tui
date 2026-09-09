param([switch]$SkipRust, [string]$OutputDirectory = '')
$ErrorActionPreference = 'Stop'
$portsRoot = Split-Path $PSScriptRoot -Parent
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $portsRoot 'artifacts\native-build' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if ($OutputDirectory.TrimEnd('\') -eq $portsRoot.TrimEnd('\')) { throw 'Use an artifact directory; production replacement belongs to the installer.' }
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$portsFramework = Join-Path $env:SystemRoot 'Microsoft.NET\Framework64\v4.0.30319'
$portsReferences = @('System.dll','System.Core.dll','System.Web.Extensions.dll') + @(
    (Join-Path $portsFramework 'WPF\UIAutomationClient.dll'),(Join-Path $portsFramework 'WPF\UIAutomationTypes.dll'),(Join-Path $portsFramework 'WPF\WindowsBase.dll'))
$portsArguments = $portsReferences | ForEach-Object { "/reference:$_" }
& (Join-Path $portsFramework 'csc.exe') /nologo /target:exe /platform:x64 "/out:$(Join-Path $OutputDirectory 'PortsFocus.exe')" @portsArguments (Join-Path $portsRoot 'native\FocusHelper.cs')
if ($LASTEXITCODE -ne 0) { throw 'Could not compile PortsFocus.' }
& (Join-Path $portsFramework 'csc.exe') /nologo /target:exe /platform:x64 "/out:$(Join-Path $OutputDirectory 'TerminalViews.exe')" @portsArguments (Join-Path $portsRoot 'native\TerminalViews.cs') (Join-Path $portsRoot 'native\TerminalViewsMain.cs')
if ($LASTEXITCODE -ne 0) { throw 'Could not compile TerminalViews.' }
if (-not $SkipRust) {
    $portsRustFlags = $env:RUSTFLAGS
    try {
        $env:RUSTFLAGS = '-C target-feature=+crt-static'
        Push-Location $portsRoot
        try { & cargo build --release --locked --target x86_64-pc-windows-msvc } finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw 'Could not compile Ports.' }
        [IO.File]::Copy((Join-Path $portsRoot 'target\x86_64-pc-windows-msvc\release\ports.exe'),(Join-Path $OutputDirectory 'ports.exe'),$true)
    } finally { $env:RUSTFLAGS = $portsRustFlags }
}
