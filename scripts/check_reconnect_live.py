"""Opt-in: two SSH forwards, isolated transport loss, recovery, and stop while offline."""
import argparse
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from port_forward_tui.background import DaemonClient, exchange, restart_daemon
from port_forward_tui.forwarding import Forward, SSH, Store, validate_host
from port_forward_tui.machines import Catalog
from port_forward_tui.connections import MultiMachineManager


class TransportGate:
    """A local TCP relay; disabling it affects only this test's SSH transport."""
    def __init__(self, target):
        self.target = target
        self.enabled = True
        self.closed = threading.Event()
        self.lock = threading.Lock()
        self.connections = set()
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen()
        self.listener.settimeout(.2)
        self.port = self.listener.getsockname()[1]
        self.thread = threading.Thread(target=self.accept, daemon=True)
        self.thread.start()

    def accept(self):
        while not self.closed.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            threading.Thread(target=self.relay, args=(client,), daemon=True).start()

    def relay(self, client):
        remote = None
        try:
            if not self.enabled:
                return
            remote = socket.create_connection(self.target, timeout=5)
            with self.lock:
                if not self.enabled:
                    return
                self.connections.update((client, remote))
            while self.enabled and not self.closed.is_set():
                ready, _, _ = select.select([client, remote], [], [], .2)
                for source in ready:
                    data = source.recv(65536)
                    if not data:
                        return
                    (remote if source is client else client).sendall(data)
        except (OSError, ValueError):
            pass
        finally:
            with self.lock:
                self.connections.discard(client)
                self.connections.discard(remote)
            client.close()
            if remote:
                remote.close()

    def offline(self):
        self.enabled = False
        with self.lock:
            for connection in list(self.connections):
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                connection.close()

    def close(self):
        self.closed.set()
        self.offline()
        self.listener.close()
        self.thread.join(timeout=2)


def wait_for(client, rule, expected, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        client.poll()
        if client.status(rule.id) == expected:
            return
        if client.status(rule.id) == 'ERROR':
            raise AssertionError(client.details(rule.id))
        time.sleep(.1)
    raise AssertionError(f'Did not reach {expected}: {client.status(rule.id)}; {client.details(rule.id)}')


def traffic(rule, remote_port):
    with socket.create_connection(('127.0.0.1', rule.local_port), timeout=5) as connection:
        connection.settimeout(5)
        if not connection.recv(256).startswith(b'SSH-2.0-'):
            raise AssertionError(f'Expected SSH banner from remote loopback port {remote_port}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', required=True, help='Trusted existing direct SSH alias with a working key login')
    parser.add_argument('--remote-port', type=int, default=22, help='Remote loopback SSH service for the banner check')
    options = parser.parse_args()
    validate_host(options.host)
    raw = subprocess.check_output([SSH, '-G', options.host], text=True, creationflags=subprocess.CREATE_NO_WINDOW, timeout=15)
    config = dict(line.split(' ', 1) for line in raw.splitlines() if ' ' in line)
    if config.get('proxyjump', 'none') != 'none' or config.get('proxycommand', 'none') != 'none':
        parser.error('This transport test requires a direct SSH alias; it does not emulate a jump host.')
    gate = TransportGate((config['hostname'], int(config['port'])))
    try:
        with tempfile.TemporaryDirectory(prefix='ports-reconnect-check-') as folder:
            root = Path(folder)
            overlay = root / 'ssh-config'
            host_key = config.get('hostkeyalias', config['hostname'])
            overlay.write_text(f'Host *\n  HostName 127.0.0.1\n  Port {gate.port}\n  HostKeyAlias {host_key}\n'
                               f'Include "{(Path.home() / ".ssh/config").as_posix()}"\n', encoding='utf-8')
            catalog = Catalog(root / 'data')
            machines = [catalog.add(options.host, 'Interrupted server', ssh_config=str(overlay)),
                        catalog.add(options.host, 'Unaffected server')]
            clients, rules = [], []
            try:
                for machine in machines:
                    with socket.socket() as reservation:
                        reservation.bind(('127.0.0.1', 0))
                        rule = Forward.make(reservation.getsockname()[1], options.remote_port, 'Temporary traffic check')
                    store = Store(machine.directory)
                    store.load()
                    store.save([rule])
                    client = DaemonClient(machine.target, machine.directory)
                    clients.append(client)
                    rules.append(rule)
                    client.start(rule)
                    wait_for(client, rule, 'ON')
                    traffic(rule, options.remote_port)
                pids = [exchange(m.directory, 'status')['pid'] for m in machines]
                overview = MultiMachineManager(catalog, machines[0])
                try:
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline and len(overview.running) != 2:
                        overview.poll()
                        time.sleep(.1)
                    assert len(overview.running) == 2
                finally:
                    overview.detach()
                print('PASS: two independent machine profiles carry traffic and appear ON in one overview', flush=True)
                gate.offline()
                wait_for(clients[0], rules[0], 'RETRYING')
                traffic(rules[1], options.remote_port)
                time.sleep(3)
                gate.enabled = True
                wait_for(clients[0], rules[0], 'ON')
                traffic(rules[0], options.remote_port)
                assert pids == [exchange(m.directory, 'status')['pid'] for m in machines]
                print('PASS: isolated transport loss recovers automatically after the overview closes; both controllers survive', flush=True)
                off = clients[0].upsert(Forward.make(32001, options.remote_port, 'Leave stopped'))
                restored = restart_daemon(machines[0].directory)
                assert restored == [rules[0].id]
                wait_for(clients[0], rules[0], 'ON')
                assert clients[0].status(off.id) == 'OFF'
                assert exchange(machines[1].directory, 'status')['pid'] == pids[1]
                traffic(rules[0], options.remote_port)
                print('PASS: explicit controller upgrade restores the ON forward, leaves OFF stopped, and preserves the other controller', flush=True)
                gate.offline()
                wait_for(clients[0], rules[0], 'RETRYING')
                clients[0].stop(rules[0].id)
                gate.enabled = True
                time.sleep(5)
                clients[0].poll()
                assert clients[0].status(rules[0].id) == 'OFF'
                traffic(rules[1], options.remote_port)
                print('PASS: manual stop while offline prevents reconnect; the other profile stays usable', flush=True)
            finally:
                for machine in machines:
                    try:
                        exchange(machine.directory, 'shutdown')
                    except (OSError, ValueError):
                        pass
                deadline = time.monotonic() + 8
                while any((m.directory / 'endpoint.json').exists() for m in machines) and time.monotonic() < deadline:
                    time.sleep(.1)
                assert not any((m.directory / 'endpoint.json').exists() for m in machines)
    finally:
        gate.close()
    print('PASS: isolated resources removed; original machines and tunnels untouched', flush=True)


if __name__ == '__main__':
    main()
