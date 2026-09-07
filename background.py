"""Detached per-user tunnel supervisor and bounded, authenticated local JSON IPC."""
from __future__ import annotations

import argparse
from dataclasses import asdict
import json
import os
from pathlib import Path
import secrets
import re
import socket
import socketserver
import subprocess
import sys
import threading
import time

from forwarding import DATA_DIR, Forward, InstanceLock, Store, TunnelManager, port, validate_host

PROTOCOL = 1
MAX_RESPONSE = 2 * 1024 * 1024


def exchange(directory: Path, command: str, **arguments) -> dict:
    endpoint = json.loads((directory / "endpoint.json").read_text(encoding="utf-8"))
    request = {"token": endpoint["token"], "protocol": PROTOCOL, "command": command, **arguments}
    with socket.create_connection(("127.0.0.1", endpoint["port"]), timeout=2) as connection:
        connection.settimeout(5)
        connection.sendall(json.dumps(request).encode("utf-8") + b"\n")
        with connection.makefile("rb") as response:
            raw = response.readline(MAX_RESPONSE + 1)
        if len(raw) > MAX_RESPONSE or not raw.endswith(b"\n"):
            raise OSError("Invalid response from background manager")
        result = json.loads(raw)
        if not result.get("ok"):
            raise OSError(result.get("error", "Background manager rejected the request"))
        return result


def launch_daemon(directory: Path):
    """Break away from the Terminal job and console, inheriting no open handles."""
    directory.mkdir(parents=True, exist_ok=True)
    # The daemon needs only the standard library. Avoid the Windows venv launcher.
    executable = getattr(sys, "_base_executable", sys.executable)
    command = [executable, str(Path(__file__).resolve()), "--serve", "--data-dir", str(directory.resolve())]
    flags = (subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP
             | subprocess.CREATE_BREAKAWAY_FROM_JOB)
    log_path = directory / "background.log"
    if log_path.exists() and log_path.stat().st_size > 256 * 1024:
        log_path.replace(directory / "background.previous.log")
    with log_path.open("ab") as log:
        try:
            process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                       close_fds=True, creationflags=flags)
        except PermissionError as error:
            raise OSError("Windows prevented background detachment. Launch this app from a normal Windows Terminal session.") from error
    return process


def ensure_daemon(directory: Path) -> dict:
    try:
        return exchange(directory, "status")
    except (OSError, ValueError, KeyError):
        pass
    # A second launcher cannot create a second daemon; the server holds daemon.lock.
    process = launch_daemon(directory)
    deadline = time.monotonic() + 8
    try:
        while time.monotonic() < deadline:
            try:
                return exchange(directory, "status")
            except (OSError, ValueError, KeyError):
                time.sleep(.1)
        raise OSError(f"Background manager did not start. See {directory / 'background.log'}")
    finally:
        # poll reaps a losing startup process without terminating the detached winner.
        process.poll()


class DaemonClient:
    persistent = True

    def __init__(self, host: str, directory: Path):
        self.host, self.directory = host, Path(directory)
        self.running: dict[str, bool] = {}
        self.states: dict[str, str] = {}
        self.logs: dict[str, str] = {}
        self.forwards: list[Forward] = []
        self._apply(ensure_daemon(self.directory))
        if not self.shared_favorites:
            raise OSError("The background manager needs an update. Run app.py --stop-daemon once, then reopen the app.")

    def _apply(self, result: dict):
        if result.get("host") != self.host:
            raise OSError("The background manager uses a different SSH host. Stop it before changing hosts.")
        self.running = dict.fromkeys(result["running"], True)
        self.states = result["states"]
        self.logs = result["details"]
        self.shared_favorites = "shared_favorites" in result.get("capabilities", [])
        self.forwards = [Forward(**row) for row in result.get("forwards", [])]

    def _call(self, command: str, **arguments):
        result = exchange(self.directory, command, **arguments)
        self._apply(result)
        return result

    def upsert(self, rule: Forward, expected: Forward | None = None, start: bool = False) -> Forward:
        result = self._call("upsert", rule=asdict(rule), expected=asdict(expected) if expected else None, start=start)
        return next(r for r in self.forwards if r.id == result["rule_id"])

    def delete(self, rule: Forward):
        self._call("delete", rule_id=rule.id, expected=asdict(rule))

    def poll(self):
        try:
            self._call("status")
        except (OSError, ValueError, KeyError) as error:
            for rule_id in self.running:
                self.states[rule_id] = "ERROR"
                self.logs[rule_id] = "Background manager disconnected. Reopen the app to reconnect."
            self.running = {}
            raise OSError(str(error)) from error

    def start(self, rule: Forward):
        try:
            self._call("start", rule_id=rule.id)
        except (OSError, ValueError, KeyError) as error:
            self.states[rule.id] = "ERROR"
            self.logs[rule.id] = str(error)

    def stop(self, rule_id: str):
        self._call("stop", rule_id=rule_id)

    def close(self):
        """Explicit stop-all action. UI detach intentionally does not call this."""
        self._call("stop_all")

    def status(self, rule_id: str) -> str:
        return self.states.get(rule_id, "OFF")

    def details(self, rule_id: str) -> str:
        return self.logs.get(rule_id, "")


