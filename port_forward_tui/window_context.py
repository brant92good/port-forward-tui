"""Machine context for a Terminal window; independent of its SSH client."""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import sys


def window_for_title(title):
    from port_forward_tui.views import focus_command
    command = focus_command()
    if len(command) != 1:
        return 0
    try:
        result = subprocess.run([*command, '-ResolveOrigin', '-OriginTitle', title],
                                capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=5)
        return int(json.loads(result.stdout)['window']) if result.returncode == 0 else 0
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired):
        return 0  # No guess from the foreground window: fall back to a picker.


def foreground_window():
    if sys.platform != 'win32':
        return 0
    api = ctypes.WinDLL('user32', use_last_error=True)
    api.GetForegroundWindow.restype = wintypes.HWND
    return int(api.GetForegroundWindow() or 0)


def process_started(pid):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *([ctypes.POINTER(ctypes.c_ulonglong)] * 4)]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        return 0
    values = [ctypes.c_ulonglong() for _ in range(4)]
    try:
        if not kernel.GetProcessTimes(handle, *(ctypes.byref(v) for v in values)):
            return 0
        return values[0].value + 504911232000000000  # Windows FILETIME -> .NET UTC ticks.
    finally:
        kernel.CloseHandle(handle)


def machine_for_window(root, window):
    """Only live registered views in the resolved caller's window contribute."""
    if not window:
        return None
    from port_forward_tui.views import process_alive
    candidates = []
    for path in (Path(root) / 'window-views').glob('*.json'):
        try:
            record = json.loads(path.read_text(encoding='utf-8-sig'))
            if (record.get('window') == window and record.get('machine')
                    and process_alive(record['pid']) and record.get('started')
                    and process_started(record['pid']) == record['started']):
                timestamp = float(record['last_focus'])
                if timestamp > 1e14:  # Native session trackers use .NET UTC ticks.
                    timestamp = (timestamp - 621355968000000000) / 10_000_000
                candidates.append((timestamp, record['machine']))
        except (OSError, ValueError, KeyError, TypeError):
            continue
    return max(candidates)[1] if candidates else None


def choose_machine(catalog, selector=None, *, picker=False, use_window=True, purpose=None):
    if selector:
        return catalog.get(selector)
    items = catalog.list()
    if not picker:
        if len(items) == 1:
            return items[0]
        if items and use_window and sys.stdout.isatty():
            from port_forward_tui.views import mark_origin
            key = machine_for_window(catalog.root, window_for_title(mark_origin()))
            if key:
                return catalog.get(key)
    from port_forward_tui.machine_ui import pick_machine
    key = pick_machine(catalog, purpose or 'Choose a machine to manage its ports')
    return catalog.get(key) if key else None
