function Get-SetupHost {
    param([string]$Value, [string]$SettingsPath, [string]$Key = 'host', [switch]$NonInteractive)
    if (-not $Value -and (Test-Path -LiteralPath $SettingsPath)) {
        try { $Value = (Get-Content -LiteralPath $SettingsPath -Raw -Encoding UTF8 | ConvertFrom-Json).$Key }
        catch { throw "Could not read $SettingsPath. Keep a backup and repair its JSON before setup." }
    }
    if (-not $Value) { return '' }
    if ($Value -notmatch '^[A-Za-z0-9_][A-Za-z0-9_.@-]*$') {
        throw 'Enter only the SSH name, such as workbox or user@hostname, without ssh, spaces, or extra options.'
    }
    return $Value
}

function Assert-SetupCommand {
    param([string]$Name, [string]$Fix)
    if (-not (Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue)) {
        throw "Missing $Name. $Fix"
    }
}
