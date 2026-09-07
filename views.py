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

from focus_settings import read_scope


def mark_origin() -> str:
    """Give this launcher tab a unique title so its window can be found exactly."""
    title = "Shortcut | " + uuid.uuid4().hex
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.SetConsoleTitleW.argtypes = [wintypes.LPCWSTR]
    kernel.SetConsoleTitleW.restype = wintypes.BOOL
    if not sys.stdout.isatty() or not kernel.SetConsoleTitleW(title):
        raise OSError("Cannot identify the current Terminal tab")
    return title


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
    def __init__(self, directory: Path, host: str):
        self.directory = Path(directory) / "views"
        self.directory.mkdir(parents=True, exist_ok=True)
        identifier = uuid.uuid4().hex
        self.path = self.directory / f"{identifier}.json"
        self.title = f"Ports | {host} | {identifier[:6]}"
        self.data = {"pid": os.getpid(), "title": self.title}
        self.touch()
        if sys.stdout.isatty():
            # The console may not have VT output enabled until Textual starts.
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.SetConsoleTitleW.argtypes = [wintypes.LPCWSTR]
            kernel.SetConsoleTitleW.restype = wintypes.BOOL
            kernel.SetConsoleTitleW(self.title)

    def touch(self):
        self.data["last_focus"] = time.time()
        temporary = self.path.with_suffix(".tmp")
        temporary.write_text(json.dumps(self.data), encoding="utf-8")
        os.replace(temporary, self.path)

    def close(self):
        self.path.unlink(missing_ok=True)


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
    powershell = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32/WindowsPowerShell/v1.0/powershell.exe"
    command = [str(powershell), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
               "-File", str(Path(__file__).with_name("focus_existing.ps1")), "-TitlesBase64", payload]
    scope = read_scope(directory)
    if scope == "window":
        # Probe mode never changes a title or guesses which window owns this view.
        if probe_only:
            return False
        try:
            command.extend(["-OriginTitle", mark_origin()])
        except OSError:
            return False
    if probe_only:
        command.append("-ProbeOnly")
    try:
        result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, timeout=5,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False
