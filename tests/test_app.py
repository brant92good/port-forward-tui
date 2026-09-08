import asyncio
import ctypes
from ctypes import wintypes
from dataclasses import replace
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from textual.widgets import DataTable, Input, Static

from port_forward_tui.ui import Confirm, EditForward, PortApp
from port_forward_tui.forwarding import Forward, InstanceLock, ProcessJob, Store, TunnelManager, listeners, quick_ports, ssh_command


class FakeManager:
    def __init__(self):
        self.running = {}
        self.states = {}
        self.starts = []
        self.stops = []

    def status(self, key):
        return self.states.get(key, "OFF")

    def start(self, rule):
        if rule.id not in self.running:
            self.running[rule.id] = rule
            self.states[rule.id] = "ON"
            self.starts.append(rule)

    def stop(self, key):
        self.running.pop(key, None)
        self.states[key] = "OFF"
        self.stops.append(key)

    def close(self):
        for key in list(self.running):
            self.stop(key)

    def poll(self):
        pass

    def details(self, key):
        return ""


class SettingsTests(unittest.TestCase):
    def test_quick_entry_and_port_boundaries(self):
        self.assertEqual(quick_ports("8000"), (8000, 8000))
        self.assertEqual(quick_ports("18000:8000"), (18000, 8000))
        self.assertEqual(quick_ports(" 1:65535 "), (1, 65535))
        for invalid in ("", "0", "65536", "-1", "1:2:3", "8000;whoami", "-L", "localhost:80", "1:", "1.2"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                quick_ports(invalid)

    def test_favorites_round_trip_and_empty_list(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            self.assertEqual(len(store.forwards), 6)
            rule = Forward.make(18000, 8000, "測試 API")
            store.save([rule])
            reopened = Store(Path(folder))
            reopened.load()
            self.assertEqual(reopened.forwards, [rule])
            reopened.save([])
            store.load()
            self.assertEqual(store.forwards, [])

    def test_corrupt_settings_are_not_overwritten(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.path.write_text('{broken', encoding="utf-8")
            with self.assertRaises(ValueError):
                store.load()
            self.assertEqual(store.path.read_text(), '{broken')

    def test_failed_save_preserves_disk_and_memory(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            before = store.path.read_bytes()
            rules = list(store.forwards)
            with patch("port_forward_tui.forwarding.os.replace", side_effect=OSError("disk error")):
                with self.assertRaises(OSError):
                    store.save([])
            self.assertEqual(store.path.read_bytes(), before)
            self.assertEqual(store.forwards, rules)
            self.assertFalse(list(Path(folder).glob("*.tmp")))

    def test_ssh_forward_direction_and_no_remote_command(self):
        args = ssh_command("workbox", Forward.make(18000, 8000, "$(bad)"))
        self.assertEqual(args[-1], "workbox")
        self.assertEqual(args[args.index("-L") + 1], "127.0.0.1:18000:127.0.0.1:8000")
        self.assertIn("StrictHostKeyChecking=yes", args)
        self.assertIn("ExitOnForwardFailure=yes", args)
        self.assertIn("-N", args)
        self.assertNotIn("$(bad)", args)

    def test_single_instance_lock_releases(self):
        with tempfile.TemporaryDirectory() as folder:
            lock = InstanceLock(Path(folder))
            try:
                with self.assertRaises(RuntimeError):
                    InstanceLock(Path(folder))
            finally:
                lock.close()
            InstanceLock(Path(folder)).close()


class ProcessTests(unittest.TestCase):
    def test_abrupt_parent_exit_removes_tunnel_child(self):
        code = """import subprocess,sys,time
from port_forward_tui.forwarding import ProcessJob
job = ProcessJob()
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'], creationflags=subprocess.CREATE_NO_WINDOW)
job.attach(child)
print(child.pid, flush=True)
time.sleep(60)
"""
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        holder = subprocess.Popen([sys._base_executable, "-c", code], stdout=subprocess.PIPE,
                                  encoding="ascii", creationflags=subprocess.CREATE_NO_WINDOW)
        handle = None
        try:
            child_pid = int(holder.stdout.readline().strip())
            handle = kernel.OpenProcess(0x100001, False, child_pid)
            self.assertTrue(handle)
            holder.kill()
            holder.wait(timeout=5)
            self.assertEqual(kernel.WaitForSingleObject(handle, 5000), 0,
                             "Child survived abrupt parent exit")
        finally:
            if holder.poll() is None:
                holder.kill()
            holder.wait()
            holder.stdout.close()
            if handle:
                kernel.TerminateProcess(handle, 1)
                kernel.CloseHandle(handle)

    def test_job_close_terminates_owned_child(self):
        job = ProcessJob()
        process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            job.attach(process)
            self.assertIsNone(process.poll())
            job.close()
            process.wait(timeout=5)
            self.assertIsNotNone(process.returncode)
        finally:
            job.close()
            if process.poll() is None:
                process.kill()
            process.wait()

    def test_bind_conflict_keeps_existing_listener_alive(self):
        with tempfile.TemporaryDirectory() as folder, socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            number = listener.getsockname()[1]
            manager = TunnelManager("unused", Path(folder))
            rule = Forward.make(number, 8000)
            manager.start(rule)
            self.assertEqual(manager.status(rule.id), "ERROR")
            self.assertIn("already in use", manager.details(rule.id))
            self.assertFalse(manager.running)
            self.assertIn((__import__('os').getpid(), number), listeners())

    def test_listener_ownership_status_stop_and_disconnect(self):
        with tempfile.TemporaryDirectory() as folder, socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            number = reservation.getsockname()[1]
            reservation.close()
            rule = Forward.make(number, 8000)
            manager = TunnelManager("unused", Path(folder))
            command = [sys._base_executable, "-c", f"import socket,time; s=socket.socket(); s.bind(('127.0.0.1',{number})); s.listen(); time.sleep(60)"]
            try:
                with patch("port_forward_tui.forwarding.ssh_command", return_value=command):
                    manager.start(rule)
                    deadline = time.monotonic() + 5
                    while manager.status(rule.id) != "ON" and time.monotonic() < deadline:
                        time.sleep(.05)
                        manager.poll()
                    self.assertEqual(manager.status(rule.id), "ON", manager.details(rule.id))
                    child = manager.running[rule.id].process
                    manager.stop(rule.id)
                    self.assertIsNotNone(child.poll())
                    self.assertEqual(manager.status(rule.id), "OFF")
                    self.assertNotIn((child.pid, number), listeners())
                    manager.start(rule)
                    child = manager.running[rule.id].process
                    child.kill()
                    child.wait()
                    manager.poll()
                    self.assertEqual(manager.status(rule.id), "RETRYING")
                    self.assertFalse(manager.running)
                    self.assertIn(rule.id, manager.wanted)
            finally:
                manager.close()


class KeyboardTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.folder.name))
        self.store.load()
        self.manager = FakeManager()
        self.app = PortApp(self.store, self.manager)

    async def asyncTearDown(self):
        self.folder.cleanup()

    async def test_type_port_enter_save_start_toggle(self):
        async with self.app.run_test(size=(110, 32)) as pilot:
            await pilot.press("9", "0", "0", "0", "enter")
            await pilot.pause()
            rule = self.store.forwards[-1]
            self.assertEqual((rule.local_port, rule.remote_port), (9000, 9000))
            self.assertIn(rule.id, self.manager.running)
            self.assertIsInstance(self.app.focused, DataTable)
            await pilot.press("enter")
            self.assertNotIn(rule.id, self.manager.running)
            await pilot.press("space")
            self.assertIn(rule.id, self.manager.running)
            reopened = Store(Path(self.folder.name))
            reopened.load()
            self.assertEqual(reopened.forwards[-1], rule)

    async def test_custom_port_and_duplicate_quick_entry(self):
        async with self.app.run_test(size=(100, 30)) as pilot:
            await pilot.press(*list("18000:8000"), "enter")
            await pilot.pause()
            rule = self.store.forwards[-1]
            self.assertEqual((rule.local_port, rule.remote_port), (18000, 8000))
            await pilot.press(*list("18000:8000"), "enter")
            self.assertEqual(len(self.store.forwards), 7)
            self.assertEqual(len(self.manager.starts), 1)

    async def test_edit_active_forward_with_keyboard_and_blank_local_default(self):
        async with self.app.run_test(size=(100, 36)) as pilot:
            await pilot.press("enter", "e")
            self.assertIsInstance(self.app.screen, EditForward)
            self.assertEqual(self.app.focused.id, "local")
            await pilot.press(*list("13000"), "enter")
            await pilot.pause()
            self.assertEqual(self.store.forwards[0].local_port, 13000)
            self.assertEqual(self.manager.starts[-1].local_port, 13000)
            await pilot.press("e", "backspace", "enter")
            await pilot.pause()
            self.assertEqual(self.store.forwards[0].local_port, self.store.forwards[0].remote_port)

    async def test_invalid_entry_does_not_save_or_start(self):
        async with self.app.run_test() as pilot:
            await pilot.press(*list("99999"), "enter")
            self.assertEqual(len(self.store.forwards), 6)
            self.assertFalse(self.manager.running)
            self.assertIsInstance(self.app.focused, Input)
            await pilot.press("escape")
            self.assertIsInstance(self.app.focused, DataTable)

    async def test_immediate_edit_typing_keeps_every_digit_and_the_name(self):
        original = self.store.forwards[0]
        async with self.app.run_test(size=(100, 36)) as pilot:
            for local_port in (19001, 28002, 37003, 46004, 55005):
                # No pause between opening the dialog and typing. A queued
                # focus change used to consume or overwrite the first digit.
                await pilot.press("e", *str(local_port), "enter")
                changed = self.store.forwards[0]
                self.assertEqual(changed.local_port, local_port)
                self.assertEqual(changed.name, original.name)
                self.assertEqual(changed.remote_port, original.remote_port)

    async def test_delete_cancel_confirm_and_quit_cleanup(self):
        async with self.app.run_test(size=(100, 32)) as pilot:
            await pilot.press("enter", "d")
            self.assertIsInstance(self.app.screen, Confirm)
            await pilot.press("escape")
            self.assertEqual(len(self.store.forwards), 6)
            await pilot.press("d", "y")
            await pilot.pause()
            self.assertEqual(len(self.store.forwards), 5)
            self.assertFalse(self.manager.running)
            await pilot.press("enter", "q")
            self.assertIsInstance(self.app.screen, Confirm)
            await pilot.press("y")
            self.assertFalse(self.manager.running)

    async def test_compact_window_and_help(self):
        async with self.app.run_test(size=(80, 24)) as pilot:
            self.assertGreaterEqual(self.app.query_one(DataTable).size.height, 7)
            await pilot.press("question_mark")
            await pilot.press("escape")
            self.assertIsInstance(self.app.focused, DataTable)


if __name__ == "__main__":
    unittest.main()
