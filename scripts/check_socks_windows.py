"""Owned Windows OpenSSH SOCKS qualification; developer tooling, never runtime.

Requires a portable official Win32-OpenSSH directory and a compiled Ports beta.
No services, accounts, user SSH files, PATH or desktop windows are changed.
The trusted loopback fixture sshd starts just before assignment to a kill-on-close
job; clients start only after assignment. This fixture is not the production
suspended-spawn ownership proof. All generated private keys are removed in finally.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import select
import socket
import struct
import subprocess
import sys
import threading
import time
import traceback
import uuid

NO_WINDOW = 0x08000000


def run(args, **kwargs):
    return subprocess.run([str(arg) for arg in args], capture_output=True,
                          timeout=20, creationflags=NO_WINDOW, **kwargs)


def quote(path):
    return '"' + str(path).replace('\\', '/') + '"'


def free_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]


def preferred_port(port):
    """Check bind availability without sending bytes to an unknown service."""
    with socket.socket() as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        try:
            probe.bind(('127.0.0.1', port))
        except OSError:
            return free_port()
        return port


def listening(port):
    try:
        with socket.create_connection(('127.0.0.1', port), timeout=.2):
            return True
    except OSError:
        return False


def receive(connection, count):
    output = b''
    while len(output) < count:
        chunk = connection.recv(count - len(output))
        assert chunk, 'Unexpected EOF during SOCKS negotiation'
        output += chunk
    return output


def http_bytes(connection):
    connection.sendall(b'GET / HTTP/1.0\r\nHost: fixture\r\nConnection: close\r\n\r\n')
    output = b''
    while True:
        chunk = connection.recv(4096)
        if not chunk:
            break
        output += chunk
        assert len(output) < 65536, 'Unexpected oversized HTTP response'
    headers, body = output.split(b'\r\n\r\n', 1)
    assert b' 200 ' in headers.split(b'\r\n', 1)[0], headers
    return body


def http_get(port):
    with socket.create_connection(('127.0.0.1', port), timeout=2) as connection:
        return http_bytes(connection)


def socks_get(proxy_port, target_port, domain=False):
    with socket.create_connection(('127.0.0.1', proxy_port), timeout=3) as connection:
        connection.sendall(b'\x05\x01\x00')
        assert receive(connection, 2) == b'\x05\x00', 'SOCKS5 no-auth negotiation failed'
        # Literal DOMAIN bytes: this test never resolves the target on the client.
        address = b'\x03\x09localhost' if domain else b'\x01\x7f\x00\x00\x01'
        connection.sendall(b'\x05\x01\x00' + address + struct.pack('>H', target_port))
        header = receive(connection, 4)
        assert header[:3] == b'\x05\x00\x00', ('SOCKS CONNECT failed', header)
        size = {1: 4, 4: 16}.get(header[3])
        if header[3] == 3:
            size = receive(connection, 1)[0]
        assert size is not None, header
        receive(connection, size + 2)
        return http_bytes(connection)


class Http:
    def __init__(self, preferred=0):
        self.payload = ('owned-http-' + uuid.uuid4().hex).encode()
        self.requests = 0
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                owner.requests += 1
                self.send_response(200)
                self.send_header('Content-Length', str(len(owner.payload)))
                self.end_headers()
                self.wfile.write(owner.payload)

            def log_message(self, *_):
                pass

        class OwnedServer(ThreadingHTTPServer):
            allow_reuse_address = False

            def server_bind(self):
                self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
                super().server_bind()

        try:
            self.server = OwnedServer(('127.0.0.1', preferred), Handler)
        except OSError:
            if not preferred:
                raise
            self.server = OwnedServer(('127.0.0.1', 0), Handler)
        self.port = self.server.server_port

        class Ipv6Server(OwnedServer):
            address_family = socket.AF_INET6

            def server_bind(self):
                self.socket.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
                super().server_bind()

        # Windows localhost commonly resolves ::1 first. Own both loopback
        # families at the same port; never bind a wildcard/network interface.
        try:
            self.ipv6 = Ipv6Server(('::1', self.port), Handler)
        except BaseException:
            self.server.server_close()
            raise
        self.threads = [threading.Thread(target=server.serve_forever)
                        for server in (self.server, self.ipv6)]
        for thread in self.threads:
            thread.start()

    def close(self):
        for server in (self.server, self.ipv6):
            server.shutdown()
            server.server_close()
        for thread in self.threads:
            thread.join(timeout=3)
        assert not any(thread.is_alive() for thread in self.threads), 'HTTP fixture thread survived cleanup'


class Gate:
    """Bounded relay; unexpected errors cannot masquerade as product recovery."""
    def __init__(self, target):
        self.target = target
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.enabled = True
        self.generation = 0
        self.streams = set()
        self.workers = []
        self.errors = []
        self.accepted = 0
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen()
        self.listener.settimeout(.2)
        self.port = self.listener.getsockname()[1]
        self.thread = threading.Thread(target=self.accept)
        self.thread.start()

    def accept(self):
        while not self.stop.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError as error:
                if not self.stop.is_set():
                    self.errors.append(repr(error))
                break
            client.setblocking(True)
            client.settimeout(3)
            with self.lock:
                self.accepted += 1
                if not self.enabled or self.stop.is_set():
                    client.close()
                    continue
                generation = self.generation
                worker = threading.Thread(target=self.relay, args=(client, generation))
                self.workers.append(worker)
                worker.start()

    def relay(self, client, generation):
        remote = None
        try:
            with self.lock:
                if generation != self.generation or self.stop.is_set():
                    return
                self.streams.add(client)
            remote = socket.create_connection(self.target, timeout=3)
            remote.setblocking(True)
            remote.settimeout(3)
            with self.lock:
                if generation != self.generation or self.stop.is_set():
                    return
                self.streams.add(remote)
            while not self.stop.is_set():
                ready, _, _ = select.select([client, remote], [], [], .2)
                for source in ready:
                    data = source.recv(65536)
                    if not data:
                        return
                    (remote if source is client else client).sendall(data)
        except (OSError, ValueError) as error:
            with self.lock:
                expected = self.stop.is_set() or generation != self.generation
            # Reset/abort/broken pipe is also an ordinary peer termination.
            peer_closed = isinstance(error, (ConnectionResetError, ConnectionAbortedError, BrokenPipeError))
            if not expected and not peer_closed:
                self.errors.append(repr(error))
        finally:
            with self.lock:
                self.streams.discard(client)
                self.streams.discard(remote)
            client.close()
            if remote is not None:
                remote.close()

    def offline(self):
        with self.lock:
            self.enabled = False
            self.generation += 1
            for stream in tuple(self.streams):
                try:
                    stream.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                stream.close()

    def online(self):
        with self.lock:
            self.enabled = True

    def count(self):
        with self.lock:
            return self.accepted

    def check(self):
        assert not self.errors, self.errors

    def close(self):
        self.stop.set()
        self.offline()
        self.listener.close()
        self.thread.join(timeout=3)
        for worker in self.workers:
            worker.join(timeout=3)
        assert not self.thread.is_alive() and not any(w.is_alive() for w in self.workers)
        self.check()


class Basic(c.Structure):
    _fields_ = [('user', c.c_int64), ('kernel', c.c_int64), ('flags', w.DWORD),
                ('min', c.c_size_t), ('max', c.c_size_t), ('active', w.DWORD),
                ('affinity', c.c_size_t), ('priority', w.DWORD), ('scheduling', w.DWORD)]


class Io(c.Structure):
    _fields_ = [(name, c.c_uint64) for name in ('read', 'write', 'other', 'read_bytes', 'write_bytes', 'other_bytes')]


class Extended(c.Structure):
    _fields_ = [('basic', Basic), ('io', Io), ('process', c.c_size_t),
                ('job', c.c_size_t), ('peak_process', c.c_size_t), ('peak_job', c.c_size_t)]


def kernel():
    api = c.WinDLL('kernel32', use_last_error=True)
    for name, args, result in (
        ('CreateJobObjectW', [c.c_void_p, w.LPCWSTR], w.HANDLE),
        ('SetInformationJobObject', [w.HANDLE, c.c_int, c.c_void_p, w.DWORD], w.BOOL),
        ('AssignProcessToJobObject', [w.HANDLE, w.HANDLE], w.BOOL),
        ('CloseHandle', [w.HANDLE], w.BOOL),
        ('OpenProcess', [w.DWORD, w.BOOL, w.DWORD], w.HANDLE),
        ('QueryFullProcessImageNameW', [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)], w.BOOL),
        ('GetProcessTimes', [w.HANDLE, c.POINTER(w.FILETIME), c.POINTER(w.FILETIME), c.POINTER(w.FILETIME), c.POINTER(w.FILETIME)], w.BOOL),
        ('WaitForSingleObject', [w.HANDLE, w.DWORD], w.DWORD),
        ('TerminateProcess', [w.HANDLE, w.UINT], w.BOOL),
    ):
        function = getattr(api, name)
        function.argtypes, function.restype = args, result
    return api


def process_rows(parent=None, pid=None):
    assert (parent is None) != (pid is None)
    field, number = ('ParentProcessId', parent) if parent is not None else ('ProcessId', pid)
    command = (f"@(Get-CimInstance Win32_Process -Filter '{field} = {int(number)}' | "
               "Select-Object ProcessId,ExecutablePath,CommandLine) | ConvertTo-Json -Compress")
    powershell = Path(os.environ['SystemRoot'])/'System32/WindowsPowerShell/v1.0/powershell.exe'
    result = run([powershell, '-NoProfile', '-NonInteractive', '-Command', command], text=True)
    assert result.returncode == 0, result.stderr
    value = json.loads(result.stdout) if result.stdout.strip() else []
    return value if isinstance(value, list) else [value]


class Identity:
    """Keep a process HANDLE, so cleanup never retargets a reused PID."""
    def __init__(self, api, pid, executable, required):
        self.api, self.pid = api, pid
        self.handle = api.OpenProcess(0x1000 | 0x100000 | 1, False, pid)
        if not self.handle:
            raise c.WinError(c.get_last_error())
        try:
            size = w.DWORD(32768)
            path = c.create_unicode_buffer(size.value)
            assert api.QueryFullProcessImageNameW(self.handle, 0, path, c.byref(size))
            assert Path(path.value).resolve() == executable, 'Unexpected process executable'
            rows = process_rows(pid=pid)
            assert len(rows) == 1 and all(value in rows[0]['CommandLine'] for value in required), 'Unexpected process arguments'
            created, exited, kernel_time, user_time = (w.FILETIME() for _ in range(4))
            assert api.GetProcessTimes(self.handle, c.byref(created), c.byref(exited), c.byref(kernel_time), c.byref(user_time))
            self.created = (created.dwHighDateTime << 32) | created.dwLowDateTime
            assert self.running(), 'Process exited before identity capture'
        except BaseException:
            self.close()
            raise

    def running(self):
        return self.api.WaitForSingleObject(self.handle, 0) == 258

    def terminate(self):
        if self.running():
            assert self.api.TerminateProcess(self.handle, 125), 'Owned fallback termination failed'
            assert self.api.WaitForSingleObject(self.handle, 5000) == 0, 'Owned process did not stop'

    def close(self):
        if self.handle:
            self.api.CloseHandle(self.handle)
            self.handle = None


class Fixture:
    def __init__(self, binary, root, api):
        self.binary, self.root, self.api = binary, root, api
        self.machines, self.identities, self.configs, self.ports = [], {}, [], []
        self.ssh = (Path(os.environ['SystemRoot'])/'System32/OpenSSH/ssh.exe').resolve()

    def cli(self, *args, machine=None, fail=False, structured=True):
        command = [self.binary, '--data-dir', self.root/'data']
        if machine:
            command += ['--machine', machine]
        if structured:
            command += ['--json']
        result = run([*command, *args], text=True, encoding='utf-8')
        assert (result.returncode != 0) if fail else (result.returncode == 0), (args, result.returncode, result.stdout, result.stderr)
        if not structured:
            return result.stdout
        value = json.loads(result.stdout)
        assert value.get('ok') is (not fail), value
        return value

    def directory(self, machine):
        assert re.fullmatch('[0-9a-f]{32}', machine)
        return self.root/'data'/'machines'/machine

    def capture(self, machine):
        endpoint = json.loads((self.directory(machine)/'endpoint.json').read_text())
        pid = endpoint['pid']
        if pid not in self.identities:
            self.identities[pid] = Identity(self.api, pid, self.binary, ['--serve', str(self.directory(machine))])
        return endpoint

    def status(self, machine):
        endpoint = self.capture(machine)
        request = dict(protocol=2, token=endpoint['token'], command='status')
        with socket.create_connection(('127.0.0.1', endpoint['port']), timeout=2) as connection:
            connection.sendall(json.dumps(request).encode() + b'\n')
            with connection.makefile('rb') as reader:
                raw = reader.readline(2 * 1024 * 1024)
        result = json.loads(raw)
        assert result.get('ok') and result.get('protocol') == 2, result.get('error')
        return result

    def children(self, machine):
        endpoint = self.capture(machine)
        for row in process_rows(parent=endpoint['pid']):
            if not row['ExecutablePath'] or Path(row['ExecutablePath']).resolve() != self.ssh:
                continue
            pid = row['ProcessId']
            if pid not in self.identities:
                self.identities[pid] = Identity(self.api, pid, self.ssh, ['-F', str(self.root)])

    def wait(self, machine, rule, wanted, guard=lambda: None, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            guard()
            status = self.status(machine)
            state = status['states'].get(rule, 'OFF')
            if state == wanted:
                return status
            assert state != 'ERROR' or wanted == 'ERROR', status['details'].get(rule)
            time.sleep(.1)
        raise AssertionError((wanted, state, status['details'].get(rule)))

    def close(self):
        errors = []
        # Capture any controller created just before an earlier assertion failed.
        for machine in self.machines:
            if not (self.directory(machine)/'endpoint.json').exists():
                continue
            try:
                self.capture(machine)
                self.children(machine)
                self.cli('--stop-daemon', machine=machine, structured=False)
            except BaseException as error:
                errors.append(repr(error))
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline and any(identity.running() for identity in self.identities.values()):
            time.sleep(.05)
        residual = [identity.pid for identity in self.identities.values() if identity.running()]
        if residual:
            errors.append(f'Normal shutdown left owned processes: {residual}')
        try:
            for identity in self.identities.values():
                identity.terminate()
            assert all(not identity.running() for identity in self.identities.values())
            assert not any(listening(port) for port in self.ports), 'Owned tunnel listener survived'
            assert not any((self.directory(machine)/'endpoint.json').exists() for machine in self.machines), 'Controller endpoint survived'
            assert not errors, errors
        finally:
            for identity in self.identities.values():
                identity.close()


def exercise(fixture, gates, servers, configs, refused_port, record):
    for index, config in enumerate(configs):
        machine = fixture.cli('machines', 'add', f'fixture-{index}', '--name', f'Owned fixture {index}', '--config', config)['machine']['id']
        fixture.machines.append(machine)
    proxy_machine, fixed_machine, bad_machine = fixture.machines
    proxy_port = preferred_port(1080)
    fixed_port = free_port()
    fixture.ports += [proxy_port, fixed_port]
    proxy = fixture.cli('save', '--socks', '--local', str(proxy_port), '--name', 'Owned SOCKS', machine=proxy_machine)['id']
    fixed = fixture.cli('save', '--remote', str(servers[1].port), '--local', str(fixed_port), '--name', 'Unaffected HTTP', machine=fixed_machine)['id']
    assert not listening(proxy_port) and not listening(fixed_port), 'Save started SSH'
    for machine in (proxy_machine, fixed_machine):
        snapshot = fixture.status(machine)
        assert not snapshot['running'] and all(state == 'OFF' for state in snapshot['states'].values()), snapshot['states']
    record('save_off', proxy_port=proxy_port, collision_port=servers[0].port)
    fixture.cli('start', proxy, machine=proxy_machine)
    fixture.cli('start', fixed, machine=fixed_machine)
    fixture.wait(proxy_machine, proxy, 'ON')
    fixture.wait(fixed_machine, fixed, 'ON')
    for machine in (proxy_machine, fixed_machine):
        fixture.children(machine)
    pids = [fixture.status(m)['pid'] for m in (proxy_machine, fixed_machine)]
    for server in servers:
        assert socks_get(proxy_port, server.port) == server.payload
    assert socks_get(proxy_port, servers[0].port, domain=True) == servers[0].payload
    record('raw_socks5_ipv4_two_destinations_and_domain', domain='localhost', domain_atyp=3)
    before = time.monotonic()
    try:
        socks_get(proxy_port, refused_port)
    except (OSError, AssertionError, ValueError):
        pass
    else:
        raise AssertionError('A bound, non-listening destination returned HTTP')
    assert time.monotonic() - before < 5, 'Refused destination was not bounded'
    assert fixture.status(proxy_machine)['states'][proxy] == 'ON'
    assert socks_get(proxy_port, servers[1].port) == servers[1].payload
    record('permitted_closed_destination_fails_without_killing_proxy')

    def unaffected():
        for gate in gates:
            gate.check()
        status = fixture.status(fixed_machine)
        assert status['states'][fixed] == 'ON'
        assert http_get(fixed_port) == servers[1].payload

    collision = fixture.cli('save', '--socks', '--local', str(servers[0].port), '--name', 'Collision', machine=proxy_machine)['id']
    count = servers[0].requests
    result = fixture.cli('start', collision, machine=proxy_machine, fail=True)
    assert 'already in use' in json.dumps(result), result
    assert servers[0].requests == count, 'Collision probe sent application traffic'
    assert fixture.status(proxy_machine)['states'][collision] == 'ERROR'
    assert http_get(servers[0].port) == servers[0].payload
    assert socks_get(proxy_port, servers[1].port) == servers[1].payload
    record('occupied_port_preserved_and_other_proxy_still_works')

    bad_port = free_port()
    fixture.ports.append(bad_port)
    bad = fixture.cli('save', '--socks', '--local', str(bad_port), '--name', 'Wrong trust', machine=bad_machine)['id']
    fixture.cli('start', bad, machine=bad_machine, fail=True)
    snapshot = fixture.wait(bad_machine, bad, 'ERROR')
    assert any(text in snapshot['details'][bad].lower() for text in ('host key verification failed', 'remote host identification has changed')), snapshot['details'][bad]
    assert not listening(bad_port)
    count = gates[1].count()
    time.sleep(2.5)
    assert fixture.status(bad_machine)['states'][bad] == 'ERROR'
    assert gates[1].count() == count, 'Trust failure scheduled a retry'
    record('strict_host_trust_failure_terminal')

    gates[0].offline()
    fixture.wait(proxy_machine, proxy, 'RETRYING', unaffected)
    attempts = gates[0].count()
    deadline = time.monotonic() + 8
    while gates[0].count() == attempts and time.monotonic() < deadline:
        unaffected()
        time.sleep(.1)
    assert gates[0].count() > attempts, 'No actual reconnect attempt during outage'
    fixture.wait(proxy_machine, proxy, 'RETRYING', unaffected)
    gates[0].online()
    fixture.wait(proxy_machine, proxy, 'ON', unaffected)
    assert socks_get(proxy_port, servers[1].port, domain=True) == servers[1].payload
    assert pids == [fixture.status(m)['pid'] for m in (proxy_machine, fixed_machine)]
    fixture.children(proxy_machine)
    record('recovery_same_controllers_unaffected_fixed_http')

    for action in ('stop', 'stop-all', 'delete'):
        gates[0].offline()
        snapshot = fixture.wait(proxy_machine, proxy, 'RETRYING', unaffected)
        match = re.search(r'Retrying in (\d+)s', snapshot['details'][proxy])
        assert match, snapshot['details'][proxy]
        delay = int(match.group(1))
        args = [action] if action == 'stop-all' else [action, proxy]
        if action == 'delete':
            args.append('--yes')
        fixture.cli(*args, machine=proxy_machine)
        time.sleep(.3)  # Drain an accept already queued before cancellation.
        attempts = gates[0].count()
        gates[0].online()
        deadline = time.monotonic() + delay + 1.5
        while time.monotonic() < deadline:
            unaffected()
            snapshot = fixture.status(proxy_machine)
            assert snapshot['states'].get(proxy, 'OFF') == 'OFF'
            if action == 'delete':
                assert all(row['id'] != proxy for row in snapshot['forwards'])
            assert not listening(proxy_port)
            assert gates[0].count() == attempts, f'{action} allowed retry after cancellation'
            time.sleep(.1)
        record('retry_cancel_' + action, observed_retry_delay_seconds=delay)
        if action != 'delete':
            fixture.cli('start', proxy, machine=proxy_machine)
            fixture.wait(proxy_machine, proxy, 'ON', unaffected)
            assert socks_get(proxy_port, servers[0].port) == servers[0].payload
            fixture.children(proxy_machine)
    for machine in fixture.machines:
        snapshot = fixture.status(machine)
        for row in snapshot['forwards']:
            if row['id'] not in (fixed, collision, bad):
                assert snapshot['states'].get(row['id'], 'OFF') == 'OFF'
    record('untouched_favorites_remain_off')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--sshd-dir', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    options = parser.parse_args()
    if os.name != 'nt':
        parser.error('This fixture requires Windows; use the Linux transport fixture there.')
    binary = options.binary.resolve(strict=True)
    portable = options.sshd_dir.resolve(strict=True)
    sshd = (portable/'sshd.exe').resolve(strict=True)
    keygen = Path(os.environ['SystemRoot'])/'System32/OpenSSH/ssh-keygen.exe'
    root = options.artifacts.resolve()/('socks-windows-' + uuid.uuid4().hex)
    root.mkdir(parents=True)
    api = kernel()
    fixture = Fixture(binary, root, api)
    servers, gates, server, job, refused = [], [], None, None, None
    failures, observations = [], []
    started = time.monotonic()
    result = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'sshd_sha256': hashlib.sha256(sshd.read_bytes()).hexdigest(),
              'root': str(root), 'observations': observations}

    def record(case, **details):
        observations.append({'case': case, 'elapsed_seconds': round(time.monotonic()-started, 3), **details})
        print('PASS: ' + case, flush=True)

    print('Owned fixture: ' + str(root), flush=True)
    try:
        version = run([binary, '--version'], text=True)
        assert version.returncode == 0, version.stderr
        result['version'] = version.stdout.strip()
        servers.append(Http(8765))
        servers.append(Http())
        refused = socket.socket()
        refused.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        refused.bind(('127.0.0.1', 0))  # Reserve a permitted port without listening.
        refused_port = refused.getsockname()[1]
        for name in ('host', 'client', 'wrong_host'):
            generated = run([keygen, '-q', '-t', 'ed25519', '-N', '', '-f', root/name])
            assert generated.returncode == 0, generated.stderr.decode(errors='replace')
        sshd_port = free_port()
        result['sshd_port'] = sshd_port
        config = root/'sshd_config'
        allowed = ' '.join(f'{host}:{port}' for host in ('127.0.0.1', 'localhost') for port in [*(server.port for server in servers), refused_port])
        config.write_text('\n'.join([
            f'Port {sshd_port}', 'ListenAddress 127.0.0.1', f'HostKey {quote(root/"host")}',
            f'PidFile {quote(root/"sshd.pid")}', f'AuthorizedKeysFile {quote(root/"client.pub")}',
            'StrictModes no', 'PasswordAuthentication no', 'PubkeyAuthentication yes',
            'AuthenticationMethods publickey', 'PermitEmptyPasswords no', 'PermitTTY no',
            'AllowTcpForwarding local', f'PermitOpen {allowed}', 'AllowStreamLocalForwarding no',
            'PermitListen none', 'AllowAgentForwarding no', 'X11Forwarding no', 'PermitTunnel no',
            'PermitUserEnvironment no', 'MaxAuthTries 2', 'LoginGraceTime 10',
            f'ForceCommand {quote(portable/"sftp-server.exe")}', 'LogLevel DEBUG3', '',
        ]), encoding='utf-8')
        check = run([sshd, '-t', '-f', config])
        (root/'config-check.log').write_bytes(check.stdout + check.stderr)
        assert check.returncode == 0, check.stderr.decode(errors='replace')
        job = api.CreateJobObjectW(None, None)
        assert job, 'Cannot create fixture process job'
        limits = Extended()
        limits.basic.flags = 0x2000
        assert api.SetInformationJobObject(job, 9, c.byref(limits), c.sizeof(limits))
        with (root/'sshd.log').open('wb') as log:
            server = subprocess.Popen([str(sshd), '-D', '-e', '-f', str(config)],
                                      stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                      creationflags=NO_WINDOW, cwd=root,
                                      env={**os.environ, 'PATH': str(portable)+os.pathsep+os.environ['PATH']})
        assert api.AssignProcessToJobObject(job, w.HANDLE(int(server._handle))), 'Cannot own fixture sshd'
        deadline = time.monotonic() + 8
        while not listening(sshd_port):
            assert server.poll() is None and time.monotonic() < deadline, 'Fixture sshd did not start'
            time.sleep(.05)
        gates.append(Gate(('127.0.0.1', sshd_port)))
        gates.append(Gate(('127.0.0.1', sshd_port)))
        for index in range(3):
            known = root/f'known_hosts_{index}'
            public = (root/('wrong_host.pub' if index == 2 else 'host.pub')).read_text().split()
            known.write_text(f'owned-socks-fixture {public[0]} {public[1]}\n', encoding='ascii')
            client_config = root/f'client_config_{index}'
            client_config.write_text('\n'.join([
                f'Host fixture-{index}', ' HostName 127.0.0.1', f' Port {gates[min(index, 1)].port}',
                f' User {os.environ["USERNAME"]}', f' IdentityFile {quote(root/"client")}',
                ' IdentitiesOnly yes', ' IdentityAgent none', ' BatchMode yes',
                ' StrictHostKeyChecking yes', f' UserKnownHostsFile {quote(known)}',
                ' GlobalKnownHostsFile none', ' HostKeyAlias owned-socks-fixture',
                ' ConnectTimeout 3', '',
            ]), encoding='utf-8')
            fixture.configs.append(client_config)
        exercise(fixture, gates, servers, fixture.configs, refused_port, record)
    except BaseException:
        failures.append(traceback.format_exc())
        for machine in fixture.machines:
            try:
                if (fixture.directory(machine)/'endpoint.json').exists():
                    # Status has no auth token and names only owned test metadata.
                    (root/f'failed-status-{machine}.json').write_text(
                        json.dumps(fixture.status(machine), indent=2), encoding='utf-8')
            except BaseException as error:
                failures.append('Failure-status capture: ' + repr(error))
    finally:
        for close in [fixture.close, *(gate.close for gate in gates)]:
            try:
                close()
            except BaseException:
                failures.append(traceback.format_exc())
        if job:
            api.CloseHandle(job)
        if server:
            try:
                if server.poll() is None:
                    try:
                        server.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait(timeout=5)
                        failures.append('Fixture job did not stop sshd normally')
                assert server.poll() is not None
            except BaseException:
                failures.append(traceback.format_exc())
        for owned in servers:
            try:
                owned.close()
            except BaseException:
                failures.append(traceback.format_exc())
        if refused is not None:
            refused.close()
        for name in ('host', 'client', 'wrong_host'):
            try:
                (root/name).unlink(missing_ok=True)
            except BaseException:
                failures.append(traceback.format_exc())
        result['private_keys_removed'] = all(not (root/name).exists() for name in ('host', 'client', 'wrong_host'))
        result['sshd_stopped'] = server is None or server.poll() is not None
        result['sshd_listener_absent'] = 'sshd_port' not in result or not listening(result['sshd_port'])
        try:
            result['binary_sha256_after'] = hashlib.sha256(binary.read_bytes()).hexdigest()
            if result['binary_sha256_after'] != result['binary_sha256']:
                failures.append('Candidate executable changed during qualification')
        except BaseException:
            failures.append(traceback.format_exc())
        result['failures'] = failures
        result['ok'] = not failures and result['private_keys_removed'] and result['sshd_stopped'] and result['sshd_listener_absent']
        result['elapsed_seconds'] = round(time.monotonic()-started, 3)
        # Evidence writing follows all cleanup; a full disk cannot skip cleanup.
        (root/'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    for failure in failures:
        print(failure, file=sys.stderr)
    print(json.dumps({'ok': result['ok'], 'report': str(root/'result.json')}), flush=True)
    return 0 if result['ok'] else 1


if __name__ == '__main__':
    sys.exit(main())
