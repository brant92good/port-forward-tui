from pathlib import Path
import tempfile
import unittest

from textual.widgets import Input
from port_forward_tui.ui import EditForward, PortApp
from port_forward_tui.forwarding import Store
from tests.test_app import FakeManager


class AddFormTests(unittest.IsolatedAsyncioTestCase):
    async def test_quick_entry_add_and_edit_are_distinct_keyboard_actions(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            app = PortApp(store, FakeManager())
            async with app.run_test(size=(104, 34)) as pilot:
                await pilot.press('n')
                self.assertEqual(app.focused.id, 'quick')
                self.assertNotIsInstance(app.screen, EditForward)
                await pilot.press('escape', 'a')
                self.assertIsInstance(app.screen, EditForward)
                self.assertTrue(app.screen.create)
                self.assertEqual(app.focused.id, 'remote')
                self.assertEqual(app.screen.query_one('#remote', Input).value, '')
                await pilot.press('escape', 'e')
                self.assertIsInstance(app.screen, EditForward)
                self.assertFalse(app.screen.create)
                self.assertEqual(app.focused.id, 'local')
                self.assertEqual(app.screen.query_one('#remote', Input).value,
                                 str(store.forwards[0].remote_port))

    async def test_remote_first_form_defaults_local_and_reuses_an_existing_favorite(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.save([])
            manager = FakeManager()
            app = PortApp(store, manager)
            async with app.run_test(size=(80, 26)) as pilot:
                await pilot.press('a')
                self.assertEqual(app.focused.id, 'remote')
                await pilot.press('8', '0', '0', '0', 'enter')
                self.assertEqual(len(store.forwards), 1)
                first = store.forwards[0]
                self.assertEqual((first.local_port, first.remote_port), (8000, 8000))
                self.assertEqual(manager.status(first.id), 'ON')
                await pilot.press('a', '8', '0', '0', '0', 'enter')
                self.assertEqual(len(store.forwards), 1)

    async def test_form_custom_local_port_and_cancel_do_not_change_other_favorites(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.save([])
            app = PortApp(store, FakeManager())
            async with app.run_test() as pilot:
                await pilot.press('a', '8', '0', '0', '0', 'tab', '1', '8', '0', '0', '0', 'enter')
                self.assertEqual((store.forwards[0].local_port, store.forwards[0].remote_port), (18000, 8000))
                await pilot.press('a', 'escape')
                self.assertEqual(len(store.forwards), 1)
