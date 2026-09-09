"""Linux CI: compiled Ports, disposable OpenSSH, concurrent traffic and recovery.

Creates fresh keys/configs, two loopback transport gates and an HTTP server.
Never imports the legacy app or reads user SSH configuration. Python and sshd
are test tools only; the compiled app launches the actual system ssh client.
"""
import argparse
import getpass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from urllib.request import ProxyHandler, build_opener
import uuid


class TransportGate:
    """Only closes this fixture's transport, including already connected sockets."""

    def __init__(self, target):
        self.target = target
        self.enabled = True
        self.closed = threading.Event()
        self.lock = threading.Lock()
        self.connections = set()
        self.workers = []
        self.accepted = 0
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen()
        self.listener.settimeout(.2)
        self.port = self.listener.getsockname()[1]
        self.thread = threading.Thread(target=self.accept)
        self.thread.start()

    def accept(self):
        while not self.closed.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with self.lock:
                self.accepted += 1
                worker = threading.Thread(target=self.relay, args=(client,))
                self.workers.append(worker)
                worker.start()

    def relay(self, client):
        remote = None
        try:
            with self.lock:
                if not self.enabled or self.closed.is_set():
                    return
                self.connections.add(client)
            remote = socket.create_connection(self.target, timeout=2)
            with self.lock:
                if not self.enabled or self.closed.is_set():
                    return
                self.connections.add(remote)
            while not self.closed.is_set():
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
            if remote is not None:
                remote.close()

    def offline(self):
        with self.lock:
            self.enabled = False
            for connection in tuple(self.connections):
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                connection.close()

    def online(self):
        with self.lock:
            self.enabled = True

    def count(self):
        with self.lock:
            return self.accepted

    def close(self):
        self.closed.set()
        self.offline()
        self.listener.close()
        self.thread.join(timeout=3)
        for worker in self.workers:
            worker.join(timeout=3)
        assert not self.thread.is_alive() and not any(w.is_alive() for w in self.workers)


def free_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]


def listening(port):
    try:
        with socket.create_connection(('127.0.0.1', port), timeout=.2):
            return True
    except OSError:
        return False


def proc_identity(pid):
    """Linux identity token; zombies are finished, and reused PIDs do not match."""
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return None if fields[0] == 'Z' else fields[19]
    except (FileNotFoundError, ProcessLookupError):
        return None


def owned_processes(binary, directories, configs):
    """Fallback cleanup is limited to an exact executable/config in our temp dir."""
    result = {}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            args = (entry/'cmdline').read_bytes().split(b'\0')[:-1]
            args = [os.fsdecode(value) for value in args]
            exe = (entry/'exe').resolve(strict=True)
            pairs = set(zip(args, args[1:]))
            controller = exe == binary and '--serve' in args and any(
                ('--data-dir', str(directory)) in pairs for directory in directories)
            transport = exe.name == 'ssh' and any(('-F', str(config)) in pairs for config in configs)
            if controller or transport:
                pid = int(entry.name)
                result[pid] = proc_identity(pid)
        except (OSError, ValueError):
            continue
    return result


