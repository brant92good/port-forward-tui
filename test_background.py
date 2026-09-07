import json
from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest

from app import PortApp
from background import DaemonClient, Supervisor, exchange
from forwarding import Forward, Store
from test_app import FakeManager


class BackgroundSettingsTests(unittest.TestCase):
    def test_background_enabled_for_new_and_legacy_settings(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            self.assertTrue(store.keep_alive)
            self.assertEqual(len(store.forwards), 6)
            data = json.loads(store.path.read_text())
            del data["keep_alive"]
            store.path.write_text(json.dumps(data))
            store.load()
            self.assertTrue(store.keep_alive)
            store.keep_alive = False
            store.save(store.forwards)
            store.load()
            self.assertFalse(store.keep_alive)

    def test_control_protocol_requires_auth_and_saved_rules(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            store.host = "workbox"
            store.save(store.forwards)
            manager = FakeManager()
            manager.host = "workbox"
            supervisor = Supervisor(store, manager)
            request = {"protocol": 1, "command": "start", "rule_id": store.forwards[0].id}
            self.assertFalse(supervisor.dispatch(request)["ok"])
            self.assertFalse(manager.running)
            request["token"] = supervisor.token
            self.assertTrue(supervisor.dispatch(request)["ok"])
            self.assertEqual(len(manager.running), 1)
            request["rule_id"] = "unknown"
            with self.assertRaises(ValueError):
                supervisor.dispatch(request)
            request["command"] = "stop_all"
            store.path.write_text("invalid settings")
            self.assertFalse(supervisor.dispatch(request)["running"])
            request["command"] = "shutdown"
            self.assertTrue(supervisor.dispatch(request)["ok"])
            self.assertTrue(supervisor.stopping.is_set())


class BackgroundKeyboardTests(unittest.IsolatedAsyncioTestCase):
    async def test_quit_detaches_and_reopen_shows_same_forward(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            manager = FakeManager()
            manager.persistent = True
            first = PortApp(store, manager)
            async with first.run_test() as pilot:
                await pilot.press("enter", "q")
                self.assertTrue(manager.running)
            second = PortApp(store, manager)
            async with second.run_test() as pilot:
                self.assertEqual(manager.status(store.forwards[0].id), "ON")
                await pilot.press("s")
                self.assertFalse(manager.running)


class DetachedProcessTests(unittest.TestCase):
    def test_background_control_server_and_reattach(self):
        # Exercise the real server even on runners that prohibit job breakaway.
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            store = Store(directory)
            store.load()
            store.host = "workbox"
            store.save(store.forwards)
            server = subprocess.Popen([sys._base_executable, str(Path(__file__).with_name("background.py")),
                                       "--serve", "--data-dir", folder],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                      creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                deadline = time.monotonic() + 8
                while not (directory / "endpoint.json").exists() and time.monotonic() < deadline:
                    self.assertIsNone(server.poll(), "Control server exited during startup")
                    time.sleep(.05)
                self.assertTrue((directory / "endpoint.json").exists())
                first = DaemonClient("workbox", directory)
                first.poll()
                second = DaemonClient("workbox", directory)
                second.poll()
                self.assertEqual(exchange(directory, "status")["pid"], server.pid)
                a = Forward.make(19000, 9000, "First view")
                b = Forward.make(19001, 9001, "Second view")
                with ThreadPoolExecutor(max_workers=2) as clients:
                    saves = [clients.submit(first.upsert, a), clients.submit(second.upsert, b)]
                    for save in saves:
                        save.result(timeout=5)
                first.poll()
                second.poll()
                self.assertEqual(first.forwards, second.forwards)
                self.assertTrue({a.id, b.id}.issubset(r.id for r in first.forwards))
                changed = first.upsert(replace(a, name="Renamed"), expected=a)
                with self.assertRaisesRegex(OSError, "changed in another view"):
                    second.upsert(replace(a, name="Stale edit"), expected=a)
                second.delete(changed)
                first.poll()
                self.assertNotIn(a.id, [r.id for r in first.forwards])
                store.load()
                self.assertEqual(store.forwards, first.forwards)
                endpoint = json.loads((directory / "endpoint.json").read_text())
                with socket.create_connection(("127.0.0.1", endpoint["port"]), timeout=2) as connection:
                    connection.sendall(b'{"protocol":1,"command":"shutdown","token":"wrong"}\n')
                    self.assertFalse(json.loads(connection.recv(4096))["ok"])
                second.close()
                self.assertFalse(exchange(directory, "status")["running"])
                exchange(directory, "shutdown")
                server.wait(timeout=5)
                self.assertEqual(server.returncode, 0)
                self.assertFalse((directory / "endpoint.json").exists())
            finally:
                if server.poll() is None:
                    server.kill()
                server.wait()
                server.stderr.close()

    def test_supervisor_survives_launcher_death_and_can_be_reattached(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            store = Store(directory)
            store.load()
            store.host = "workbox"
            store.save(store.forwards)
            # No SSH connections are started in this test.
            code = """from pathlib import Path
import sys,time
from background import DaemonClient
client = DaemonClient('workbox', Path(sys.argv[1]))
print('ready', flush=True)
time.sleep(60)
"""
            stdout_path = directory / "launcher-output.log"
            with stdout_path.open("wb") as output:
                launcher = subprocess.Popen([sys._base_executable, "-c", code, folder],
                                            stdout=output, stderr=output,
                                            creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                deadline = time.monotonic() + 12
                while time.monotonic() < deadline:
                    if "ready" in stdout_path.read_text(errors="replace"):
                        break
                    if launcher.poll() is not None:
                        break
                    time.sleep(.1)
                startup_output = stdout_path.read_text(errors="replace")
                if (os.environ.get("GITHUB_ACTIONS") == "true"
                        and "Windows prevented background detachment." in startup_output):
                    self.skipTest("Hosted runner job forbids process breakaway; run this test on a desktop Windows session")
                self.assertIn("ready", startup_output)
                first = exchange(directory, "status")
                launcher.kill()
                launcher.wait(timeout=5)
                reopened = DaemonClient("workbox", directory)
                reopened.poll()
                self.assertEqual(exchange(directory, "status")["pid"], first["pid"])
                self.assertFalse(reopened.running)
                # Unauthenticated loopback requests cannot control the daemon.
                endpoint = json.loads((directory / "endpoint.json").read_text())
                with socket.create_connection(("127.0.0.1", endpoint["port"]), timeout=2) as connection:
                    connection.sendall(b'{"protocol":1,"command":"shutdown","token":"wrong"}\n')
                    denied = json.loads(connection.recv(4096))
                self.assertFalse(denied["ok"])
                self.assertEqual(exchange(directory, "status")["pid"], first["pid"])
            finally:
                if launcher.poll() is None:
                    launcher.kill()
                launcher.wait()
                try:
                    exchange(directory, "shutdown")
                except (OSError, ValueError):
                    pass
                deadline = time.monotonic() + 5
                while (directory / "endpoint.json").exists() and time.monotonic() < deadline:
                    time.sleep(.1)
                self.assertFalse((directory / "endpoint.json").exists())


if __name__ == "__main__":
    unittest.main()
