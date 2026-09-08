"""Saved local forwards and supervised Windows OpenSSH processes."""
from __future__ import annotations

import ctypes
from ctypes import wintypes
from collections import deque
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import threading
import time
import uuid

DEFAULT_HOST = ""
DATA_DIR = Path(os.environ.get("LOCALAPPDATA", Path.home())) / "PortForwardTUI"
SSH = str(Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32/OpenSSH/ssh.exe")


def port(value: str | int) -> int:
    if isinstance(value, bool) or not re.fullmatch(r"[0-9]{1,5}", str(value)):
        raise ValueError("Use a port number from 1 to 65535.")
    result = int(value)
    if not 1 <= result <= 65535:
        raise ValueError("Ports must be between 1 and 65535.")
    return result


def quick_ports(value: str) -> tuple[int, int]:
    parts = value.strip().split(":")
    if len(parts) == 1:
        number = port(parts[0])
        return number, number
    if len(parts) == 2:
        return port(parts[0].strip()), port(parts[1].strip())
    raise ValueError("Type PORT or LOCAL:REMOTE, for example 8000 or 18000:8000.")


def validate_host(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.@-]*", value):
        raise ValueError("Use an SSH config alias or user@hostname (no spaces or options).")
    return value


@dataclass(frozen=True)
class Forward:
    id: str
    name: str
    local_port: int
    remote_port: int

    @classmethod
    def make(cls, local_port: int, remote_port: int, name: str = "") -> Forward:
        return cls(uuid.uuid4().hex, name.strip() or f"Port {remote_port}", port(local_port), port(remote_port))


class Store:
    def __init__(self, directory: Path = DATA_DIR):
        self.directory = Path(directory)
        self.path = self.directory / "forwards.json"
        self.host = DEFAULT_HOST
        self.keep_alive = True
        self.forwards: list[Forward] = []

    def load(self):
        if not self.path.exists():
            self.forwards = [Forward.make(p, p, n) for p, n in (
                (3000, "Web app"), (5173, "Vite"), (8000, "API / dev server"),
                (8080, "Web server"), (8888, "Jupyter"), (6006, "TensorBoard"))]
            self.save(self.forwards)
            return
        data = json.loads(self.path.read_text(encoding="utf-8"))
        if not isinstance(data, dict) or not isinstance(data.get("forwards"), list):
            raise ValueError("Saved connections must be a JSON object with a forwards list. Keep a backup before repairing it.")
        if data.get("version") != 1:
            raise ValueError("Unsupported favorites file version.")
        host = data["host"]
        if host:
            validate_host(host)
        elif host != "":
            raise ValueError("Invalid SSH host alias in favorites file.")
        keep_alive = data.get("keep_alive", True)
        if not isinstance(keep_alive, bool):
            raise ValueError("keep_alive must be true or false.")
        rules = []
        for row in data["forwards"]:
            if not isinstance(row, dict):
                raise ValueError("Each saved connection must be a JSON object with a name and two ports.")
            if not re.fullmatch(r"[a-f0-9]{32}", row["id"]):
                raise ValueError("Invalid saved forward ID.")
            if not isinstance(row["name"], str) or len(row["name"]) > 80:
                raise ValueError("Saved names must be text, at most 80 characters.")
            rules.append(Forward(row["id"], row["name"], port(row["local_port"]), port(row["remote_port"])))
        if len({r.id for r in rules}) != len(rules):
            raise ValueError("Duplicate saved forward IDs.")
        self.host, self.forwards = host, rules
        self.keep_alive = keep_alive

    def save(self, rules: list[Forward]):
        self.directory.mkdir(parents=True, exist_ok=True)
        payload = json.dumps({"version": 1, "host": self.host, "keep_alive": self.keep_alive,
                              "forwards": [asdict(r) for r in rules]}, indent=2, ensure_ascii=False) + "\n"
        fd, temporary = tempfile.mkstemp(prefix="forwards-", suffix=".tmp", dir=self.directory)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as output:
                output.write(payload)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, self.path)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
        self.forwards = list(rules)