class Fixture:
    def __init__(self, binary, root):
        self.binary, self.root = binary, root
        self.machines = []
        self.configs = []
        self.ports = []
        self.processes = {}

    def cli(self, *args, machine=None, structured=True):
        command = [str(self.binary), '--data-dir', str(self.root/'data')]
        if machine:
            command += ['--machine', machine]
        if structured:
            command += ['--json']
        result = subprocess.run([*command, *map(str, args)], capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, (command, result.returncode, result.stdout, result.stderr)
        if not structured:
            return result.stdout
        value = json.loads(result.stdout)
        assert value.get('ok'), value
        return value

    def directory(self, machine):
        assert re.fullmatch(r'[0-9a-f]{32}', machine)
        return self.root/'data'/'machines'/machine

    def status(self, machine):
        endpoint = json.loads((self.directory(machine)/'endpoint.json').read_text())
        pid = endpoint['pid']
        self.processes.setdefault(pid, proc_identity(pid))
        request = dict(protocol=1, token=endpoint['token'], command='status')
        with socket.create_connection(('127.0.0.1', endpoint['port']), timeout=2) as connection:
            connection.settimeout(2)
            connection.sendall(json.dumps(request).encode() + b'\n')
            with connection.makefile('rb') as reader:
                raw = reader.readline(2 * 1024 * 1024)
        value = json.loads(raw)
        assert value.get('ok'), value
        return value

    def off_favorites(self, machine, active):
        snapshot = self.status(machine)
        for rule in snapshot['forwards']:
            if rule['id'] != active:
                # Protocol 1 only records touched runtime entries; an untouched
                # saved favorite is OFF, as in the native CLI's status mapping.
                assert snapshot['states'].get(rule['id'], 'OFF') == 'OFF', snapshot
        return snapshot

    def wait_state(self, machine, rule, wanted, guard=lambda: None, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            guard()
            snapshot = self.off_favorites(machine, rule)
            state = snapshot['states'].get(rule)
            if state == wanted:
                return snapshot
            assert state != 'ERROR', snapshot
            time.sleep(.1)
        raise AssertionError((wanted, snapshot))

    def close(self):
        failures = []
        for machine in self.machines:
            endpoint = self.directory(machine)/'endpoint.json'
            if endpoint.exists():
                try:
                    self.cli('--stop-daemon', machine=machine, structured=False)
                except Exception as error:
                    failures.append(str(error))
        deadline = time.monotonic() + 6
        directories = [self.directory(machine) for machine in self.machines]
        while time.monotonic() < deadline:
            residual = owned_processes(self.binary, directories, self.configs)
            if not residual and not any((p/'endpoint.json').exists() for p in directories):
                break
            time.sleep(.1)
        else:
            failures.append('Normal shutdown left an owned controller/SSH process or endpoint')
        # Clean up even on a failed product assertion; never target a process by
        # name alone or signal an unverified PID copied from an endpoint file.
        for sig in (signal.SIGTERM, signal.SIGKILL):
            residual = owned_processes(self.binary, directories, self.configs)
            for pid, identity in residual.items():
                if identity is not None and proc_identity(pid) == identity:
                    try:
                        os.kill(pid, sig)
                    except ProcessLookupError:
                        pass
            if residual:
                time.sleep(.3)
        assert not owned_processes(self.binary, directories, self.configs), 'Owned process cleanup failed'
        assert not any(listening(port) for port in self.ports), 'Owned SSH listener survived cleanup'
        assert all(proc_identity(pid) != identity for pid, identity in self.processes.items()), 'Controller survived cleanup'
        assert not failures, failures


def exercise(fixture, gates, http_port, payload):
    opener = build_opener(ProxyHandler({}))  # Ignore runner proxy environment for loopback traffic.

    def traffic(port):
        with opener.open(f'http://127.0.0.1:{port}/', timeout=2) as response:
            assert response.read() == payload, 'Forward returned the wrong HTTP payload'

    rules = []
    for index, config in enumerate(fixture.configs):
        added = fixture.cli('machines', 'add', f'ports-fixture-{index}', '--name', f'Fixture {index}', '--config', config)
        machine = added['machine']['id']
        fixture.machines.append(machine)
        port = free_port()
        fixture.ports.append(port)
        saved = fixture.cli('save', '--remote', http_port, '--local', port, '--name', 'Loopback HTTP', machine=machine)
        rule = saved['id']
        rules.append(rule)
        snapshot = fixture.off_favorites(machine, None)
        assert len(snapshot['forwards']) >= 7, 'Expected initial OFF favorites plus saved test forward'
        assert not listening(port), 'Saving a favorite started a listener'
        fixture.cli('start', rule, '--wait', '5', machine=machine)
        fixture.wait_state(machine, rule, 'ON')
        traffic(port)

    a, b = fixture.machines
    ra, rb = rules
    pids = [fixture.status(machine)['pid'] for machine in fixture.machines]
    overview = fixture.cli('list')
    on_rows = {(row['machine_id'], row['id']) for row in overview['forwards'] if row['state'] == 'ON'}
    assert on_rows == {(a, ra), (b, rb)}, overview
    print('PASS: two auto-detached native controllers forward real HTTP over OpenSSH; one overview lists both.', flush=True)

    def unaffected():
        assert fixture.off_favorites(b, rb)['states'][rb] == 'ON'
        traffic(fixture.ports[1])

    gates[0].offline()
    fixture.wait_state(a, ra, 'RETRYING', unaffected)
    # Keep the outage through an actual retry, rather than restoring transport
    # before the retry scheduler has ever attempted a second connection.
    attempts = gates[0].count()
    deadline = time.monotonic() + 8
    while gates[0].count() == attempts and time.monotonic() < deadline:
        unaffected()
        time.sleep(.1)
    assert gates[0].count() > attempts, 'No automatic retry attempted during outage'
    fixture.wait_state(a, ra, 'RETRYING', unaffected)
    gates[0].online()
    fixture.wait_state(a, ra, 'ON', unaffected)
    traffic(fixture.ports[0])
    assert pids == [fixture.status(machine)['pid'] for machine in fixture.machines]
    print('PASS: one transport drops and retries automatically; the other keeps serving HTTP; original controllers recover.', flush=True)

    def cancelled(action):
        gates[0].offline()
        snapshot = fixture.wait_state(a, ra, 'RETRYING', unaffected)
        detail = snapshot['details'][ra]
        match = re.search(r'Retrying in (\d+)s', detail)
        assert match, detail
        delay = int(match.group(1))
        args = [action, ra] + (['--yes'] if action == 'delete' else [])
        fixture.cli(*args, machine=a)
        # Drain an accept already queued before cancellation; count only after
        # that short grace, then restore the route and exceed its retry timer.
        time.sleep(.3)
        attempts = gates[0].count()
        gates[0].online()
        deadline = time.monotonic() + delay + 1.5
        while time.monotonic() < deadline:
            unaffected()
            snapshot = fixture.off_favorites(a, None)
            if action == 'stop':
                assert snapshot['states'][ra] == 'OFF', snapshot
            else:
                # Deletion can retain the stopped runtime entry until shutdown.
                assert snapshot['states'].get(ra, 'OFF') == 'OFF', snapshot
                assert all(r['id'] != ra for r in snapshot['forwards']), snapshot
            assert not listening(fixture.ports[0]), 'Cancelled forward reopened its listener'
            assert gates[0].count() == attempts, 'Cancelled forward attempted to reconnect'
            time.sleep(.1)
        print(f'PASS: {action} while RETRYING cancels future attempts after route recovery; OFF favorites stay OFF.', flush=True)

    cancelled('stop')
    fixture.cli('start', ra, '--wait', '5', machine=a)
    fixture.wait_state(a, ra, 'ON', unaffected)
    traffic(fixture.ports[0])
    cancelled('delete')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--sudo-sshd', action='store_true', help='Disposable Linux CI runner only')
    options = parser.parse_args()
    if not sys.platform.startswith('linux'):
        parser.error('This qualification uses Linux /proc and an owned loopback sshd')
    binary = options.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='ports-openssh-') as temporary:
        root = Path(temporary)
        fixture = Fixture(binary, root)
        payload = ('ports-http-' + uuid.uuid4().hex).encode()

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass

        http = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        http_thread = threading.Thread(target=http.serve_forever)
        http_thread.start()
        server = None
        gates = []
        sshd_port = free_port()
        try:
            for name in ('client', 'host'):
                subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(root/name)], check=True, timeout=10)
            known = root/'known_hosts'
            known.write_text('ports-fixture ' + (root/'host.pub').read_text())
            server_config = root/'sshd_config'
            server_config.write_text(f'''Port {sshd_port}
ListenAddress 127.0.0.1
HostKey {root/'host'}
PidFile {root/'sshd.pid'}
AuthorizedKeysFile {root/'client.pub'}
StrictModes no
UsePAM yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
AllowTcpForwarding local
PermitOpen 127.0.0.1:{http.server_port}
PermitTTY no
X11Forwarding no
AllowAgentForwarding no
PrintMotd no
LogLevel VERBOSE
ForceCommand /bin/true
''')
            prefix = ['sudo', '-n'] if options.sudo_sshd else []
            with (root/'sshd.log').open('wb') as log:
                server = subprocess.Popen([*prefix, '/usr/sbin/sshd', '-D', '-e', '-f', str(server_config)], stdout=log, stderr=log)
            deadline = time.monotonic() + 10
            while not listening(sshd_port):
                assert server.poll() is None and time.monotonic() < deadline, (root/'sshd.log').read_text()
                time.sleep(.05)
            for index in range(2):
                gate = TransportGate(('127.0.0.1', sshd_port))
                gates.append(gate)
                config = root/f'ssh_config_{index}'
                config.write_text(f'''Host ports-fixture-{index}
 HostName 127.0.0.1
 Port {gate.port}
 User {getpass.getuser()}
 IdentityFile {root/'client'}
 IdentityAgent none
 IdentitiesOnly yes
 UserKnownHostsFile {known}
 GlobalKnownHostsFile /dev/null
 HostKeyAlias ports-fixture
 StrictHostKeyChecking yes
 BatchMode yes
 ConnectTimeout 5
''')
                fixture.configs.append(config)
                # Qualify account/key/config before attributing auth failures to Ports.
                result = subprocess.run(['ssh', '-F', str(config), f'ports-fixture-{index}', 'true'], capture_output=True, text=True, timeout=10)
                assert result.returncode == 0, (result.stdout, result.stderr, (root/'sshd.log').read_text())
            exercise(fixture, gates, http.server_port, payload)
        except BaseException:
            # No private keys, endpoint tokens or user files are emitted.
            for log in [root/'sshd.log', *(p/'port_forward_tui.background.log' for p in (root/'data'/'machines').glob('*'))]:
                if log.exists():
                    print(f'{log.name}:\n{log.read_text(errors="replace")[-12000:]}', file=sys.stderr)
            raise
        finally:
            try:
                fixture.close()
            finally:
                try:
                    for gate in gates:
                        gate.close()
                finally:
                    if server is not None and server.poll() is None:
                        server.terminate()
                        try:
                            server.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            server.kill()
                            server.wait(timeout=5)
                    http.shutdown()
                    http.server_close()
                    http_thread.join(timeout=3)
            assert not listening(sshd_port), 'Owned sshd listener survived cleanup'
            assert not http_thread.is_alive(), 'Owned HTTP thread survived cleanup'
        print('PASS: public controller shutdown removes endpoints and owned SSH processes/listeners; fixture servers closed.', flush=True)


if __name__ == '__main__':
    main()
