$ErrorActionPreference = 'Stop'
$trackerRoot = Split-Path $PSScriptRoot -Parent
$trackerFramework = Join-Path $env:SystemRoot 'Microsoft.NET\Framework64\v4.0.30319'
$trackerOutput = Join-Path $trackerRoot ('artifacts\tracker-observation-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $trackerOutput -Force | Out-Null
$trackerBinary = Join-Path $trackerOutput 'TerminalViewsObservationTests.exe'
$trackerReferences = @('System.dll','System.Core.dll','System.Web.Extensions.dll') + @(
    (Join-Path $trackerFramework 'WPF\UIAutomationClient.dll'),
    (Join-Path $trackerFramework 'WPF\UIAutomationTypes.dll'),
    (Join-Path $trackerFramework 'WPF\WindowsBase.dll'))
$trackerArguments = $trackerReferences | ForEach-Object { "/reference:$_" }
& (Join-Path $trackerFramework 'csc.exe') /nologo /target:exe /platform:x64 "/out:$trackerBinary" @trackerArguments (Join-Path $trackerRoot 'native\TerminalViews.cs') (Join-Path $PSScriptRoot 'TerminalViewsObservationTests.cs')
if ($LASTEXITCODE -ne 0) { throw 'Tracker observation test compilation failed.' }
$trackerStart = New-Object Diagnostics.ProcessStartInfo
$trackerStart.FileName = $trackerBinary
$trackerStart.WorkingDirectory = $trackerOutput
$trackerStart.UseShellExecute = $false
$trackerStart.CreateNoWindow = $true
$trackerStart.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
$trackerStart.RedirectStandardOutput = $true
$trackerStart.RedirectStandardError = $true
$trackerProcess = New-Object Diagnostics.Process
$trackerProcess.StartInfo = $trackerStart
$trackerStarted = $false
try {
    $trackerStarted = $trackerProcess.Start()
    if (-not $trackerStarted) { throw 'Could not start tracker observation tests.' }
    $trackerOut = $trackerProcess.StandardOutput.ReadToEndAsync()
    $trackerErr = $trackerProcess.StandardError.ReadToEndAsync()
    if (-not $trackerProcess.WaitForExit(15000)) {
        $trackerProcess.Kill()
        if (-not $trackerProcess.WaitForExit(2000)) {
            throw 'Tracker test timeout cleanup failed to exit within 2 seconds.'
        }
        throw 'Tracker observation tests exceeded 15 seconds.'
    }
    $trackerReaders = [Threading.Tasks.Task[]]@($trackerOut, $trackerErr)
    if (-not [Threading.Tasks.Task]::WaitAll($trackerReaders, 2000)) {
        throw 'Tracker test output did not close within 2 seconds after exit.'
    }
    [IO.File]::WriteAllText((Join-Path $trackerOutput 'stdout.txt'), $trackerOut.Result)
    [IO.File]::WriteAllText((Join-Path $trackerOutput 'stderr.txt'), $trackerErr.Result)
    Write-Output $trackerOut.Result
    if ($trackerProcess.ExitCode -ne 0) {
        Write-Output $trackerErr.Result
        throw "Tracker observation tests failed with exit $($trackerProcess.ExitCode)."
    }
} finally {
    try {
        if ($trackerStarted -and -not $trackerProcess.HasExited) {
            $trackerProcess.Kill()
            if (-not $trackerProcess.WaitForExit(2000)) {
                throw 'Tracker test final cleanup failed to exit within 2 seconds.'
            }
        }
    } finally {
        $trackerProcess.Dispose()
    }
}
