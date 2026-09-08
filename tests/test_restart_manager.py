from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from port_forward_tui.background import restart_daemon, Supervisor
from port_forward_tui.cli import arguments
from port_forward_tui.forwarding import Store, Forward
from tests.test_app import FakeManager
from dataclasses import asdict, replace


class RestartManagerTests(unittest.TestCase):
    def test_upgrade_restores_only_requested_connections(self):
        snapshot = {'pid': 1234, 'forwards': [{'id': key} for key in ('on', 'connecting', 'retrying', 'off', 'error')],
                    'states': {'on': 'ON', 'connecting': 'CONNECTING', 'retrying': 'RETRYING', 'off': 'OFF', 'error': 'ERROR'}}
        with patch('port_forward_tui.background.exchange', return_value=snapshot) as send, \
                patch('port_forward_tui.background.ensure_daemon') as launch, \
                patch('port_forward_tui.views.process_alive', return_value=False):
            restored = restart_daemon(Path('example'))
        self.assertEqual(restored, ['on', 'connecting', 'retrying'])
        self.assertEqual([c.kwargs['rule_id'] for c in send.call_args_list if c.args[1] == 'start'], restored)
        launch.assert_called_once()
        self.assertEqual(arguments(['restart-manager', '--machine', 'workbox', '--json']).machine, 'workbox')

    def test_editing_a_pending_retry_restarts_the_updated_mapping(self):
        with tempfile.TemporaryDirectory() as folder:
            store = Store(Path(folder))
            store.host = 'workbox'
            rule = Forward.make(18000, 8000)
            store.save([rule])
            manager = FakeManager()
            manager.host = store.host
            manager.states[rule.id] = 'RETRYING'
            supervisor = Supervisor(store, manager)
            updated = replace(rule, local_port=28000)
            result = supervisor.dispatch({'protocol': 1, 'token': supervisor.token, 'command': 'upsert',
                                          'rule': asdict(updated), 'expected': asdict(rule)})
            self.assertEqual(manager.stops, [rule.id])
            self.assertEqual(manager.starts, [updated])
            self.assertEqual(result['states'][rule.id], 'ON')
