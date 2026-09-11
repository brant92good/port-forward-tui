param(
    [ValidateSet('stable','beta')][string]$Channel = 'stable',
    [string]$InstallDir = $env:PORTS_INSTALL_DIR,
    [string]$Version = $(if ($env:PORTS_VERSION) { $env:PORTS_VERSION } else { '0.9.1' }),
    [string]$Bundle = $env:PORTS_BUNDLE,
    [string]$Sha256 = $env:PORTS_SHA256,
    [switch]$NoPath = ($env:PORTS_NO_PATH -eq '1')
)
$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitOperatingSystem) { throw 'Ports requires 64-bit Windows.' }
if (($Channel -eq 'stable' -and $Version -cnotmatch '\A(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\z') -or
    ($Channel -eq 'beta' -and $Version -cnotmatch '\A(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-beta\.[1-9][0-9]*\z')) { throw 'Select an exact stable version, or explicitly use -Channel beta with an exact x.y.z-beta.N version.' }
$portsCommandName = if ($Channel -eq 'beta') { 'ports-beta.exe' } else { 'ports.exe' }
$portsOwner = if ($Channel -eq 'beta') { 'port-forward-tui-beta' } else { 'port-forward-tui' }
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA $(if ($Channel -eq 'beta') { 'Programs\PortsBeta' } else { 'Programs\Ports' }) }
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
if ($Channel -eq 'beta') {
    $NoPath = $true # The beta is an explicit command/path, never a stable PATH replacement.
    $stableRoot = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs\Ports')).TrimEnd('\','/')
    if ($InstallDir.TrimEnd('\','/') -ieq $stableRoot -or $InstallDir.StartsWith($stableRoot+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Beta must not be installed in the stable installation.' }
    $inspect = $InstallDir
    while ($inspect) {
        if ((Test-Path -LiteralPath $inspect) -and ((Get-Item -LiteralPath $inspect -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Beta install paths must not traverse junctions or symbolic links.' }
        $parent = [IO.Directory]::GetParent($inspect); $inspect = if ($parent) { $parent.FullName } else { $null }
    }
}
$portsMarker = Join-Path $InstallDir '.ports-installer'
if (Test-Path -LiteralPath $portsMarker) {
    if ([IO.File]::ReadAllText($portsMarker).Trim() -ne $portsOwner) { throw 'This directory belongs to another application or release channel.' }
} elseif ((Test-Path -LiteralPath $InstallDir) -and @(Get-ChildItem -LiteralPath $InstallDir -Force).Count) { throw 'Choose an empty or installer-owned directory.' }
if ($Channel -eq 'beta') {
    foreach ($relative in @('bin','version','.ports-installer','bin\ports-beta.exe','bin\PortsFocus.exe','bin\TerminalViews.exe','bin\LICENSE.txt','bin\THIRD_PARTY_NOTICES.txt')) {
        $candidate = Join-Path $InstallDir $relative
        if ((Test-Path -LiteralPath $candidate) -and ((Get-Item -LiteralPath $candidate -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Beta installation files must not traverse junctions or symbolic links.' }
    }
}
function Get-PortsHash([string]$Path) {
    $portsHasher = [Security.Cryptography.SHA256]::Create(); $portsInput = [IO.File]::OpenRead($Path)
    try { ([BitConverter]::ToString($portsHasher.ComputeHash($portsInput))).Replace('-','').ToLowerInvariant() }
    finally { $portsInput.Dispose(); $portsHasher.Dispose() }
}
$portsTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$portsStage = Join-Path $portsTempRoot ('ports-install-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $portsStage | Out-Null
try {
    if (-not $Bundle) { $Bundle = "https://github.com/brant92good/port-forward-tui/releases/download/v$Version/ports-x86_64-pc-windows-msvc.zip" }
    $portsArchive = Join-Path $portsStage 'ports.zip'
    if ($Bundle -match '^https://') {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri $Bundle -OutFile $portsArchive
        if (-not $Sha256) {
            $portsChecksum = Join-Path $portsStage 'checksum.txt'
            Invoke-WebRequest -UseBasicParsing -Uri ($Bundle + '.sha256') -OutFile $portsChecksum
            $Sha256 = ([IO.File]::ReadAllText($portsChecksum).Trim() -split '\s+')[0]
        }
    } elseif ((Test-Path -LiteralPath $Bundle -PathType Leaf) -and $Sha256) { [IO.File]::Copy([IO.Path]::GetFullPath($Bundle),$portsArchive) }
    else { throw 'Use an HTTPS bundle or a local file with -Sha256.' }
    if ($Sha256 -notmatch '^[a-fA-F0-9]{64}$' -or (Get-PortsHash $portsArchive) -ne $Sha256.ToLowerInvariant()) { throw 'Download checksum mismatch. The existing installation was preserved.' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $portsFiles = @('ports.exe','PortsFocus.exe','TerminalViews.exe')
    $portsNotices = @('LICENSE.txt','THIRD_PARTY_NOTICES.txt')
    $portsLegacyVersion = ([version]($Version.Split('-')[0])) -le ([version]'0.8.1')
    $portsZip = [IO.Compression.ZipFile]::OpenRead($portsArchive)
    try {
        $portsNames = @($portsZip.Entries | ForEach-Object { $_.FullName })
        if ($portsNames.Count -eq 6) { $portsFiles += $portsNotices }
        elseif ($portsNames.Count -eq 4 -and -not $portsLegacyVersion) { throw 'This Ports release requires both bundled license notices.' }
        elseif ($portsNames.Count -ne 4) { throw 'The Ports bundle has unexpected archive entries or incomplete license notices.' }
        if (@($portsNames | Where-Object { $_ -notin ($portsFiles + 'SHA256SUMS') }).Count -or @($portsNames | Select-Object -Unique).Count -ne ($portsFiles.Count + 1)) { throw 'The Ports bundle has unexpected archive entries.' }
    } finally { $portsZip.Dispose() }
    $portsPackage = Join-Path $portsStage 'package'
    [IO.Compression.ZipFile]::ExtractToDirectory($portsArchive,$portsPackage)
    $portsChecksums = @{}
    foreach ($portsLine in [IO.File]::ReadAllLines((Join-Path $portsPackage 'SHA256SUMS'))) {
        if ($portsLine -notmatch '^([a-f0-9]{64})  (ports\.exe|PortsFocus\.exe|TerminalViews\.exe|LICENSE\.txt|THIRD_PARTY_NOTICES\.txt)$') { throw 'Invalid bundled checksum index.' }
        if ($Matches[2] -notin $portsFiles) { throw 'Unexpected file in bundled checksum index.' }
        if ($portsChecksums.ContainsKey($Matches[2])) { throw 'Duplicate bundled checksum.' }
        $portsChecksums[$Matches[2]] = $Matches[1]
    }
    if ($portsChecksums.Count -ne $portsFiles.Count) { throw 'Bundled checksum index is incomplete.' }
    foreach ($portsName in $portsFiles) {
        if ((Get-PortsHash (Join-Path $portsPackage $portsName)) -ne $portsChecksums[$portsName]) { throw "Invalid bundled file $portsName." }
    }
    $portsVersion = & (Join-Path $portsPackage 'ports.exe') --version
    if ($LASTEXITCODE -ne 0 -or $portsVersion -ne "ports $Version") { throw 'The downloaded app could not run or has the wrong version.' }
    if ($Channel -eq 'beta') {
        Move-Item -LiteralPath (Join-Path $portsPackage 'ports.exe') -Destination (Join-Path $portsPackage $portsCommandName)
        $portsFiles = @($portsFiles | ForEach-Object { if ($_ -eq 'ports.exe') { $portsCommandName } else { $_ } })
    }
    $portsBin = Join-Path $InstallDir 'bin'
    New-Item -ItemType Directory -Path $portsBin -Force | Out-Null
    $portsTransaction = [Guid]::NewGuid().ToString('N')
    $portsChanges = New-Object Collections.Generic.List[object]
    try {
        foreach ($portsName in $portsFiles) {
            $portsDestination = Join-Path $portsBin $portsName
            $portsPrevious = $null
            if (Test-Path -LiteralPath $portsDestination) {
                $portsPrevious = $portsDestination + '.previous-' + $portsTransaction
                Move-Item -LiteralPath $portsDestination -Destination $portsPrevious
            }
            $portsChanges.Add(@{path=$portsDestination;previous=$portsPrevious})
            [IO.File]::Copy((Join-Path $portsPackage $portsName),$portsDestination)
        }
        [IO.File]::WriteAllText($portsMarker,$portsOwner)
        [IO.File]::WriteAllText((Join-Path $InstallDir 'version'),$Version)
    } catch {
        for ($portsIndex=$portsChanges.Count-1; $portsIndex -ge 0; $portsIndex--) {
            $portsChange = $portsChanges[$portsIndex]
            if (Test-Path -LiteralPath $portsChange.path) { Remove-Item -LiteralPath $portsChange.path -Force }
            if ($portsChange.previous) { Move-Item -LiteralPath $portsChange.previous -Destination $portsChange.path }
        }
        throw
    }
    if (-not $NoPath) {
        $portsUserPath = [Environment]::GetEnvironmentVariable('Path','User')
        $portsEntries = @($portsUserPath -split ';' | Where-Object { $_ })
        if (-not ($portsEntries | Where-Object { $_.TrimEnd('\') -ieq $portsBin.TrimEnd('\') })) { [Environment]::SetEnvironmentVariable('Path',(($portsEntries + $portsBin) -join ';'),'User') }
        if (-not ($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $portsBin.TrimEnd('\') })) { $env:Path += ';' + $portsBin }
    }
    Write-Output "Installed Ports $Version ($Channel)."
    Write-Output "Command: $(Join-Path $portsBin $portsCommandName)"
    if ($Channel -eq 'beta') { Write-Output 'BETA: invoke the exact ports-beta.exe path above; PATH and the stable ports command were preserved.' }
    Write-Output 'Add a machine or import SSH aliases on first launch. Open a new terminal if the command is not found yet.'
} finally {
    $portsResolved = [IO.Path]::GetFullPath($portsStage)
    if ((Split-Path $portsResolved -Parent) -eq $portsTempRoot -and (Split-Path $portsResolved -Leaf) -match '^ports-install-[a-f0-9]{32}$') { Remove-Item -LiteralPath $portsResolved -Recurse -Force }
}
