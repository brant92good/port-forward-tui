from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from port_forward_tui.forwarding import Forward, Store, ssh_command
from port_forward_tui.machines import Catalog, import_ssh, ssh_aliases
from port_forward_tui.window_context import machine_for_window, process_started, choose_machine
from tests.test_ports import command, ROOT


class MachinesTests(unittest.TestCase):
    def test_legacy_favorites_and_endpoint_are_never_moved_or_rewritten(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            store = Store(root)
            store.host = 'old-machine'
            store.save([Forward.make(8766, 8766, 'My only favorite')])
            before = store.path.read_bytes()
            endpoint = root / 'endpoint.json'
            endpoint.write_text('a live endpoint must stay here')
            catalog = Catalog(root)
            old = catalog.add('old-machine')
            other = catalog.add('alex@other-machine', 'Other machine', 2222)
            self.assertEqual(old.directory, root)
            self.assertNotEqual(other.directory, root)
            self.assertEqual(store.path.read_bytes(), before)
            self.assertEqual(endpoint.read_text(), 'a live endpoint must stay here')
            self.assertEqual(len(catalog.list()), 2)

    def test_simultaneous_addition_preserves_favorites_and_duplicate_identity(self):
        with tempfile.TemporaryDirectory() as folder:
            catalog = Catalog(Path(folder))
            with ThreadPoolExecutor(max_workers=2) as pool:
                a, b = list(pool.map(catalog.add, ['workbox', 'workbox']))
            self.assertEqual(a, b)
            store = Store(a.directory)
            store.load()
            store.save([])
            catalog.add('workbox')
            store.load()
            self.assertEqual(store.forwards, [])

    def test_import_reads_literal_names_and_includes_without_executing_match_commands(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            config = root / 'config'
            included = root / 'included config'
            included.write_text('Host = two\nInclude "' + str(config) + '"\n')
            contents = 'Host one *.example !excluded\nMatch exec "never-run-this"\nInclude "' + str(included) + '"\nHost three # comment\n'
            config.write_text(contents)
            with patch('subprocess.run') as process:
                self.assertEqual(ssh_aliases(config), ['one', 'three', 'two'])
                imported = import_ssh(Catalog(root / 'app-data'), config, ['two'])
                process.assert_not_called()
            self.assertEqual([m.target for m in imported], ['two'])
            self.assertEqual(imported[0].ssh_config, str(config.resolve()))
            self.assertEqual(config.read_text(), contents)

    def test_manual_login_port_and_import_config_reach_ssh_as_arguments(self):
        rule = Forward.make(18000, 8000)
        args = ssh_command('alex@server', rule, 2222, r'C:\my configs\ssh.conf')
        self.assertEqual(args[args.index('-p') + 1], '2222')
        self.assertEqual(args[args.index('-F') + 1], r'C:\my configs\ssh.conf')
        self.assertEqual(args[-1], 'alex@server')
        self.assertNotIn('-p', ssh_command('workbox', rule))

    def test_install_check_without_host_does_not_create_data(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / 'absent'
            result = subprocess.run([sys.executable, str(ROOT / 'app.py'), '--data-dir', str(root), '--check'],
                                    capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(root.exists())

    def test_commands_require_machine_for_ambiguous_writes_and_do_not_start_ssh(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for target in ('one', 'two'):
                code, result = command(root, 'machines', 'add', target)
                self.assertEqual(code, 0, result)
            code, result = command(root, 'save', '--remote', '8000')
            self.assertEqual(code, 2)
            self.assertIn('--machine', result['error']['message'])
            self.assertFalse(list(root.rglob('endpoint.json')))
            code, result = command(root, 'list')
            self.assertEqual(code, 0)
            self.assertEqual(len(result['machines']), 2)
            self.assertTrue(all(r['state'] == 'UNKNOWN' for r in result['forwards']))

    def test_context_follows_last_focused_machine_in_the_origin_window_and_ignores_dead_views(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            views = root / 'window-views'
            views.mkdir()
            started = process_started(os.getpid())
            records = [dict(machine='one', window=100, last_focus=10),
                       dict(machine='two', window=100, last_focus=20),
                       dict(machine='elsewhere', window=200, last_focus=30),
                       dict(machine='stale', window=100, last_focus=40, started=1)]
            for index, record in enumerate(records):
                record = dict(pid=os.getpid(), started=started, **{k: v for k, v in record.items() if k != 'started'}) if 'started' not in record else dict(pid=os.getpid(), **record)
                (views / f'{index}.json').write_text(json.dumps(record))
            self.assertEqual(machine_for_window(root, 100), 'two')
            self.assertEqual(machine_for_window(root, 200), 'elsewhere')
            self.assertIsNone(machine_for_window(root, 300))

    def test_new_window_with_multiple_machines_uses_picker_and_explicit_machine_skips_it(self):
        with tempfile.TemporaryDirectory() as folder:
            catalog = Catalog(Path(folder))
            a, b = catalog.add('one'), catalog.add('two')
            with patch('port_forward_tui.machine_ui.pick_machine', return_value=b.id) as picker:
                self.assertEqual(choose_machine(catalog, use_window=False), b)
                self.assertEqual(choose_machine(catalog, a.id), a)
                self.assertEqual(picker.call_count, 1)


class SeparateControllersTests(unittest.TestCase):
    def test_two_machines_have_separate_servers_and_favorite_ids(self):
        from port_forward_tui.background import exchange
        with tempfile.TemporaryDirectory() as folder:
            catalog = Catalog(Path(folder))
            machines = [catalog.add('one'), catalog.add('two', ssh_port=2222)]
            servers = []
            try:
                for machine in machines:
                    server = subprocess.Popen([sys._base_executable, '-E', '-s', str(ROOT / 'port_forward_tui/background.py'),
                                               '--serve', '--data-dir', str(machine.directory)],
                                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
                    servers.append(server)
                    deadline = time.monotonic() + 8
                    while not (machine.directory / 'endpoint.json').exists() and time.monotonic() < deadline:
                        self.assertIsNone(server.poll())
                        time.sleep(.05)
                ids = []
                for machine in machines:
                    code, result = command(catalog.root, 'save', '--machine', machine.id, '--remote', '19000')
                    self.assertEqual(code, 0, result)
                    ids.append(result['id'])
                self.assertNotEqual(*ids)
                code, _ = command(catalog.root, 'delete', ids[0], '--machine', machines[0].id, '--yes')
                self.assertEqual(code, 0)
                second = exchange(machines[1].directory, 'status')
                self.assertIn(ids[1], [r['id'] for r in second['forwards']])
                self.assertEqual(second['ssh_port'], 2222)
            finally:
                for machine, server in zip(machines, servers):
                    if server.poll() is None:
                        try:
                            exchange(machine.directory, 'shutdown')
                            server.wait(timeout=8)
                        except (OSError, ValueError, subprocess.TimeoutExpired):
                            server.kill()
                            server.wait(timeout=5)