class InstanceLock:
    """Windows releases this byte lock even if the terminal is closed abruptly."""
    def __init__(self, directory: Path, filename: str = "manager.lock"):
        import msvcrt
        directory.mkdir(parents=True, exist_ok=True)
        self.file = (directory / filename).open("a+b")
        self.file.seek(0, 2)
        if self.file.tell() == 0:
            self.file.write(b"0")
            self.file.flush()
        self.file.seek(0)
        try:
            msvcrt.locking(self.file.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            self.file.close()
            raise RuntimeError(f"Port manager is already running ({filename}). Switch to its existing Terminal tab.") from None

    def close(self):
        self.file.close()


class ProcessJob:
    """Kernel-owned kill-on-close job: no orphan tunnels after a tab/process dies."""
    def __init__(self):
        class BasicLimit(ctypes.Structure):
            _fields_ = [("process_time", ctypes.c_int64), ("job_time", ctypes.c_int64),
                        ("flags", wintypes.DWORD), ("min_ws", ctypes.c_size_t),
                        ("max_ws", ctypes.c_size_t), ("active", wintypes.DWORD),
                        ("affinity", ctypes.c_size_t), ("priority", wintypes.DWORD),
                        ("scheduling", wintypes.DWORD)]

        class IoCounters(ctypes.Structure):
            _fields_ = [(name, ctypes.c_uint64) for name in
                        ("read_ops", "write_ops", "other_ops", "read_bytes", "write_bytes", "other_bytes")]

        class ExtendedLimit(ctypes.Structure):
            _fields_ = [("basic", BasicLimit), ("io", IoCounters),
                        ("process_mem", ctypes.c_size_t), ("job_mem", ctypes.c_size_t),
                        ("peak_process", ctypes.c_size_t), ("peak_job", ctypes.c_size_t)]

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel.CreateJobObjectW.restype = wintypes.HANDLE
        kernel.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
        kernel.SetInformationJobObject.restype = wintypes.BOOL
        kernel.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel.AssignProcessToJobObject.restype = wintypes.BOOL
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        self.kernel, self.handle = kernel, kernel.CreateJobObjectW(None, None)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())
        limit = ExtendedLimit()
        limit.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not kernel.SetInformationJobObject(self.handle, 9, ctypes.byref(limit), ctypes.sizeof(limit)):
            error = ctypes.WinError(ctypes.get_last_error())
            self.close()
            raise error

    def attach(self, process: subprocess.Popen):
        if not self.kernel.AssignProcessToJobObject(self.handle, int(process._handle)):
            raise ctypes.WinError(ctypes.get_last_error())

    def close(self):
        if self.handle:
            self.kernel.CloseHandle(self.handle)
            self.handle = None


def listeners() -> set[tuple[int, int]]:
    """Read TCP listener ownership without opening test connections to services."""
    iphlp = ctypes.WinDLL("iphlpapi")
    function = iphlp.GetExtendedTcpTable
    function.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.ULONG), wintypes.BOOL,
                         wintypes.ULONG, ctypes.c_int, wintypes.ULONG]
    function.restype = wintypes.DWORD
    size = wintypes.ULONG(0)
    function(None, ctypes.byref(size), False, socket.AF_INET, 3, 0)
    for _ in range(3):
        buffer = ctypes.create_string_buffer(size.value)
        result = function(buffer, ctypes.byref(size), False, socket.AF_INET, 3, 0)
        if result == 122:  # table grew between calls
            continue
        if result:
            raise OSError(result, "Could not read local TCP listeners")
        count = ctypes.cast(buffer, ctypes.POINTER(wintypes.DWORD))[0]
        rows = ctypes.cast(ctypes.addressof(buffer) + 4, ctypes.POINTER(wintypes.DWORD))
        return {(int(rows[i * 6 + 5]), socket.ntohs(rows[i * 6 + 2] & 0xffff))
                for i in range(count) if rows[i * 6] == 2}
    return set()


