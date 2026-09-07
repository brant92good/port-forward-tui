import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from app import PortApp
from focus_settings import read_scope, save_scope
from forwarding import Store
from test_app import FakeManager
from views import ViewRegistration, focus_existing


class PreferenceTests(unittest.TestCase):
    def test_focus_handoff_uses_the_candidate_selected_before_launcher_closes(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            registration = ViewRegistration(directory, "workbox")
            try:
                with patch("views.subprocess.run") as probe, patch("views.delayed_focus", return_value=True) as launch:
                    probe.return_value.returncode = 0
                    probe.return_value.stdout = json.dumps({"title": registration.title, "window": 100, "origin": 200})
                    self.assertTrue(focus_existing(directory))
                    args = launch.call_args.args[0]
                    self.assertIn("-AfterPid", args)
                    self.assertEqual(args[args.index("-WindowHandle") + 1], "100")
                    self.assertEqual(args[args.index("-InvokeWindow") + 1], "200")
            finally:
                registration.close()

    def test_default_round_trip_and_invalid_settings(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            self.assertEqual(read_scope(directory), "all")
            save_scope(directory, "window")
            self.assertEqual(read_scope(directory), "window")
            path = directory / "ui-settings.json"
            path.write_text("invalid")
            with self.assertRaises(ValueError):
                save_scope(directory, "all")
            self.assertEqual(path.read_text(), "invalid")

    def test_current_window_uses_launcher_identity_and_never_falls_back_globally(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            registration = ViewRegistration(directory, "workbox")
            try:
                save_scope(directory, "window")
                with patch("views.mark_origin", return_value="unique-launcher"), patch("views.subprocess.run") as run:
                    run.return_value.returncode = 1
                    self.assertFalse(focus_existing(directory))
                    args = run.call_args.args[0]
                    self.assertEqual(args[args.index("-OriginTitle") + 1], "unique-launcher")
                    self.assertEqual(run.call_count, 1)
                with patch("views.mark_origin", side_effect=OSError("No console")), patch("views.subprocess.run") as run:
                    self.assertFalse(focus_existing(directory))
                    run.assert_not_called()
            finally:
                registration.close()


class PreferenceKeyboardTests(unittest.IsolatedAsyncioTestCase):
    async def test_settings_save_cancel_and_reopen_preserve_favorites(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            before = store.path.read_bytes()
            app = PortApp(store, FakeManager())
            async with app.run_test(size=(100, 32)) as pilot:
                await pilot.press("f2", "down", "enter")
                self.assertEqual(read_scope(store.directory), "window")
                await pilot.press("f2", "up", "escape")
                self.assertEqual(read_scope(store.directory), "window")
                await pilot.press("f2", "up", "enter")
                self.assertEqual(read_scope(store.directory), "all")
            self.assertEqual(store.path.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
