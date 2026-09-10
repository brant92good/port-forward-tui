"""Windows developer-only protocol/locking migration test; no SSH connections."""
import argparse
from dataclasses import asdict
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from port_forward_tui.background import DaemonClient, exchange
from port_forward_tui.forwarding import Forward, InstanceLock


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    options = parser.parse_args()
    if os.name != 'nt':
        parser.error('The historical controller uses Windows process APIs')
    binary = options.binary.resolve(strict=True)
    flags = subprocess.CREATE_NO_WINDOW
    with tempfile.TemporaryDirectory(prefix='ports-native-compat-') as temporary:
        root = Path(temporary)
        for implementation in ('legacy', 'native'):
            directory = root / implementation
            directory.mkdir()
            rule = Forward.make(18000, 8000, 'Original API')
            data = {'version': 1, 'host': 'loopback-compat', 'keep_alive': True,
                    'forwards': [asdict(rule)], 'machine_name': 'Compatibility'}
            (directory / 'forwards.json').write_text(json.dumps(data), encoding='utf-8')
            command = ([sys.executable, '-E', '-s', str(ROOT/'port_forward_tui/background.py')]
                       if implementation == 'legacy' else [str(binary)])
            with (directory/'controller.log').open('wb') as log:
                controller = subprocess.Popen([*command, '--serve', '--data-dir', str(directory)],
                    stdin=subprocess.DEVNULL, stdout=log, stderr=log, creationflags=flags)
            try:
                deadline = time.monotonic() + 10
                while True:
                    try:
                        initial = exchange(directory, 'status', timeout=.3)
                        break
                    except (OSError, ValueError, KeyError):
                        assert controller.poll() is None, (directory/'controller.log').read_text()
                        assert time.monotonic() < deadline, 'Controller startup timed out'
                        time.sleep(.05)

                def native(*args):
                    result = subprocess.run([str(binary), '--data-dir', str(directory), *args, '--json'],
                        capture_output=True, text=True, encoding='utf-8', timeout=12, creationflags=flags)
                    assert result.returncode == 0, (result.stdout, result.stderr)
                    return json.loads(result.stdout)

                # Native CLI edits the existing mapping through either server.
                result = native('save', '--remote', '8000', '--local', '18000', '--name', 'Renamed API')
                assert result['id'] == rule.id
                assert result['forwards'][0]['name'] == 'Renamed API'
                assert result['forwards'][0]['state'] == 'OFF'
                # New-view preferences live outside the old forwards schema.
                # Toggling and reading them must not start either controller's SSH.
                before_options = (directory/'forwards.json').read_bytes()
                native('auto-open', rule.id, '--on')
                assert native('list')['forwards'][0]['open_automatically'] is True
                assert exchange(directory, 'status')['states'].get(rule.id, 'OFF') == 'OFF'
                assert (directory/'forwards.json').read_bytes() == before_options
                # The old client must attach to the native controller and use its
                # shared-favorites protocol without launching a second server.
                client = DaemonClient(data['host'], directory)
                assert client.shared_favorites and client.auto_reconnect
                added = Forward.make(18888, 8888, 'Notebook')
                client.upsert(added)
                assert native('list')['forwards'][0]['open_automatically'] is True
                assert any(row['id'] == added.id for row in native('list')['forwards'])
                try:
                    client.upsert(rule, expected=rule)
                except OSError as error:
                    assert 'changed' in str(error).lower(), str(error)
                else:
                    raise AssertionError('Old client overwrote a newer favorite')
                native('delete', added.id, '--yes')
                native('auto-open', rule.id, '--off')
                assert native('list')['forwards'][0]['open_automatically'] is False
                assert len(exchange(directory, 'status')['forwards']) == 1
                assert exchange(directory, 'stop_all')['states'].get(rule.id, 'OFF') == 'OFF'
                # The historical Windows byte lock and fs2 lock exclude each other.
                try:
                    lock = InstanceLock(directory, 'daemon.lock')
                except (OSError, RuntimeError):
                    pass
                else:
                    lock.close()
                    raise AssertionError('Legacy lock did not exclude running controller')
                loser = subprocess.run([str(binary), '--serve', '--data-dir', str(directory)],
                    capture_output=True, text=True, encoding='utf-8', timeout=5, creationflags=flags)
                assert loser.returncode != 0, 'Second native controller acquired the same lock'
                assert exchange(directory, 'status')['pid'] == initial['pid']
                saved = json.loads((directory/'forwards.json').read_text(encoding='utf-8'))
                assert saved['keep_alive'] is True and saved['forwards'][0]['id'] == rule.id
                print(f'PASS {implementation} controller: native and legacy clients, CAS, IDs, OFF state, shared lock')
            except BaseException:
                # Fixture-only diagnostics: never dump endpoint authentication.
                print(f'FAIL {implementation}: launcher exit={controller.poll()}', file=sys.stderr)
                print((directory/'controller.log').read_text(encoding='utf-8', errors='replace')[-8192:],
                      file=sys.stderr)
                raise
            finally:
                try:
                    exchange(directory, 'shutdown', timeout=2)
                    controller.wait(timeout=5)
                except (OSError, ValueError, KeyError, subprocess.TimeoutExpired):
                    controller.kill()
                    controller.wait(timeout=5)


if __name__ == '__main__':
    main()
