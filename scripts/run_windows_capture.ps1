param(
    [string]$Executable = '', [string]$WorkingDirectory = '', [string]$Bundle = '',
    [ValidateRange(10,120)][int]$TimeoutSeconds = 60,
    [Parameter(DontShow=$true)][string]$WorkerSpec = ''
)
# CI fixture only. Production detachment policy is deliberately unchanged.
# WMI's provider also owns a job; explicitly request breakaway and verify it:
# https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/create-method-in-class-win32-process
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This helper runs only in a Windows GitHub Actions job.'
}
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class CaptureJobProbe {
    [DllImport("kernel32.dll", SetLastError=true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool result);
    public static bool InJob(IntPtr process) {
        bool result;
        if (!IsProcessInJob(process, IntPtr.Zero, out result)) throw new Win32Exception();
        return result;
    }
}
'@
function Write-CaptureJson([string]$Path,$Value) {
    $temporary = $Path + '.tmp'
    [IO.File]::WriteAllText($temporary,($Value | ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
    [IO.File]::Move($temporary,$Path)
}
function Assert-CaptureFixture([string]$Path) {
    $pathFull = [IO.Path]::GetFullPath($Path)
    $runnerTemp = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')
    if ((Split-Path $pathFull -Parent) -ine $runnerTemp -or (Split-Path $pathFull -Leaf) -notmatch '^ports-capture-[a-f0-9]{32}$') {
        throw 'Unexpected capture fixture path.'
    }
    if ((Get-Item -LiteralPath $pathFull).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Fixture cannot be a reparse point.' }
    return $pathFull
}
if ($WorkerSpec) {
    $captureRoot = Assert-CaptureFixture (Split-Path ([IO.Path]::GetFullPath($WorkerSpec)) -Parent)
    if ([IO.Path]::GetFullPath($WorkerSpec) -ine (Join-Path $captureRoot 'spec.json')) { throw 'Unexpected worker specification.' }
    $spec = [IO.File]::ReadAllText($WorkerSpec) | ConvertFrom-Json
    $test = $null
    $result = [ordered]@{ok=$false;stage='harness';worker_pid=$PID;worker_in_job=$null;test_in_job=$null;test_pid=$null;exit_code=$null;error=$null}
    try {
        if ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -cne $spec.sid) { throw 'WMI worker account differs from the invoking runner.' }
        $result.worker_in_job = [CaptureJobProbe]::InJob((Get-Process -Id $PID).Handle)
        if ($result.worker_in_job) { throw 'WMI worker is still constrained by a job; detachment test did not run.' }
        if ((Get-FileHash -LiteralPath $spec.executable -Algorithm SHA256).Hash -cne $spec.sha256) { throw 'Test executable changed after selection.' }
        Write-CaptureJson (Join-Path $captureRoot 'ready.json') $result
        $readyLimit = [DateTime]::UtcNow.AddSeconds(20)
        while (-not (Test-Path -LiteralPath (Join-Path $captureRoot 'go'))) {
            if ([DateTime]::UtcNow -gt $readyLimit) { throw 'Runner did not authorize the verified worker.' }
            Start-Sleep -Milliseconds 50
        }
        $env:TEMP = Join-Path $captureRoot 'temp'; $env:TMP = $env:TEMP
        [IO.Directory]::CreateDirectory($env:TEMP) | Out-Null
        if ($spec.bundle) { $env:WORKSPACE_TEST_BUNDLE = $spec.bundle }
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = $spec.executable; $start.Arguments = '--ignored --nocapture'
        $start.WorkingDirectory = $spec.working_directory
        $start.UseShellExecute = $false; $start.CreateNoWindow = $true
        $start.RedirectStandardInput = $true; $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
        $start.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
        $start.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
        $test = [Diagnostics.Process]::new(); $test.StartInfo = $start
        if (-not $test.Start()) { throw 'Could not start capture regression.' }
        $null = $test.Handle # Cache the actual process handle, not a later reused PID.
        $result.test_pid = $test.Id
        Write-CaptureJson (Join-Path $captureRoot 'test-start.json') ([ordered]@{pid=$test.Id;executable=$spec.executable;created_utc=$test.StartTime.ToUniversalTime().ToString('o')})
        $result.test_in_job = [CaptureJobProbe]::InJob($test.Handle)
        if ($result.test_in_job) { throw 'Capture test is still constrained by a job.' }
        $test.StandardInput.Close()
        $stdout = $test.StandardOutput.ReadToEndAsync(); $stderr = $test.StandardError.ReadToEndAsync()
        $result.stage = 'regression'
        if (-not $test.WaitForExit([int]$spec.timeout_seconds * 1000)) { throw 'Capture regression exceeded its deadline.' }
        if (-not $stdout.Wait(2000) -or -not $stderr.Wait(2000)) { throw 'Capture regression retained its logging pipes.' }
        [IO.File]::WriteAllText((Join-Path $captureRoot 'stdout.log'),$stdout.Result,[Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText((Join-Path $captureRoot 'stderr.log'),$stderr.Result,[Text.UTF8Encoding]::new($false))
        $result.exit_code = $test.ExitCode; $result.ok = $test.ExitCode -eq 0
    } catch { $result.error = $_.Exception.Message }
    finally {
        if ($test) {
            if (-not $test.HasExited) { $test.Kill(); $null = $test.WaitForExit(3000) }
            $test.Dispose()
        }
        Write-CaptureJson (Join-Path $captureRoot 'result.json') $result
    }
    exit $(if ($result.ok) { 0 } else { 1 })
}
if (-not $WorkingDirectory) { $WorkingDirectory = (Get-Location).Path }
$WorkingDirectory = [IO.Path]::GetFullPath($WorkingDirectory).TrimEnd('\')
$Executable = [IO.Path]::GetFullPath($Executable)
if (-not $Executable.StartsWith($WorkingDirectory + '\',[StringComparison]::OrdinalIgnoreCase) -or
    [IO.Path]::GetFileName($Executable) -notmatch '^native_(cli_)?capture-[a-f0-9]+\.exe$' -or
    -not (Test-Path -LiteralPath $Executable -PathType Leaf)) { throw 'Choose the compiled capture regression inside this checkout.' }
if ($Bundle) { $Bundle = [IO.Path]::GetFullPath($Bundle); if (-not (Test-Path -LiteralPath $Bundle -PathType Leaf)) { throw 'Bundle does not exist.' } }
$captureRoot = Join-Path ([IO.Path]::GetFullPath($env:RUNNER_TEMP)) ('ports-capture-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($captureRoot) | Out-Null
$captureRoot = Assert-CaptureFixture $captureRoot
$workerPath = Join-Path $captureRoot 'worker.ps1'; $specPath = Join-Path $captureRoot 'spec.json'
[IO.File]::Copy($PSCommandPath,$workerPath)
$started = [DateTime]::UtcNow; $worker = $null; $workerId = 0; $success = $false; $workerVerified = $false
function Stop-CaptureRecord($record) {
    if (-not $record.CreationDate -or $record.CreationDate.ToUniversalTime() -lt $started.AddSeconds(-1)) { throw "Cleanup process lacks this invocation's creation identity." }
    $process = $null
    try {
        $process = [Diagnostics.Process]::GetProcessById([int]$record.ProcessId)
        $null = $process.Handle
        if ($process.HasExited) { return }
        if ($process.MainModule.FileName -ine $record.ExecutablePath -or
            [Math]::Abs(($process.StartTime.ToUniversalTime() - $record.CreationDate.ToUniversalTime()).TotalMilliseconds) -gt 1) { throw 'Process identity changed before cleanup.' }
        $process.Kill()
        if (-not $process.WaitForExit(3000)) { throw 'Owned capture process did not stop.' }
    } catch [ArgumentException] { } # Already exited before a handle was obtained.
    finally { if ($process) { $process.Dispose() } }
}
function Stop-OwnedCaptureProcesses {
    # Stop each spawner before enumerating its children. A single snapshot taken
    # while the worker/test still ran could miss a newly detached controller.
    if ($workerVerified -and $worker) {
        if (-not $worker.HasExited) { $worker.Kill(); if (-not $worker.WaitForExit(3000)) { throw 'Verified worker did not stop.' } }
    } elseif ($workerId -gt 0) {
        $record = Get-CimInstance Win32_Process -Filter "ProcessId=$workerId" -OperationTimeoutSec 10
        if ($record -and $record.ExecutablePath -ieq $powershell -and $record.CommandLine -ceq $command) { Stop-CaptureRecord $record }
    }
    if ($workerId -gt 0) {
        foreach ($record in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$workerId" -OperationTimeoutSec 10)) {
            if ($record.ExecutablePath -ieq $Executable) { Stop-CaptureRecord $record }
        }
    }
    # Packaged tests also launch PowerShell wrappers. Stop those verified
    # fixture shells after the test and before any Ports process snapshot.
    $shellPaths = @((Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'))
    $pwshCommand = Get-Command pwsh.exe -CommandType Application -ErrorAction SilentlyContinue
    if ($pwshCommand) { $shellPaths += $pwshCommand.Source }
    foreach ($record in @(Get-CimInstance Win32_Process -Filter "Name='powershell.exe' OR Name='pwsh.exe'" -OperationTimeoutSec 10)) {
        if ($record.ExecutablePath -and $shellPaths -icontains $record.ExecutablePath -and
            $record.CommandLine -and $record.CommandLine.IndexOf($captureRoot,[StringComparison]::OrdinalIgnoreCase) -ge 0) {
            Stop-CaptureRecord $record
        }
    }
    # No-SSH tests create only Ports CLI/daemon processes after those shells.
    # Stop CLI spawners first, then repeat after they cannot spawn anymore.
    $cleanupLimit = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $remaining = @(Get-CimInstance Win32_Process -Filter "Name='ports.exe'" -OperationTimeoutSec 10 | Where-Object {
            $_.CommandLine -and $_.CommandLine.IndexOf($captureRoot,[StringComparison]::OrdinalIgnoreCase) -ge 0 -and
            ($_.ExecutablePath.StartsWith($WorkingDirectory + '\',[StringComparison]::OrdinalIgnoreCase) -or $_.ExecutablePath.StartsWith($captureRoot + '\',[StringComparison]::OrdinalIgnoreCase))
        } | Sort-Object { [int]($_.CommandLine -match '(?<!\S)"?--serve"?(?!\S)') })
        if (-not $remaining.Count) { break }
        foreach ($record in $remaining) { Stop-CaptureRecord $record }
        if ([DateTime]::UtcNow -gt $cleanupLimit) { throw 'Owned controller cleanup exceeded its deadline.' }
    } while ($true)
}

try {
    Write-CaptureJson $specPath ([ordered]@{sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;executable=$Executable;sha256=(Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash;working_directory=$WorkingDirectory;timeout_seconds=$TimeoutSeconds;bundle=$Bundle})
    $environment = foreach ($name in @('PATH','SystemRoot','windir','COMSPEC','APPDATA','LOCALAPPDATA','USERPROFILE','USERNAME','USERDOMAIN','ProgramFiles','ProgramFiles(x86)','ProgramData','PROCESSOR_ARCHITECTURE')) {
        $value = [Environment]::GetEnvironmentVariable($name); if ($value) { $name + '=' + $value }
    }
    $environment += @('GITHUB_ACTIONS=true','RUNNER_OS=Windows',('RUNNER_TEMP=' + $env:RUNNER_TEMP),('TEMP=' + $captureRoot),('TMP=' + $captureRoot))
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ShowWindow=[uint16]0;CreateFlags=[uint32]0x09000400;EnvironmentVariables=[string[]]$environment}
    $powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $command = '"' + $powershell + '" -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + $workerPath + '" -WorkerSpec "' + $specPath + '"'
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -OperationTimeoutSec 10 -Arguments @{CommandLine=$command;CurrentDirectory=$WorkingDirectory;ProcessStartupInformation=$startup}
    if ($created.ReturnValue -ne 0) { throw "WMI harness setup failed (code $($created.ReturnValue)); capture regression did not run." }
    $workerId = [int]$created.ProcessId
    $record = Get-CimInstance Win32_Process -Filter "ProcessId=$workerId" -OperationTimeoutSec 10
    if (-not $record) {
        $early = Join-Path $captureRoot 'result.json'; if (Test-Path -LiteralPath $early) { Write-Output ([IO.File]::ReadAllText($early)) }; throw 'WMI worker exited before identity verification.'
    }
    if ($record.ExecutablePath -ine $powershell -or $record.CommandLine -cne $command) { throw 'WMI worker identity differs from the requested hidden command.' }
    $worker = [Diagnostics.Process]::GetProcessById($workerId); $null = $worker.Handle
    if ($worker.MainModule.FileName -ine $record.ExecutablePath -or [Math]::Abs(($worker.StartTime.ToUniversalTime() - $record.CreationDate.ToUniversalTime()).TotalMilliseconds) -gt 1) { throw 'Cached worker handle differs from the WMI identity.' }
    $workerVerified = $true
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds + 30)
    $approved = $false
    while (-not (Test-Path -LiteralPath (Join-Path $captureRoot 'result.json'))) {
        if (-not $approved -and (Test-Path -LiteralPath (Join-Path $captureRoot 'ready.json'))) {
            $ready = [IO.File]::ReadAllText((Join-Path $captureRoot 'ready.json')) | ConvertFrom-Json
            if ($ready.worker_pid -ne $workerId -or $ready.worker_in_job -ne $false -or [CaptureJobProbe]::InJob($worker.Handle)) { throw 'WMI worker did not prove job-free identity.' }
            [IO.File]::WriteAllText((Join-Path $captureRoot 'go'),'run'); $approved = $true
        }
        if ($worker.HasExited -and -not (Test-Path -LiteralPath (Join-Path $captureRoot 'result.json'))) { throw 'WMI harness exited without a completion record.' }
        if ([DateTime]::UtcNow -gt $deadline) { throw 'WMI capture harness exceeded its deadline.' }
        Start-Sleep -Milliseconds 100
    }
    $result = [IO.File]::ReadAllText((Join-Path $captureRoot 'result.json')) | ConvertFrom-Json
    foreach ($name in @('stdout.log','stderr.log')) { $path = Join-Path $captureRoot $name; if (Test-Path -LiteralPath $path) { Write-Output ([IO.File]::ReadAllText($path)) } }
    Write-Output ($result | ConvertTo-Json -Depth 4)
    if (-not $result.ok -or $result.exit_code -ne 0 -or $result.worker_in_job -ne $false -or $result.test_in_job -ne $false) { throw "Capture $($result.stage) failed: $($result.error)" }
    if (-not $worker.WaitForExit(5000)) { throw 'WMI worker did not exit after reporting completion.' }
    $success = $true
} finally {
    Stop-OwnedCaptureProcesses
    if ($worker) { $worker.Dispose() }
    if ($success) { $verified = Assert-CaptureFixture $captureRoot; Remove-Item -LiteralPath $verified -Recurse -Force }
    else {
        foreach ($name in @('result.json','stdout.log','stderr.log')) {
            $path = Join-Path $captureRoot $name
            if (Test-Path -LiteralPath $path) { Write-Output ([IO.File]::ReadAllText($path)) }
        }
        Write-Warning "Capture fixture retained for diagnostics: $captureRoot"
    }
}
