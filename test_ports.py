"""Command contracts tested through real processes and the real control server."""
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest

from background import exchange
from forwarding import Store

ROOT = Path(__file__).resolve().parent


def command(directory, *args):
    result = subprocess.run([sys.executable, '-E', '-s', str(ROOT / 'ports.py'), '--data-dir', str(directory),
                             *args, '--json'], capture_output=True, text=True,
                            creationflags=subprocess.CREATE_NO_WINDOW, timeout=15)
    return result.returncode, json.loads(result.stdout)


class ReadOnlyCommands(unittest.TestCase):
    def test_list_and_doctor_do_not_initialize_an_absent_data_folder(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder) / 'not-created'
            code, result = command(directory, 'list')
            self.assertEqual(code, 0)
            self.assertEqual(result['forwards'], [])
            code, result = command(directory, 'doctor')
            self.assertEqual(code, 1)
            self.assertFalse(result['ok'])
            self.assertFalse(directory.exists())

    def test_json_argument_error_and_corrupt_file_are_explicit_and_preserved(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            code, result = command(directory, 'save', '--remote', '99999')
            self.assertEqual(code, 2)
            self.assertEqual(result['error']['code'], 'invalid_arguments')
            saved = directory / 'forwards.json'
            saved.write_text('not JSON')
            code, result = command(directory, 'list')
            self.assertEqual(code, 1)
            self.assertFalse(result['ok'])
            self.assertEqual(saved.read_text(), 'not JSON')
            saved.write_text('[]')
            code, result = command(directory, 'list')
            self.assertEqual(code, 1)
            self.assertFalse(result['ok'])
            self.assertEqual(saved.read_text(), '[]')

    def test_saved_only_status_does_not_claim_tunnels_are_stopped(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.load()
            code, result = command(store.directory, 'list')
            self.assertEqual(code, 0)
            self.assertTrue(all(r['state'] == 'UNKNOWN' for r in result['forwards']))


class SharedCommands(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        store = Store(self.directory)
        store.host = 'workbox'
        store.save([])
        self.server = subprocess.Popen([sys._base_executable, '-E', '-s', str(ROOT / 'background.py'),
                                        '--serve', '--data-dir', str(self.directory)],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                       creationflags=subprocess.CREATE_NO_WINDOW)
        deadline = time.monotonic() + 8
        while not (self.directory / 'endpoint.json').exists() and time.monotonic() < deadline:
            self.assertIsNone(self.server.poll())
            time.sleep(.05)
        self.assertTrue((self.directory / 'endpoint.json').exists())

    def tearDown(self):
        try:
            exchange(self.directory, 'shutdown')
            self.server.wait(timeout=8)
        finally:
            if self.server.poll() is None:
                self.server.kill()
                self.server.wait(timeout=5)
            self.temporary.cleanup()

    def test_concurrent_save_same_mapping_idempotence_and_confirmed_delete(self):
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda remote: command(self.directory, 'save', '--remote', str(remote)), (18080, 18081)))
        self.assertTrue(all(code == 0 for code, _ in results))
        code, saved = command(self.directory, 'save', '--remote', '18080')
        self.assertEqual(code, 0)
        self.assertEqual(len(saved['forwards']), 2)
        self.assertTrue(all(r['state'] == 'OFF' for r in saved['forwards']))
        self.assertTrue(all(r['local_port'] == r['remote_port'] for r in saved['forwards']))
        self.assertNotIn('token', json.dumps(saved))
        code, rejected = command(self.directory, 'delete', saved['id'])
        self.assertEqual(code, 2)
        self.assertEqual(len(exchange(self.directory, 'status')['forwards']), 2)
        code, deleted = command(self.directory, 'delete', saved['id'], '--yes')
        self.assertEqual(code, 0)
        self.assertEqual(len(deleted['forwards']), 1)

    def test_start_reports_a_real_local_port_conflict_as_failure(self):
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            local_port = listener.getsockname()[1]
            code, saved = command(self.directory, 'save', '--remote', '8000', '--local', str(local_port))
            self.assertEqual(code, 0)
            code, result = command(self.directory, 'start', saved['id'])
            self.assertEqual(code, 1)
            self.assertFalse(result['ok'])
            self.assertIn('already in use', result['error']['message'])
            self.assertEqual(listener.getsockname()[1], local_port)

    def test_stop_all_can_reach_the_server_even_with_broken_favorites(self):
        path = self.directory / 'forwards.json'
        path.write_text('broken JSON')
        code, result = command(self.directory, 'stop-all')
        self.assertEqual(code, 0)
        self.assertTrue(result['ok'])
        self.assertEqual(path.read_text(), 'broken JSON')
