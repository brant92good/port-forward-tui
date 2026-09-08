import io
from pathlib import Path
import socket
import tempfile
import unittest
from unittest.mock import patch

from port_forward_tui.forwarding import Forward, TunnelManager


class Process:
    next_pid = 100

    def __init__(self, message='', code=None):
        Process.next_pid += 1
        self.pid, self.code = Process.next_pid, code
        self.stderr = io.StringIO(message)

    def poll(self):
        return self.code

    def wait(self, timeout=None):
        return self.code

    def kill(self):
        self.code = -1


class Job:
    def attach(self, process):
        self.process = process

    def close(self):
        if hasattr(self, 'process'):
            self.process.kill()


class ReconnectTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.now = 100.0
        self.online = True
        self.failure = 'Connection timed out'
        self.processes = []
        with socket.socket() as reserve:
            reserve.bind(('127.0.0.1', 0))
            self.rule = Forward.make(reserve.getsockname()[1], 8000)
        self.manager = TunnelManager('example-server', Path(self.folder.name))
        self.addCleanup(self.manager.close)
        self.patches = [patch('port_forward_tui.forwarding.time.monotonic', lambda: self.now),
                        patch('port_forward_tui.forwarding.ProcessJob', Job),
                        patch('port_forward_tui.forwarding.subprocess.Popen', self.spawn),
                        patch('port_forward_tui.forwarding.listeners', self.listeners)]
        for item in self.patches:
            item.start()
            self.addCleanup(item.stop)

    def spawn(self, *args, **kwargs):
        process = Process() if self.online else Process(self.failure, 255)
        self.processes.append(process)
        return process

    def listeners(self):
        return {(p.pid, self.rule.local_port) for p in self.processes if p.code is None}

    def advance(self, seconds):
        self.now += seconds
        self.manager.poll()

    def lose_connection(self):
        self.online = False
        self.processes[-1].code = 255
        self.manager.poll()

    def test_interruption_retries_with_backoff_and_recovers_without_start(self):
        self.manager.start(self.rule)
        self.manager.poll()
        self.assertEqual(self.manager.status(self.rule.id), 'ON')
        self.lose_connection()
        self.assertEqual(self.manager.status(self.rule.id), 'RETRYING')
        self.advance(1)
        self.assertEqual(len(self.processes), 1)
        self.advance(1)
        self.manager.poll()
        self.assertEqual(self.manager.retries[self.rule.id] - self.now, 4)
        self.advance(4)
        self.manager.poll()
        self.assertEqual(self.manager.retries[self.rule.id] - self.now, 8)
        self.online = True
        self.advance(8)
        self.manager.poll()
        self.assertEqual(self.manager.status(self.rule.id), 'ON')
        self.assertEqual(len(self.manager.running), 1)
        self.advance(30)
        self.lose_connection()
        self.assertEqual(self.manager.retries[self.rule.id] - self.now, 2)

    def test_starting_while_offline_retries_and_delay_is_capped(self):
        self.online = False
        self.manager.start(self.rule)
        for delay in (2, 4, 8, 16, 30, 30):
            self.manager.poll()
            self.assertEqual(self.manager.retries[self.rule.id] - self.now, delay)
            self.advance(delay)

    def test_stop_and_stop_all_cancel_pending_retries(self):
        for stop in (lambda: self.manager.stop(self.rule.id), self.manager.close):
            self.manager.start(self.rule)
            self.lose_connection()
            stop()
            count = len(self.processes)
            self.online = True
            self.advance(300)
            self.assertEqual(self.manager.status(self.rule.id), 'OFF')
            self.assertFalse(self.manager.wanted)
            self.assertEqual(len(self.processes), count)

    def test_authentication_and_host_key_errors_do_not_retry(self):
        self.online = False
        for error in ('Permission denied (publickey).', 'Host key verification failed.',
                      'REMOTE HOST IDENTIFICATION HAS CHANGED!', 'Bad configuration option: typo'):
            self.failure = error
            self.manager.start(self.rule)
            self.manager.poll()
            count = len(self.processes)
            self.advance(300)
            self.assertEqual(self.manager.status(self.rule.id), 'ERROR')
            self.assertFalse(self.manager.wanted)
            self.assertFalse(self.manager.retries)
            self.assertEqual(len(self.processes), count)

    def test_retry_does_not_take_over_a_port_claimed_by_another_process(self):
        self.manager.start(self.rule)
        self.lose_connection()
        with socket.socket() as other:
            other.bind(('127.0.0.1', self.rule.local_port))
            other.listen()
            self.online = True
            self.advance(2)
            self.assertEqual(self.manager.status(self.rule.id), 'ERROR')
            self.assertFalse(self.manager.wanted)
            self.assertEqual(other.getsockname()[1], self.rule.local_port)

    def test_attempt_that_never_opens_listener_is_retried(self):
        with patch('port_forward_tui.forwarding.listeners', return_value=set()):
            self.manager.start(self.rule)
            self.advance(21)
            self.assertEqual(self.manager.status(self.rule.id), 'RETRYING')
            self.assertFalse(self.manager.running)

    def test_temporary_windows_status_failure_preserves_requested_tunnel(self):
        self.manager.start(self.rule)
        self.manager.poll()
        with patch('port_forward_tui.forwarding.listeners', side_effect=OSError('TCP table unavailable')):
            self.advance(300)
            self.assertEqual(self.manager.status(self.rule.id), 'ON')
            self.assertIn(self.rule.id, self.manager.running)
        self.manager.poll()
        self.assertEqual(self.manager.status(self.rule.id), 'ON')
