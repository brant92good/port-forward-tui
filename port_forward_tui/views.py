"""Live TUI view registry and explicit, on-demand Windows Terminal focus."""
import base64
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

from port_forward_tui.focus_settings import read_scope


def focus_command() -> list[str]:
    from port_forward_tui.build_focus_helper import ensure_helper
    try:
        return [str(ensure_helper())]
    except (OSError, subprocess.TimeoutExpired):
        powershell = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32/WindowsPowerShell/v1.0/powershell.exe"
        return [str(powershell), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                "-File", str((Path(__file__).resolve().parents[1] / "native/focus_existing.ps1"))]


def mark_origin() -> str:
    """Give this launcher tab a unique title so its window can be found exactly."""
    title = "Shortcut | " + uuid.uuid4().hex
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.SetConsoleTitleW.argtypes = [wintypes.LPCWSTR]
    kernel.SetConsoleTitleW.restype = wintypes.BOOL
    if not sys.stdout.isatty() or not kernel.SetConsoleTitleW(title):
        raise OSError("Cannot identify the current Terminal tab")
    return title


def delayed_focus(command: list[str]) -> bool:
    """Finish focus after Terminal has disposed of the short-lived launcher tab."""
    try:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL, close_fds=True,
                                   creationflags=subprocess.CREATE_NO_WINDOW | subprocess.CREATE_BREAKAWAY_FROM_JOB)
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        user32.AllowSetForegroundWindow.argtypes = [wintypes.DWORD]
        user32.AllowSetForegroundWindow(process.pid)
        return True
    except OSError:
        return False


def native_focus(command: list[str], trace=None) -> bool:
    """One detached helper probes, signals a match, then completes the handoff."""
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateEventW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR]
    kernel.CreateEventW.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.WaitForMultipleObjects.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE), wintypes.BOOL, wintypes.DWORD]
    kernel.WaitForMultipleObjects.restype = wintypes.DWORD
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    name = "Local\\PortsFocus-" + uuid.uuid4().hex
    ready = kernel.CreateEventW(None, True, False, name)
    if not ready:
        return False
    handle = None
    process = None
    try:
        if trace:
            trace.mark("helper_spawn")
        process = subprocess.Popen([*command, "-ReadyEvent", name, "-AfterPid", str(os.getpid())],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True,
            creationflags=subprocess.CREATE_NO_WINDOW | subprocess.CREATE_BREAKAWAY_FROM_JOB)
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        user32.AllowSetForegroundWindow.argtypes = [wintypes.DWORD]
        user32.AllowSetForegroundWindow(process.pid)
        handle = kernel.OpenProcess(0x100000, False, process.pid)
        if not handle:
            return False
        handles = (wintypes.HANDLE * 2)(ready, handle)
        # A match wakes immediately. No match exits the child; a stalled lookup
        # is bounded and cannot select a tab after the fallback view opens.
        if kernel.WaitForMultipleObjects(2, handles, False, 5000) == 0:
            return True
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        return False
    except (OSError, subprocess.TimeoutExpired):
        if process and process.poll() is None:
            process.kill()
        return False
    finally:
        if handle:
            kernel.CloseHandle(handle)
        kernel.CloseHandle(ready)


def process_alive(pid: int) -> bool:
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.WaitForSingleObject.restype = wintypes.DWORD
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x100000, False, pid)
    if not handle:
        return False
    try:
        return kernel.WaitForSingleObject(handle, 0) == 258
    finally:
        kernel.CloseHandle(handle)


class ViewRegistration:
    def __init__(self, directory: Path, host: str, *, catalog_root=None, machine=None):
        self.directory = Path(directory) / "views"
        self.directory.mkdir(parents=True, exist_ok=True)
        identifier = uuid.uuid4().hex
        self.path = self.directory / f"{identifier}.json"
        self.title = f"Ports | {host} | {identifier[:6]}"
        self.data = {"pid": os.getpid(), "title": self.title}
        self.context_path = None
        if catalog_root and machine:
            from port_forward_tui.window_context import process_started
            self.context_path = Path(catalog_root) / 'window-views' / self.path.name
            self.context_path.parent.mkdir(parents=True, exist_ok=True)
            self.data['machine'] = machine
            self.data['started'] = process_started(os.getpid())
        self.touch()
        if sys.stdout.isatty():
            # The console may not have VT output enabled until Textual starts.
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.SetConsoleTitleW.argtypes = [wintypes.LPCWSTR]
            kernel.SetConsoleTitleW.restype = wintypes.BOOL
            kernel.SetConsoleTitleW(self.title)
            if self.context_path:
                from port_forward_tui.window_context import window_for_title
                self.data['window'] = window_for_title(self.title)
                self.touch()

    def touch(self):
        self.data["last_focus"] = time.time()
        temporary = self.path.with_suffix(".tmp")
        temporary.write_text(json.dumps(self.data), encoding="utf-8")
        os.replace(temporary, self.path)
        if self.context_path:
            temporary = self.context_path.with_suffix('.tmp')
            temporary.write_text(json.dumps(self.data), encoding='utf-8')
            os.replace(temporary, self.context_path)

    def focused(self):
        if self.context_path:
            from port_forward_tui.window_context import foreground_window
            self.data['window'] = foreground_window()
        self.touch()

    def close(self):
        self.path.unlink(missing_ok=True)
        if self.context_path:
            self.context_path.unlink(missing_ok=True)


def live_titles(directory: Path) -> list[str]:
    views = []
    for path in (Path(directory) / "views").glob("*.json"):
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
            pid, title = record["pid"], record["title"]
            if not isinstance(pid, int) or pid <= 0 or not isinstance(title, str) or not title.startswith("Ports | "):
                continue
            if not process_alive(pid):
                path.unlink(missing_ok=True)
                continue
            views.append((float(record.get("last_focus", 0)), title))
        except (OSError, ValueError, KeyError, TypeError):
            continue
    return [title for _, title in sorted(views, reverse=True)]


def focus_existing(directory: Path, probe_only: bool = False) -> bool:
    titles = live_titles(directory)
    if not titles:
        return False
    payload = base64.b64encode(json.dumps(titles).encode("utf-8")).decode("ascii")
    executable = focus_command()
    command = [*executable, "-TitlesBase64", payload]
    scope = read_scope(directory)
    command.extend(["-Scope", scope])
    if probe_only:
        # A read-only probe cannot identify the caller's window without a marker.
        if scope == "window":
            return False
    else:
        # Resolve the invoking Terminal window even for a global search. The user
        # may switch applications while the accessibility lookup is running.
        try:
            origin_title = mark_origin()
            command.extend(["-OriginTitle", origin_title])
        except OSError:
            return False
    if not probe_only and len(executable) == 1:
        return native_focus(command)
    command.append("-ProbeOnly")
    try:
        result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, encoding="utf-8", timeout=5,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        if result.returncode != 0:
            return False
        if probe_only:
            return True
        target = json.loads(result.stdout)
        if not isinstance(target.get("origin"), int) or not target["origin"]:
            return False
        titles = base64.b64encode(json.dumps([target["title"]]).encode()).decode("ascii")
        return delayed_focus([*executable, "-TitlesBase64", titles,
                              "-WindowHandle", str(target["window"]), "-InvokeWindow", str(target["origin"]),
                              "-ClosedTitle", origin_title,
                              "-AfterPid", str(os.getpid())])
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired):
        return False
