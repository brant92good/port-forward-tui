param(
    [Parameter(Mandatory = $true)][string]$TitlesBase64,
    [string]$OriginTitle = '',
    [string]$ClosedTitle = '',
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
        $portsForeground = [PortsWindowFocus]::GetForegroundWindow().ToInt64()
        if ($portsForeground -ne $InvokeWindow -and $portsForeground -ne $WindowHandle) { exit 1 }
    }
    $portsTitles = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($TitlesBase64)) | ConvertFrom-Json
    $portsWindowCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ClassNameProperty, 'CASCADIA_HOSTING_WINDOW_CLASS')
    $portsWindows = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $portsWindowCondition)
    $portsTabCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem)
    if ($AfterPid -and $ClosedTitle) {
        $portsCloseDeadline = [DateTime]::UtcNow.AddSeconds(2)
        $portsClosed = $false
        while ([DateTime]::UtcNow -lt $portsCloseDeadline) {
            $portsForeground = [PortsWindowFocus]::GetForegroundWindow().ToInt64()
            if ($portsForeground -ne $InvokeWindow -and $portsForeground -ne $WindowHandle) { exit 1 }
            $portsPending = $false
            try {
                $portsOriginWindow = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$InvokeWindow)
                foreach ($portsTab in $portsOriginWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $portsTabCondition)) {
                    if ($portsTab.Current.Name -ceq $ClosedTitle) { $portsPending = $true; break }
                }
            } catch [System.Windows.Automation.ElementNotAvailableException] { }
            if (-not $portsPending) { $portsClosed = $true; break }
            Start-Sleep -Milliseconds 10
        }
        if (-not $portsClosed) { exit 1 }
    }
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
                $portsForeground = [PortsWindowFocus]::GetForegroundWindow().ToInt64()
                if ($InvokeWindow) {
                    if ($portsForeground -ne $InvokeWindow -and $portsForeground -ne $portsHandle.ToInt64()) { exit 1 }
                }
                if ([PortsWindowFocus]::IsIconic($portsHandle)) {
                    [void][PortsWindowFocus]::ShowWindow($portsHandle, 9)
                }
                $portsSelection.Select()
                $portsAfterSelect = [PortsWindowFocus]::GetForegroundWindow().ToInt64()
                if ($portsAfterSelect -ne $portsForeground -and $portsAfterSelect -ne $portsHandle.ToInt64()) { exit 1 }
                if ($portsAfterSelect -ne $portsHandle.ToInt64() -and -not [PortsWindowFocus]::SetForegroundWindow($portsHandle)) { exit 2 }
                $portsContentCondition = [System.Windows.Automation.AndCondition]::new([System.Windows.Automation.Condition[]]@(
                    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Text),
                    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::IsKeyboardFocusableProperty, $true),
                    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::IsOffscreenProperty, $false),
                    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty, $true)
                ))
                $portsContentDeadline = [DateTime]::UtcNow.AddSeconds(2)
                while ([DateTime]::UtcNow -lt $portsContentDeadline) {
                    if (-not $portsSelection.Current.IsSelected -or [PortsWindowFocus]::GetForegroundWindow() -ne $portsHandle) { exit 1 }
                    try {
                        $portsContent = $portsWindow.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $portsContentCondition)
                        if ($null -ne $portsContent) {
                            # Tab selection leaves keyboard focus in the tab strip.
                            # Check again after lookup before focusing the TUI.
                            if (-not $portsSelection.Current.IsSelected -or [PortsWindowFocus]::GetForegroundWindow() -ne $portsHandle) { exit 1 }
                            $portsContent.SetFocus()
                            $portsFocused = [System.Windows.Automation.AutomationElement]::FocusedElement
                            if ([PortsWindowFocus]::GetForegroundWindow() -eq $portsHandle -and $portsSelection.Current.IsSelected -and $null -ne $portsFocused -and
                                $portsFocused.Current.ControlType -eq [System.Windows.Automation.ControlType]::Text -and
                                ($portsFocused.GetRuntimeId() -join '.') -ceq ($portsContent.GetRuntimeId() -join '.')) { exit 0 }
                        }
                    } catch [System.Windows.Automation.ElementNotAvailableException] { }
                    Start-Sleep -Milliseconds 50
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
