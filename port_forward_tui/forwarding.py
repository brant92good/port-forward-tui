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
        self.ssh_port = None
        self.ssh_config = None
        self.machine_name = ''
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
        self.ssh_port = port(data['ssh_port']) if data.get('ssh_port') is not None else None
        self.ssh_config = data.get('ssh_config')
        if self.ssh_config is not None and (not isinstance(self.ssh_config, str) or not self.ssh_config):
            raise ValueError('Invalid SSH configuration path.')
        self.machine_name = data.get('machine_name', '')
        if not isinstance(self.machine_name, str) or len(self.machine_name) > 80:
            raise ValueError('Invalid machine name.')

    def save(self, rules: list[Forward]):
        self.directory.mkdir(parents=True, exist_ok=True)
        data = {"version": 1, "host": self.host, "keep_alive": self.keep_alive,
                "forwards": [asdict(r) for r in rules]}
        for key in ('ssh_port', 'ssh_config', 'machine_name'):
            if getattr(self, key):
                data[key] = getattr(self, key)
        payload = json.dumps(data, indent=2, ensure_ascii=False) + "\n"
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


def ssh_options(ssh_port=None, ssh_config=None):
    return (["-p", str(port(ssh_port))] if ssh_port is not None else []) + (["-F", str(ssh_config)] if ssh_config else [])


def ssh_command(host: str, rule: Forward, ssh_port=None, ssh_config=None) -> list[str]:
    return [SSH, *ssh_options(ssh_port, ssh_config), "-N", "-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
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
    connected: float | None = None


def requested(manager, rule_id: str) -> bool:
    """An interrupted connection still belongs to the user's current ON request."""
    return manager.status(rule_id) in ('ON', 'CONNECTING', 'RETRYING')


class TunnelManager:
    persistent = False
    def __init__(self, host: str, directory: Path):
        self.host, self.directory = host, Path(directory)
        settings = Store(self.directory)
        if settings.path.exists():
            settings.load()
        self.ssh_port, self.ssh_config = settings.ssh_port, settings.ssh_config
        self.running: dict[str, Running] = {}
        self.states: dict[str, str] = {}
        self.logs: dict[str, deque[str]] = {}
        self.log_lock = threading.Lock()
        self.wanted: dict[str, Forward] = {}
        self.retries: dict[str, float] = {}
        self.attempts: dict[str, int] = {}

    def log(self, rule_id: str, message: str):
        with self.log_lock:
            self.logs.setdefault(rule_id, deque(maxlen=60)).append(message.strip())

    def details(self, rule_id: str) -> str:
        with self.log_lock:
            message = "\n".join(self.logs.get(rule_id, ()))
        if rule_id in self.retries:
            seconds = max(0, int(self.retries[rule_id] - time.monotonic() + .999))
            message += f"\nNetwork connection interrupted. Retrying in {seconds}s. Enter stops retries; R retries now."
        return message

    def _read_errors(self, rule_id: str, process: subprocess.Popen):
        try:
            for line in process.stderr:
                self.log(rule_id, line)
        finally:
            process.stderr.close()

    def start(self, rule: Forward):
        if requested(self, rule.id):
            return
        self.wanted[rule.id] = rule
        self.attempts.pop(rule.id, None)
        self._start(rule)

    def _start(self, rule: Forward):
        self.retries.pop(rule.id, None)
        self.states[rule.id] = "ERROR"
        with self.log_lock:
            self.logs[rule.id] = deque(maxlen=60)
        for other in self.running.values():
            if other.rule.local_port == rule.local_port:
                self.log(rule.id, f"Local port {rule.local_port} is used by {other.rule.name}. Press E to change the local port.")
                self.wanted.pop(rule.id, None)
                return
        try:
            with socket.socket() as probe:
                probe.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
                probe.bind(("127.0.0.1", rule.local_port))
        except OSError:
            self.log(rule.id, f"Local port {rule.local_port} is already in use or unavailable. Press E to choose another local port.")
            self.wanted.pop(rule.id, None)
            return
        process = job = None
        try:
            job = ProcessJob()
            process = subprocess.Popen(ssh_command(self.host, rule, self.ssh_port, self.ssh_config), stdin=subprocess.DEVNULL,
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
            self.wanted.pop(rule.id, None)

    def _failed(self, rule: Forward, reason: str):
        self.log(rule.id, reason)
        details = self.details(rule.id).lower()
        permanent = ('permission denied', 'host key verification failed',
                     'remote host identification has changed', 'bad configuration option',
                     'bad owner or permissions', 'no such identity', 'unknown option',
                     'bad port', 'could not open user configuration file',
                     'cannot listen to port', 'address already in use',
                     'administratively prohibited')
        if any(text in details for text in permanent):
            self.states[rule.id] = 'ERROR'
            self.wanted.pop(rule.id, None)
            self.log(rule.id, 'Fix the SSH or local-port error, then press Enter to retry.')
            return
        if rule.id not in self.wanted:
            return
        attempt = self.attempts.get(rule.id, 0)
        delay = min(30, 2 ** min(attempt + 1, 5))
        self.attempts[rule.id] = attempt + 1
        self.retries[rule.id] = time.monotonic() + delay
        self.states[rule.id] = 'RETRYING'

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
        try:
            owned = listeners() if self.running else set()
        except OSError:
            # A transient Windows TCP-table error must not kill the supervisor
            # or turn every wanted connection off during a resume.
            owned = None
        for rule_id, running in list(self.running.items()):
            code = running.process.poll()
            if code is not None:
                self._dispose(running)
                del self.running[rule_id]
                self._failed(running.rule, f"SSH disconnected (exit {code}).")
            elif owned is not None and (running.process.pid, running.rule.local_port) in owned:
                self.states[rule_id] = "ON"
                if running.connected is None:
                    running.connected = time.monotonic()
                elif time.monotonic() - running.connected >= 30:
                    self.attempts.pop(rule_id, None)
            elif owned is not None and time.monotonic() - running.started > 20:
                self._dispose(running)
                del self.running[rule_id]
                self._failed(running.rule, 'Connection timed out. Waiting for the network or SSH server to recover.')
        for rule_id, deadline in list(self.retries.items()):
            if time.monotonic() >= deadline and rule_id in self.wanted:
                self._start(self.wanted[rule_id])

    def stop(self, rule_id: str):
        self.wanted.pop(rule_id, None)
        self.retries.pop(rule_id, None)
        self.attempts.pop(rule_id, None)
        running = self.running.pop(rule_id, None)
        if running:
            self._dispose(running)
        self.states[rule_id] = "OFF"

    def close(self):
        for rule_id in set(self.running) | set(self.wanted) | set(self.retries):
            self.stop(rule_id)

    def status(self, rule_id: str) -> str:
        return self.states.get(rule_id, "OFF")
