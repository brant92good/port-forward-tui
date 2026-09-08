from concurrent.futures import Future
from dataclasses import replace
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from textual.widgets import DataTable, Input

from port_forward_tui.all_machines_ui import AllMachinesApp
from port_forward_tui.connections import MultiMachineManager
from port_forward_tui.forwarding import Forward, Store
from port_forward_tui.machines import Catalog
from port_forward_tui.views import ViewRegistration


class Client:
    auto_reconnect = True

    def __init__(self, store):
        self.store = store
        self.forwards = list(store.forwards)
        self.states, self.logs = {}, {}

    def upsert(self, rule, expected=None, start=False):
        if expected:
            assert expected in self.forwards
        self.forwards = [rule if r.id == rule.id else r for r in self.forwards]
        if not any(r.id == rule.id for r in self.forwards):
            self.forwards.append(rule)
        self.store.save(self.forwards)
        return rule

    def start(self, rule):
        assert rule in self.forwards
        self.states[rule.id] = 'ON'

    def stop(self, rule_id):
        self.states[rule_id] = 'OFF'

    def delete(self, rule):
        self.forwards.remove(rule)
        self.store.save(self.forwards)
        self.stop(rule.id)

    def close(self):
        for rule in self.forwards:
            self.stop(rule.id)


class AllMachinesTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.catalog = Catalog(Path(self.folder.name))
        self.a = self.catalog.add('server-a', 'Development')
        self.b = self.catalog.add('server-b', 'Lab')
        # Imported copies may share a favorite ID; the UI must still route correctly.
        self.rule = Forward.make(18000, 8000, 'Web app')
        self.clients = {}
        for machine, rule in ((self.a, self.rule), (self.b, replace(self.rule, local_port=28000))):
            store = Store(machine.directory)
            store.load()
            store.save([rule])
            self.clients[machine.directory] = Client(store)
            (machine.directory / 'endpoint.json').write_text('{}')
        def control(directory, command, **arguments):
            client = self.clients[directory]
            if command == 'stop':
                client.stop(arguments['rule_id'])
            elif command == 'stop_all':
                client.close()
            return {'states': dict(client.states), 'details': dict(client.logs)}
        self.manager = MultiMachineManager(self.catalog, self.a, lambda host, directory: self.clients[directory], control)
        self.addCleanup(self.manager.detach)
        # Keyboard tests control cached snapshots. Separate tests exercise asynchronous reads.
        self.manager.next_poll = self.manager.next_catalog = float('inf')

    async def test_grouped_list_starts_both_servers_and_quick_entry_uses_selection(self):
        app = AllMachinesApp(self.manager, self.catalog)
        async with app.run_test(size=(110, 28)) as pilot:
            table = app.query_one(DataTable)
            self.assertEqual(table.row_count, 2)
            await pilot.press('enter', 'down', 'enter')
            self.assertEqual(len(self.manager.running), 2)
            self.assertEqual(self.manager.active_id, self.b.id)
            await pilot.press(*'9000', 'enter')
            self.assertEqual(len(self.clients[self.a.directory].forwards), 1)
            self.assertEqual(len(self.clients[self.b.directory].forwards), 2)
            self.assertEqual(self.clients[self.b.directory].forwards[-1].remote_port, 9000)
            self.assertNotIn(':', self.clients[self.b.directory].forwards[-1].id)
            await pilot.press('q')
            self.assertEqual(len(self.manager.running), 3)

    async def test_edit_and_delete_same_id_only_change_selected_server(self):
        app = AllMachinesApp(self.manager, self.catalog)
        async with app.run_test(size=(110, 30)) as pilot:
            await pilot.press('down', 'e', *'29000', 'ctrl+s')
            self.assertEqual(self.clients[self.b.directory].forwards[0].local_port, 29000)
            self.assertEqual(self.clients[self.a.directory].forwards[0].local_port, 18000)
            await pilot.press('d', 'y')
            self.assertFalse(self.clients[self.b.directory].forwards)
            self.assertEqual(len(self.clients[self.a.directory].forwards), 1)
            # The empty machine still has a selectable row for adding a connection.
            self.assertEqual(app.query_one(DataTable).row_count, 2)

    async def test_enter_cancels_retry_even_without_a_live_ssh_process(self):
        key = self.rule.id
        self.manager.entries[self.a.id].states[key] = 'RETRYING'
        self.clients[self.a.directory].states[key] = 'RETRYING'
        app = AllMachinesApp(self.manager, self.catalog)
        async with app.run_test(size=(110, 28)) as pilot:
            await pilot.press('enter')
            self.assertEqual(self.manager.status(f'{self.a.id}:{key}'), 'OFF')

    def test_collision_explains_which_server_without_stopping_it(self):
        first, second = self.manager.forwards
        self.manager.start(first)
        second = self.manager.upsert(replace(second, local_port=first.local_port), expected=second)
        self.manager.start(second)
        self.assertEqual(self.manager.status(first.id), 'ON')
        self.assertEqual(self.manager.status(second.id), 'ERROR')
        self.assertIn('Development', self.manager.details(second.id))

    async def test_stop_all_includes_every_server_and_pending_retries(self):
        for machine in (self.a, self.b):
            (machine.directory / 'endpoint.json').write_text('{}')
        for rule in self.manager.forwards:
            self.manager.start(rule)
        self.clients[self.b.directory].states[self.rule.id] = 'RETRYING'
        self.manager.entries[self.b.id].states[self.rule.id] = 'RETRYING'
        app = AllMachinesApp(self.manager, self.catalog)
        async with app.run_test(size=(80, 24)) as pilot:
            await pilot.press('s')
            self.assertTrue(all(self.manager.status(r.id) == 'OFF' for r in self.manager.forwards))


    def test_failed_status_read_does_not_block_other_servers_or_undo_a_new_edit(self):
        self.manager.pending[self.a.id] = (Future(), 0)
        good = Future()
        store = self.clients[self.b.directory].store
        good.set_result((store, None))
        self.manager.pending[self.b.id] = (good, 0)
        self.manager.poll()
        self.assertNotIn(self.b.id, self.manager.pending)
        self.assertFalse(self.manager.pending[self.a.id][0].done())
        stale = self.manager.pending[self.a.id][0]
        rule = self.manager.forwards[0]
        self.manager.upsert(replace(rule, name='Latest edit'), expected=rule)
        stale.set_exception(OSError('old failed request'))
        self.manager.poll()
        self.assertEqual(self.manager.forwards[0].name, 'Latest edit')
        self.assertEqual(self.manager.entries[self.a.id].error, '')

    def test_selected_machine_registration_moves_without_window_lookup(self):
        with patch('port_forward_tui.views.sys.stdout.isatty', return_value=False):
            registration = ViewRegistration(self.a.directory, self.a.target, catalog_root=self.catalog.root, machine=self.a.id)
            old = registration.path
            try:
                with patch('port_forward_tui.window_context.window_for_title', side_effect=AssertionError('slow lookup')):
                    registration.select_machine(self.b)
                self.assertFalse(old.exists())
                self.assertTrue(registration.path.exists())
                self.assertEqual(registration.data['machine'], self.b.id)
            finally:
                registration.close()

    def test_stop_controls_survive_a_damaged_saved_machine_file(self):
        for rule in self.manager.forwards:
            self.manager.start(rule)
        (self.a.directory / 'forwards.json').write_text('{broken')
        self.manager.next_catalog = 0
        self.manager.poll()
        self.assertTrue(self.manager.catalog_error)
        self.manager.close()
        self.assertTrue(all(self.manager.status(r.id) == 'OFF' for r in self.manager.forwards))
