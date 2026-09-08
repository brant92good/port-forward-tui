from dataclasses import asdict, replace
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from textual.widgets import DataTable

from port_forward_tui.ui import PortApp, main
from port_forward_tui.background import DaemonClient, Supervisor
from port_forward_tui.forwarding import Forward, InstanceLock, Store
from tests.test_app import FakeManager
from port_forward_tui.views import ViewRegistration, focus_existing, live_titles


class SharedClient(DaemonClient):
    """Same client API and protocol, with an in-process test transport."""
    def __init__(self, supervisor):
        self.supervisor = supervisor
        self.host = supervisor.store.host
        self.directory = supervisor.store.directory
        self._apply(supervisor.snapshot())

    def _call(self, command, **arguments):
        try:
            result = self.supervisor.dispatch({"token": self.supervisor.token, "protocol": 1,
                                               "command": command, **arguments})
        except ValueError as error:
            raise OSError(str(error)) from error
        self._apply(result)
        return result


def session(folder):
    store = Store(Path(folder))
    store.load()
    store.host = "workbox"
    store.save(store.forwards)
    manager = FakeManager()
    manager.host = store.host
    return store, Supervisor(store, manager)


class SharedStateTests(unittest.TestCase):
    def test_simultaneous_stale_clients_preserve_both_additions(self):
        with tempfile.TemporaryDirectory() as folder:
            store, supervisor = session(folder)
            first, second = SharedClient(supervisor), SharedClient(supervisor)
            a = first.upsert(Forward.make(9000, 9000, "A"), start=True)
            b = second.upsert(Forward.make(9001, 9001, "B"), start=True)
            first.poll()
            second.poll()
            self.assertEqual(first.forwards, second.forwards)
            self.assertTrue({a.id, b.id}.issubset(first.running))
            self.assertEqual(len(first.forwards), 8)
            duplicate = second.upsert(Forward.make(9000, 9000, "Duplicate"), start=True)
            self.assertEqual(duplicate.id, a.id)
            self.assertEqual(len(second.forwards), 8)
            self.assertEqual(len(supervisor.manager.starts), 2)

    def test_conflicting_edits_do_not_overwrite_or_resurrect(self):
        with tempfile.TemporaryDirectory() as folder:
            store, supervisor = session(folder)
            first, second = SharedClient(supervisor), SharedClient(supervisor)
            original = first.forwards[0]
            updated = first.upsert(replace(original, local_port=13000), expected=original)
            with self.assertRaises(OSError):
                second.upsert(replace(original, name="stale edit"), expected=original)
            with self.assertRaises(OSError):
                second.delete(original)
            first.delete(updated)
            with self.assertRaises(OSError):
                second.upsert(original, expected=original)
            store.load()
            self.assertNotIn(original.id, [r.id for r in store.forwards])

    def test_two_ui_launches_ignore_legacy_ui_lock(self):
        with tempfile.TemporaryDirectory() as folder:
            store, supervisor = session(folder)
            old_ui_lock = InstanceLock(Path(folder))
            try:
                with patch("sys.argv", ["app.py", "--data-dir", folder]), \
                        patch("port_forward_tui.background.DaemonClient", side_effect=lambda *args: SharedClient(supervisor)), \
                        patch("port_forward_tui.ui.PortApp.run") as run:
                    self.assertEqual(main(), 0)
                    self.assertEqual(main(), 0)
                    self.assertEqual(run.call_count, 2)
            finally:
                old_ui_lock.close()


class SharedKeyboardTests(unittest.IsolatedAsyncioTestCase):
    async def test_late_refresh_after_view_closes_does_not_touch_ui_or_daemon(self):
        with tempfile.TemporaryDirectory() as folder:
            store, supervisor = session(folder)
            client = SharedClient(supervisor)
            application = PortApp(store, client)
            async with application.run_test() as pilot:
                await pilot.press("enter", "q")
            with patch.object(client, "poll") as poll:
                application.tick()
                poll.assert_not_called()
            self.assertTrue(supervisor.manager.running)

    async def test_two_open_views_sync_add_edit_stop_delete_and_detach(self):
        with tempfile.TemporaryDirectory() as folder:
            _, supervisor = session(folder)
            store = Store(Path(folder))
            store.load()
            second_store = Store(Path(folder))
            second_store.load()
            first, second = SharedClient(supervisor), SharedClient(supervisor)
            app1, app2 = PortApp(store, first), PortApp(second_store, second)
            async with app1.run_test(size=(100, 32)) as pilot1, app2.run_test(size=(100, 32)) as pilot2:
                await pilot1.press(*list("19000:9000"), "enter")
                app2.tick()
                await pilot2.pause()
                rule = next(r for r in second_store.forwards if r.local_port == 19000)
                self.assertEqual(second.status(rule.id), "ON")
                app2.populate(rule.id)
                await pilot2.press("enter")
                app1.tick()
                self.assertEqual(first.status(rule.id), "OFF")
                await pilot2.press("e", *list("19001"), "enter")
                app1.tick()
                self.assertEqual(next(r for r in store.forwards if r.id == rule.id).local_port, 19001)
                self.assertEqual(app1.query_one(DataTable).get_row(rule.id)[2], "19001")
                await pilot2.press("enter", "q")
                self.assertIn(rule.id, supervisor.manager.running)
                await pilot1.press("d", "y")
                self.assertNotIn(rule.id, supervisor.manager.running)
                self.assertNotIn(rule.id, [r.id for r in supervisor.store.forwards])


class FocusRegistryTests(unittest.TestCase):
    def test_title_works_before_terminal_vt_mode_is_enabled(self):
        with tempfile.TemporaryDirectory() as folder:
            with patch("port_forward_tui.views.sys.stdout.isatty", return_value=True), patch("port_forward_tui.views.ctypes.WinDLL") as kernel:
                registration = ViewRegistration(Path(folder), "workbox")
                kernel.return_value.SetConsoleTitleW.assert_called_once_with(registration.title)
            registration.close()

    def test_live_registration_disappears_on_close(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            registration = ViewRegistration(directory, "workbox")
            self.assertEqual(live_titles(directory), [registration.title])
            registration.close()
            self.assertEqual(live_titles(directory), [])

    def test_dead_views_are_excluded_and_focus_is_explicit(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            with patch("port_forward_tui.views.subprocess.run") as run:
                self.assertFalse(focus_existing(directory))
                run.assert_not_called()
            registration = ViewRegistration(directory, "workbox")
            with patch("port_forward_tui.views.subprocess.run") as run:
                run.return_value.returncode = 0
                self.assertTrue(focus_existing(directory, probe_only=True))
                self.assertIn("-ProbeOnly", run.call_args.args[0])
            with patch("port_forward_tui.views.process_alive", return_value=False):
                self.assertEqual(live_titles(directory), [])
            self.assertFalse(registration.path.exists())


if __name__ == "__main__":
    unittest.main()