def ssh_command(host: str, rule: Forward) -> list[str]:
    return [SSH, "-N", "-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
            "-o", "ExitOnForwardFailure=yes", "-o", "ConnectTimeout=10",
            "-o", "ConnectionAttempts=1", "-o", "ServerAliveInterval=15",
            "-o", "ServerAliveCountMax=3", "-o", "ControlMaster=no",
            "-o", "ControlPath=none", "-o", "ForkAfterAuthentication=no",
            "-o", "LogLevel=ERROR", "-L",
            f"127.0.0.1:{rule.local_port}:127.0.0.1:{rule.remote_port}", host]


@dataclass
class Running:
    rule: Forward
    process: subprocess.Popen
    job: ProcessJob
    started: float
    reader: threading.Thread | None = None


class TunnelManager:
    persistent = False
    def __init__(self, host: str, directory: Path):
        self.host, self.directory = host, Path(directory)
        self.running: dict[str, Running] = {}
        self.states: dict[str, str] = {}
        self.logs: dict[str, deque[str]] = {}
        self.log_lock = threading.Lock()

    def log(self, rule_id: str, message: str):
        with self.log_lock:
            self.logs.setdefault(rule_id, deque(maxlen=60)).append(message.strip())

    def details(self, rule_id: str) -> str:
        with self.log_lock:
            return "\n".join(self.logs.get(rule_id, ()))

    def _read_errors(self, rule_id: str, process: subprocess.Popen):
        try:
            for line in process.stderr:
                self.log(rule_id, line)
        finally:
            process.stderr.close()

    def start(self, rule: Forward):
        if rule.id in self.running:
            return
        self.states[rule.id] = "ERROR"
        with self.log_lock:
            self.logs[rule.id] = deque(maxlen=60)
        for other in self.running.values():
            if other.rule.local_port == rule.local_port:
                self.log(rule.id, f"Local port {rule.local_port} is used by {other.rule.name}. Press E to change the local port.")
                return
        try:
            with socket.socket() as probe:
                probe.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
                probe.bind(("127.0.0.1", rule.local_port))
        except OSError:
            self.log(rule.id, f"Local port {rule.local_port} is already in use or unavailable. Press E to choose another local port.")
            return
        process = job = None
        try:
            job = ProcessJob()
            process = subprocess.Popen(ssh_command(self.host, rule), stdin=subprocess.DEVNULL,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                       encoding="utf-8", errors="replace",
                                       creationflags=subprocess.CREATE_NO_WINDOW)
            job.attach(process)
            running = Running(rule, process, job, time.monotonic())
            running.reader = threading.Thread(target=self._read_errors, args=(rule.id, process), daemon=True)
            running.reader.start()
            self.running[rule.id] = running
            self.states[rule.id] = "CONNECTING"
        except (OSError, ValueError) as error:
            if job:
                job.close()
            if process:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=3)
                if process.stderr:
                    process.stderr.close()
            self.log(rule.id, str(error))

    def _dispose(self, running: Running):
        running.job.close()
        try:
            running.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            running.process.kill()
            running.process.wait(timeout=3)
        if running.reader:
            running.reader.join(timeout=1)

    def poll(self):
        owned = listeners() if self.running else set()
        for rule_id, running in list(self.running.items()):
            code = running.process.poll()
            if code is not None:
                self._dispose(running)
                del self.running[rule_id]
                self.states[rule_id] = "ERROR"
                self.log(rule_id, f"SSH disconnected (exit {code}). Select this row and press Enter to retry.")
            elif (running.process.pid, running.rule.local_port) in owned:
                self.states[rule_id] = "ON"
            elif time.monotonic() - running.started > 20:
                self.stop(rule_id)
                self.states[rule_id] = "ERROR"
                self.log(rule_id, "Connection timed out. Check the SSH host and Tailscale connection, then press Enter to retry.")

    def stop(self, rule_id: str):
        running = self.running.pop(rule_id, None)
        if running:
            self._dispose(running)
        self.states[rule_id] = "OFF"

    def close(self):
        for rule_id in list(self.running):
            self.stop(rule_id)

    def status(self, rule_id: str) -> str:
        return self.states.get(rule_id, "OFF")
