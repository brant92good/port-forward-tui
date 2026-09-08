from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from textual.widgets import Input, SelectionList
from port_forward_tui.machine_ui import MachinePicker, AddMachine, ImportMachines
from port_forward_tui.machines import Catalog
from port_forward_tui.ui import PortApp
from port_forward_tui.forwarding import Store
from tests.test_app import FakeManager


class MachineKeyboardTests(unittest.IsolatedAsyncioTestCase):
    async def test_empty_first_launch_adds_a_machine_without_connecting(self):
        with tempfile.TemporaryDirectory() as folder:
            catalog = Catalog(Path(folder))
            app = MachinePicker(catalog)
            async with app.run_test(size=(100, 36)) as pilot:
                await pilot.press('a')
                self.assertIsInstance(app.screen, AddMachine)
                self.assertEqual(app.focused.id, 'target')
                await pilot.press(*list('alex@workbox'), 'enter')
            self.assertEqual(catalog.get(app.return_value).target, 'alex@workbox')
            self.assertFalse(list(Path(folder).rglob('endpoint.json')))

    async def test_keyboard_import_previews_and_saves_only_selected_names(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            config = root / 'config'
            config.write_text('Host one\nHost two\n')
            catalog = Catalog(root / 'data')
            app = MachinePicker(catalog)
            with patch('port_forward_tui.machine_ui.Path.home', return_value=root):
                async with app.run_test(size=(100, 38)) as pilot:
                    await pilot.press('i')
                    self.assertIsInstance(app.screen, ImportMachines)
                    app.screen.query_one('#config-path', Input).value = str(config)
                    app.screen.scan()
                    await pilot.pause()
                    self.assertEqual(app.screen.query_one(SelectionList).selected, [])
                    self.assertEqual(app.focused.id, 'import-hosts')
                    await pilot.press('space')
                    self.assertEqual(app.screen.query_one(SelectionList).selected, ['one'])
                    await pilot.press('ctrl+s')
                    await pilot.pause()
                    self.assertEqual([m.target for m in catalog.list()], ['one'])
                    await pilot.press('escape', 'enter')
            self.assertEqual(catalog.get(app.return_value).target, 'one')

    async def test_switching_machine_detaches_without_stopping_background_connections(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.host = 'one'
            store.load()
            manager = FakeManager()
            manager.persistent = True
            manager.start(store.forwards[0])
            app = PortApp(store, manager)
            async with app.run_test() as pilot:
                await pilot.press('h')
            self.assertEqual(app.return_value, 'pick-machine')
            self.assertTrue(manager.running)
