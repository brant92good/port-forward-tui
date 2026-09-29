param(
    [switch]$EnableAudio,
    [string]$Ports,
    [string]$DataDir,
    [string]$Machine,
    [string]$Microphone,
    [string]$RemoteScript,
    [string]$ExpectedHost,
    [string]$FFmpeg,
    [string]$FFplay
)
$ErrorActionPreference = 'Stop'
# The ordinary install never calls this. No opt-in means no dependency lookup,
# no settings write, no SSH, and no microphone access.
if (-not $EnableAudio) {
    Write-Output 'Experimental audio forwarding is disabled. Add -EnableAudio to configure this optional Windows feature.'
    return
}
foreach ($required in @('Ports','Machine','Microphone','RemoteScript','ExpectedHost')) {
    if ([string]::IsNullOrWhiteSpace((Get-Variable -Name $required -ValueOnly))) { throw "-$required is required when enabling audio." }
}
if (-not $FFmpeg) { $FFmpeg = (Get-Command ffmpeg.exe -ErrorAction Stop).Source }
if (-not $FFplay) { $FFplay = (Get-Command ffplay.exe -ErrorAction Stop).Source }
$audioArgs = @('--machine',$Machine)
if ($DataDir) { $audioArgs += @('--data-dir',$DataDir) }
$audioArgs += @('audio','configure','--microphone',$Microphone,'--remote-script',$RemoteScript,'--expected-host',$ExpectedHost,'--ffmpeg',$FFmpeg,'--ffplay',$FFplay,'--json')
& $Ports @audioArgs
if ($LASTEXITCODE -ne 0) { throw 'Ports audio configuration failed.' }
Write-Output 'Experimental audio configured; microphone is OFF. In Ports press V, then Enter to start; S stops.'
