param(
    [Parameter(Mandatory = $true)][string]$TitlesBase64,
    [string]$OriginTitle = '',
    [switch]$ProbeOnly
)
$ErrorActionPreference = 'Stop'
try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    $portsTitles = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($TitlesBase64)) | ConvertFrom-Json
    $portsWindowCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ClassNameProperty, 'CASCADIA_HOSTING_WINDOW_CLASS')
    $portsWindows = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $portsWindowCondition)
    $portsTabCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem)
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
        $portsWindows = @($portsOrigin)
    }
    foreach ($portsTitle in $portsTitles) {
        foreach ($portsWindow in $portsWindows) {
            $portsTabs = $portsWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $portsTabCondition)
            foreach ($portsTab in $portsTabs) {
                if ($portsTab.Current.Name -cne $portsTitle) { continue }
                $portsSelection = $null
                if (-not $portsTab.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$portsSelection)) { continue }
                if ($ProbeOnly) {
                    Write-Output 'A matching live Terminal tab supports selection.'
                    exit 0
                }
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
                $portsHandle = [IntPtr]$portsWindow.Current.NativeWindowHandle
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