class Supervisor:
    def __init__(self, store: Store, manager: TunnelManager):
        self.store, self.manager = store, manager
        self.token = secrets.token_hex(32)
        self.lock = threading.RLock()
        self.stopping = threading.Event()

    def snapshot(self):
        return {"ok": True, "protocol": PROTOCOL, "pid": os.getpid(), "host": self.manager.host,
                "capabilities": ["shared_favorites"], "forwards": [asdict(r) for r in self.store.forwards],
                "running": list(self.manager.running), "states": dict(self.manager.states),
                "details": {key: self.manager.details(key)[-6000:] for key in self.manager.states}}

    def dispatch(self, request: dict) -> dict:
        token = request.get("token")
        if not isinstance(token, str) or not secrets.compare_digest(token, self.token):
            return {"ok": False, "error": "Unauthorized"}
        if request.get("protocol") != PROTOCOL:
            return {"ok": False, "error": "Incompatible background protocol; restart the background manager"}
        with self.lock:
            action = request.get("command")
            # Stopping must still work if the user breaks or changes the settings.
            if action not in ("stop", "stop_all", "shutdown"):
                self.store.load()
                if self.store.host != self.manager.host:
                    raise ValueError("SSH host changed. Stop the background manager before switching hosts.")
            result_rule = None
            if action == "start":
                rule = next((r for r in self.store.forwards if r.id == request.get("rule_id")), None)
                if rule is None:
                    raise ValueError("Save the forward before starting it.")
                self.manager.start(rule)
            elif action == "upsert":
                row = request.get("rule")
                if not isinstance(row, dict) or not re.fullmatch(r"[a-f0-9]{32}", str(row.get("id", ""))):
                    raise ValueError("Invalid favorite ID")
                name = row.get("name")
                if not isinstance(name, str) or not 1 <= len(name) <= 80:
                    raise ValueError("A favorite name must contain 1 to 80 characters")
                rule = Forward(row["id"], name, port(row["local_port"]), port(row["remote_port"]))
                current = next((r for r in self.store.forwards if r.id == rule.id), None)
                expected = request.get("expected")
                if expected is not None and (current is None or asdict(current) != expected):
                    raise ValueError("This favorite changed in another view. Reopen Edit and try again.")
                if current is not None and expected is None and current != rule:
                    raise ValueError("This favorite changed in another view. Refresh and try again.")
                duplicate = next((r for r in self.store.forwards if r.id != rule.id
                                  and (r.local_port, r.remote_port) == (rule.local_port, rule.remote_port)), None)
                if duplicate:
                    if current:
                        raise ValueError("That mapping is already saved in another favorite.")
                    rule = duplicate
                else:
                    updated = [rule if r.id == rule.id else r for r in self.store.forwards]
                    if current is None:
                        updated.append(rule)
                    self.store.save(updated)
                    if current and current != rule and rule.id in self.manager.running:
                        self.manager.stop(rule.id)
                        self.manager.start(rule)
                if request.get("start") is True:
                    self.manager.start(rule)
                result_rule = rule.id
            elif action == "delete":
                current = next((r for r in self.store.forwards if r.id == request.get("rule_id")), None)
                if current:
                    if asdict(current) != request.get("expected"):
                        raise ValueError("This favorite changed in another view. Select it again before deleting.")
                    self.store.save([r for r in self.store.forwards if r.id != current.id])
                    self.manager.stop(current.id)
            elif action == "stop":
                rule_id = request.get("rule_id")
                if not isinstance(rule_id, str):
                    raise ValueError("Invalid forward ID")
                self.manager.stop(rule_id)
            elif action in ("stop_all", "shutdown"):
                self.manager.close()
                if action == "shutdown":
                    self.stopping.set()
            elif action != "status":
                raise ValueError("Unknown command")
            self.manager.poll()
            result = self.snapshot()
            if result_rule:
                result["rule_id"] = result_rule
            return result


class LocalServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = False


def serve(directory: Path):
    directory = directory.resolve()
    lock = InstanceLock(directory, "daemon.lock")
    manager = server = None
    server_started = False
    endpoint_path = directory / "endpoint.json"
    try:
        store = Store(directory)
        store.load()
        validate_host(store.host)
        manager = TunnelManager(store.host, directory)
        supervisor = Supervisor(store, manager)

        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                self.connection.settimeout(2)
                try:
                    raw = self.rfile.readline(65537)
                    if len(raw) > 65536 or not raw.endswith(b"\n"):
                        return
                    request = json.loads(raw)
                    if not isinstance(request, dict):
                        return
                    response = supervisor.dispatch(request)
                except (OSError, ValueError, KeyError, TypeError) as error:
                    response = {"ok": False, "error": str(error)}
                try:
                    self.wfile.write(json.dumps(response).encode("utf-8") + b"\n")
                except OSError:
                    pass

        server = LocalServer(("127.0.0.1", 0), Handler)
        endpoint = {"protocol": PROTOCOL, "pid": os.getpid(), "port": server.server_address[1],
                    "token": supervisor.token}
        temporary = directory / "endpoint.tmp"
        temporary.write_text(json.dumps(endpoint), encoding="utf-8")
        os.replace(temporary, endpoint_path)
        threading.Thread(target=server.serve_forever, kwargs={"poll_interval": .2}, daemon=True).start()
        server_started = True
        while not supervisor.stopping.wait(.25):
            with supervisor.lock:
                manager.poll()
    finally:
        if server_started:
            server.shutdown()
        if server:
            server.server_close()
        if manager:
            manager.close()
        endpoint_path.unlink(missing_ok=True)
        lock.close()


def main():
    parser = argparse.ArgumentParser(description="Port Forward TUI background supervisor")
    parser.add_argument("--data-dir", type=Path, default=DATA_DIR)
    parser.add_argument("--serve", action="store_true")
    parser.add_argument("--status", action="store_true")
    parser.add_argument("--shutdown", action="store_true")
    options = parser.parse_args()
    if options.serve:
        serve(options.data_dir)
    elif options.shutdown:
        exchange(options.data_dir, "shutdown")
        print("Background manager and its tunnels stopped.")
    else:
        result = exchange(options.data_dir, "status")
        print(json.dumps({k: result[k] for k in ("pid", "host", "running", "states")}, indent=2))


if __name__ == "__main__":
    main()
