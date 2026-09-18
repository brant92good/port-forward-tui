# Avoid selection queries while another window is active

This source candidate changes the Windows Terminal tab tracker. It does not
change the installed helper, released assets, polling interval or return shortcuts.

Previously every observation requested the tracked tab's selection pattern
before checking its foreground window. Observations run every 250 ms and on
global UI Automation focus events. A background window therefore still incurred
the selection-pattern request, even though the observation could not update its
last-focus record.

The candidate checks the foreground window first. If it is not the tracked
window, selection access is skipped and the existing active-state flag becomes
false. When the tracked window is foreground, selection is read and foreground
is checked again; switching away during that read rejects the observation.
Forced observations, re-entry detection, owner checks, machine identity and
window-scope selection retain their existing behavior.

tests/tracker_observation.ps1 compiles the production predicate with injected
getters and checks background/no-foreground access, selected/unselected tabs,
a switch during selection access and foreground re-entry. It performs no UIA
query and opens no window. Hosted Windows CI runs it before the native suite.
Those deterministic tests cannot establish real desktop CPU consumption,
shortcut latency, physical focus behavior or any effect on game frame times.

Global UIA subscriptions, the 250 ms owner loop and initial tab discovery remain.
The final foreground check and record write are not atomic. An away-and-back
transition may also escape these checks. No claim is made that all background
work is eliminated or that this fixes game lag. A later controlled measurement
must use interval CPU deltas and preserve the same workload and window state.

Distribution remains through a future qualified leaf release, followed by the
integration's exact helper/pin and the optional personal setup. A source branch
or green unit check alone is not an installed update.
