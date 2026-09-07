param(
    [Parameter(Mandatory = $true)][string]$TitlesBase64,
    [string]$OriginTitle = '',
    [ValidateSet('all', 'window')][string]$Scope = 'all',
    [long]$WindowHandle = 0,
    [long]$InvokeWindow = 0,
    [int]$AfterPid = 0,
    [switch]$ProbeOnly
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PortsWindowFocus {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
}
'@
    if ($AfterPid) {
        if (-not $InvokeWindow) { exit 1 }
        $portsLauncher = Get-Process -Id $AfterPid -ErrorAction SilentlyContinue
        if ($portsLauncher -and -not $portsLauncher.WaitForExit(5000)) { exit 1 }
        Start-Sleep -Milliseconds 300
        $portsForeground = [PortsWindowFocus]::GetForegroundWindow().ToInt64()
        if ($portsForeground -ne $InvokeWindow -and $portsForeground -ne $WindowHandle) { exit 1 }
    }
    $portsTitles = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($TitlesBase64)) | ConvertFrom-Json
    $portsWindowCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ClassNameProperty, 'CASCADIA_HOSTING_WINDOW_CLASS')
    $portsWindows = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $portsWindowCondition)
    $portsTabCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem)
    $portsOriginHandle = 0
    if ($OriginTitle) {
        $portsOrigin = $null
        foreach ($portsWindow in $portsWindows) {
            foreach ($portsTab in $portsWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $portsTabCondition)) {
                if ($portsTab.Current.Name -ceq $OriginTitle) { $portsOrigin = $portsWindow; break }
            }
            if ($null -ne $portsOrigin) { break }
        }
        # Never fall back to another window when the launcher cannot be located.
        if ($null -eq $portsOrigin) { exit 1 }
        $portsOriginHandle = $portsOrigin.Current.NativeWindowHandle
        if ($Scope -eq 'window') { $portsWindows = @($portsOrigin) }
    } elseif ($Scope -eq 'window') {
        exit 1
    }
    foreach ($portsTitle in $portsTitles) {
        foreach ($portsWindow in $portsWindows) {
            if ($WindowHandle -and $portsWindow.Current.NativeWindowHandle -ne $WindowHandle) { continue }
            $portsTabs = $portsWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $portsTabCondition)
            foreach ($portsTab in $portsTabs) {
                if ($portsTab.Current.Name -cne $portsTitle) { continue }
                $portsSelection = $null
                if (-not $portsTab.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$portsSelection)) { continue }
                if ($ProbeOnly) {
                    @{ title = $portsTitle; window = $portsWindow.Current.NativeWindowHandle; origin = $portsOriginHandle } | ConvertTo-Json -Compress
                    exit 0
                }
                $portsHandle = [IntPtr]$portsWindow.Current.NativeWindowHandle
                # Accessibility enumeration can take time. Recheck immediately
                # before restoring or selecting anything, not just before lookup.
                if ($InvokeWindow) {
                    $portsForeground = [PortsWindowFocus]::GetForegroundWindow().ToInt64()
                    if ($portsForeground -ne $InvokeWindow -and $portsForeground -ne $portsHandle.ToInt64()) { exit 1 }
                }
                if ([PortsWindowFocus]::IsIconic($portsHandle)) {
                    [void][PortsWindowFocus]::ShowWindow($portsHandle, 9)
                }
                $portsSelection.Select()
                if ([PortsWindowFocus]::GetForegroundWindow() -eq $portsHandle -or [PortsWindowFocus]::SetForegroundWindow($portsHandle)) {
                    exit 0
                }
                exit 2
            }
        }
    }
    exit 1
} catch {
    Write-Error $_
    exit 2
}
