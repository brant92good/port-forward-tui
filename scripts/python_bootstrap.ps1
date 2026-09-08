# Shared, per-user Python bootstrap. Never activates or modifies a global environment.
function Get-AppPythonInfo {
    param([Parameter(Mandatory = $true)][string]$Executable, [string[]]$PrefixArguments = @())
    $probeCode = 'import ctypes,json,ssl,sys,venv; print(json.dumps(dict(executable=sys.executable,prefix=sys.prefix,base_prefix=sys.base_prefix,version=list(sys.version_info[:3]),platform=sys.platform)))'
    $probeEncoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($probeCode))
    $probeInfo = New-Object Diagnostics.ProcessStartInfo
    $probeInfo.FileName = $Executable
    # The encoded expression contains no spaces or double quotes; no shell is used.
    $probeInfo.Arguments = (@($PrefixArguments) + @('-I', '-c', "exec(__import__('base64').b64decode('$probeEncoded'))")) -join ' '
    $probeInfo.UseShellExecute = $false
    $probeInfo.CreateNoWindow = $true
    $probeInfo.RedirectStandardOutput = $true
    $probeInfo.RedirectStandardError = $true
    $probeProcess = New-Object Diagnostics.Process
    $probeProcess.StartInfo = $probeInfo
    try {
        [void]$probeProcess.Start()
        $probeOutput = $probeProcess.StandardOutput.ReadToEndAsync()
        $probeError = $probeProcess.StandardError.ReadToEndAsync()
        if (-not $probeProcess.WaitForExit(8000)) {
            $probeProcess.Kill()
            throw 'Python startup timed out.'
        }
        $probeText = $probeOutput.GetAwaiter().GetResult()
        $probeDetails = $probeError.GetAwaiter().GetResult()
        if ($probeProcess.ExitCode -ne 0) {
            throw "Python could not import its required standard libraries (including SSL). $($probeDetails.Trim())"
        }
        $probeResult = $probeText | ConvertFrom-Json -ErrorAction Stop
        if ($probeResult.platform -ne 'win32' -or $probeResult.version[0] -ne 3 -or $probeResult.version[1] -lt 12) {
            throw 'A Windows Python 3.12 or newer is required. WSL Python cannot run the Windows launchers.'
        }
        return $probeResult
    } finally {
        $probeProcess.Dispose()
    }
}

function Resolve-AppPython {
    param([string]$Python = '')
    $pythonCandidates = if ($Python) { @($Python) } else { @('python.exe', 'py.exe', 'python3.exe') }
    $pythonFailures = [Collections.Generic.List[string]]::new()
    foreach ($pythonCandidate in $pythonCandidates) {
        $pythonCommand = Get-Command -Name $pythonCandidate -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $pythonCommand) { $pythonFailures.Add("${pythonCandidate}: not found"); continue }
        try {
            $pythonPrefix = if ([IO.Path]::GetFileNameWithoutExtension($pythonCommand.Source) -eq 'py') { @('-3') } else { @() }
            return Get-AppPythonInfo -Executable $pythonCommand.Source -PrefixArguments $pythonPrefix
        } catch {
            $pythonFailures.Add("${pythonCandidate}: $($_.Exception.Message)")
        }
    }
    throw ("No usable Windows Python 3.12+ was found. Install Python or activate your chosen Conda environment for setup. " +
        "You can also pass -Python 'C:\path\to\python.exe' (the real executable for shim-based tools). " +
        ($pythonFailures -join [Environment]::NewLine))
}

function Initialize-AppPython {
    param([Parameter(Mandatory = $true)][string]$Root, [string]$Python = '')
    $environmentRoot = [IO.Path]::GetFullPath((Join-Path $Root '.venv'))
    $environmentPython = Join-Path $environmentRoot 'Scripts\python.exe'
    if (-not (Test-Path -LiteralPath $environmentPython)) {
        if (Test-Path -LiteralPath $environmentRoot) {
            throw "The existing $environmentRoot is incomplete. Rename it to keep a backup, then rerun installation. It was not deleted."
        }
        $selectedPython = Resolve-AppPython -Python $Python
        & $selectedPython.executable -I -m venv $environmentRoot
        if ($LASTEXITCODE -ne 0) { throw 'Could not create the private app environment. Check the Python installation and available disk space.' }
    }
    try {
        $environmentInfo = Get-AppPythonInfo -Executable $environmentPython
        if (-not [string]::Equals([IO.Path]::GetFullPath($environmentInfo.prefix), $environmentRoot, [StringComparison]::OrdinalIgnoreCase) -or
            $environmentInfo.prefix -eq $environmentInfo.base_prefix) {
            throw 'The executable does not belong to the private app environment.'
        }
    } catch {
        throw "The existing app environment could not be validated: $($_.Exception.Message) Restore its base Python installation, or rename $environmentRoot and rerun setup."
    }
    Write-Host "Using private app environment: Python $($environmentInfo.version -join '.')"
    return $environmentPython
}
