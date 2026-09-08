"""Combine independent machine controllers without merging their saved files."""
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field, replace
import time

from port_forward_tui.background import DaemonClient, exchange
from port_forward_tui.forwarding import Forward, Store, requested


@dataclass
class MachineConnections:
    machine: object
    store: Store
    states: dict
    logs: dict
    error: str = ''
    auto_reconnect: bool = True
    generation: int = 0
    action_errors: dict = field(default_factory=dict)


def read_connections(machine):
    store = Store(machine.directory)
    if not store.path.is_file():
        raise OSError('Saved machine settings are no longer available.')
    store.load()
    if not (machine.directory / 'endpoint.json').exists():
        return store, None
    snapshot = exchange(machine.directory, 'status', timeout=.75)
    if (snapshot.get('host'), snapshot.get('ssh_port'), snapshot.get('ssh_config')) != (
            store.host, store.ssh_port, store.ssh_config):
        raise OSError('The running manager has different SSH settings. Restore its settings before reconnecting.')
    return store, snapshot


class MultiMachineManager:
    persistent = True
    shared_favorites = True

    def __init__(self, catalog, selected, client_factory=DaemonClient, control=exchange):
        self.catalog, self.active_id = catalog, selected.id
        self.client_factory = client_factory
        self.control = control
        self.catalog_error = ''
        self.entries = {}
        self.order = []
        self.pending = {}
        self.pool = ThreadPoolExecutor(max_workers=4, thread_name_prefix='port-status')
        self.next_poll = self.next_catalog = 0
        self.disposed = False
        self.refresh_catalog()

    def refresh_catalog(self):
        machines = self.catalog.list()
        machines.sort(key=lambda m: (m.id != self.active_id if not self.order else False, m.name.casefold(), m.id))
        for machine in machines:
            if machine.id not in self.entries:
                store = Store(machine.directory)
                store.load()
                states = {r.id: 'UNKNOWN' for r in store.forwards} if (machine.directory / 'endpoint.json').exists() else {}
                self.entries[machine.id] = MachineConnections(machine, store, states, {})
        self.order = [m.id for m in machines]
        if self.active_id not in self.order and self.order:
            self.active_id = self.order[0]
        # Keep the selected machine first only on initial open; keyboard movement
        # must never reorder the list under the cursor.
        self.order.sort(key=lambda key: list(self.entries).index(key))

    @property
    def active(self):
        return self.entries[self.active_id]

    @property
    def forwards(self):
        return [replace(rule, id=f'{key}:{rule.id}') for key in self.order
                for rule in self.entries[key].store.forwards]

    @property
    def running(self):
        return {rule.id: True for rule in self.forwards if self.status(rule.id) in ('ON', 'CONNECTING')}

    def split(self, rule_id):
        if ':' in rule_id:
            key, original = rule_id.split(':', 1)
        else:
            key, original = self.active_id, rule_id
        if key not in self.order:
            raise OSError('This machine was removed. Choose another machine with H.')
        return key, original

    def status(self, rule_id):
        key, original = self.split(rule_id)
        entry = self.entries[key]
        if original in entry.action_errors:
            return 'ERROR'
        return 'UNKNOWN' if entry.error else entry.states.get(original, 'OFF')

    def details(self, rule_id):
        key, original = self.split(rule_id)
        entry = self.entries[key]
        if original in entry.action_errors:
            return entry.action_errors[original]
        if entry.error:
            return entry.error + '\nOther servers remain available; status will be checked again automatically.'
        text = entry.logs.get(original, '')
        if not entry.auto_reconnect:
            text += '\nThis older background manager needs restarting to enable automatic reconnection. See README: Updating.'
        return text

    def poll(self):
        now = time.monotonic()
        for key, (future, generation) in list(self.pending.items()):
            if not future.done():
                continue
            del self.pending[key]
            entry = self.entries.get(key)
            try:
                store, snapshot = future.result()
                if entry is None or generation != entry.generation:
                    continue
                entry.store = store
                entry.error = ''
                entry.states = snapshot['states'] if snapshot else {}
                entry.logs = snapshot['details'] if snapshot else {}
                entry.auto_reconnect = snapshot is None or 'auto_reconnect' in snapshot.get('capabilities', [])
                if snapshot:
                    entry.store.forwards = [Forward(**r) for r in snapshot['forwards']]
            except (OSError, ValueError, KeyError, TypeError) as error:
                if entry is not None and generation == entry.generation:
                    entry.error = f'Cannot read this server\'s background manager: {error}'
        if now >= self.next_catalog:
            try:
                self.refresh_catalog()
                self.catalog_error = ''
            except (OSError, ValueError, KeyError, TypeError) as error:
                # Keep known rows and their stop controls usable if one saved
                # file is damaged while the overview is open.
                self.catalog_error = str(error)
            self.next_catalog = now + 2
        if now >= self.next_poll and not self.disposed:
            for key in self.order:
                if key not in self.pending:
                    entry = self.entries[key]
                    self.pending[key] = (self.pool.submit(read_connections, entry.machine), entry.generation)
            self.next_poll = now + .75

    def _client(self, key):
        entry = self.entries[key]
        if not entry.store.keep_alive:
            raise OSError('This machine uses foreground mode. Open it with --foreground to manage its connections.')
        return self.client_factory(entry.machine.target, entry.machine.directory)

    def _applied(self, key, client):
        entry = self.entries[key]
        entry.generation += 1  # Discard an older status read completing after this write.
        entry.store.forwards = list(client.forwards)
        entry.states, entry.logs = dict(client.states), dict(client.logs)
        entry.error = ''
        entry.auto_reconnect = getattr(client, 'auto_reconnect', True)

    def upsert(self, rule, expected=None, start=False):
        key, original = self.split(rule.id)
        self.entries[key].action_errors.pop(original, None)
        if expected is not None and self.split(expected.id)[0] != key:
            raise OSError('The edited connection belongs to another machine.')
        client = self._client(key)
        saved = client.upsert(replace(rule, id=original),
                              expected=replace(expected, id=self.split(expected.id)[1]) if expected else None)
        self._applied(key, client)
        projected = replace(saved, id=f'{key}:{saved.id}')
        if start:
            self.start(projected)
        return projected

    def start(self, rule):
        key, original = self.split(rule.id)
        self.entries[key].action_errors.pop(original, None)
        try:
            collision = next((r for r in self.forwards if r.id != rule.id and r.local_port == rule.local_port
                              and requested(self, r.id)), None)
            if collision:
                owner = self.entries[self.split(collision.id)[0]].machine.name
                raise OSError(f'Local port {rule.local_port} is already requested by {owner}: {collision.name}. '
                              'Press E to choose another local port, such as 18000.')
            client = self._client(key)
            client.start(replace(rule, id=original))
            self._applied(key, client)
        except OSError as error:
            entry = self.entries[key]
            entry.generation += 1
            entry.action_errors[original] = str(error)

    def stop(self, rule_id):
        key, original = self.split(rule_id)
        result = self.control(self.entries[key].machine.directory, 'stop', rule_id=original)
        self._stopped(key, result)
        self.entries[key].action_errors.pop(original, None)

    def _stopped(self, key, snapshot):
        entry = self.entries[key]
        entry.generation += 1
        entry.states, entry.logs = snapshot['states'], snapshot['details']
        entry.error = ''

    def delete(self, rule):
        key, original = self.split(rule.id)
        client = self._client(key)
        client.delete(replace(rule, id=original))
        self._applied(key, client)
        self.entries[key].action_errors.pop(original, None)

    def close(self):
        errors = []
        for key in self.order:
            try:
                entry = self.entries[key]
                entry.action_errors.clear()
                if not (entry.machine.directory / 'endpoint.json').exists():
                    continue
                result = self.control(entry.machine.directory, 'stop_all')
                self._stopped(key, result)
            except (OSError, ValueError, KeyError) as error:
                errors.append(f'{self.entries[key].machine.name}: {error}')
        if errors:
            raise OSError('Some servers could not be stopped: ' + '; '.join(errors))

    def detach(self):
        self.disposed = True
        self.pool.shutdown(wait=False, cancel_futures=True)


class SelectedStore:
    """UI-only view; all writes go through the selected machine's controller."""
    def __init__(self, manager):
        self.manager = manager
        self.forwards = manager.forwards

    @property
    def host(self):
        return self.manager.active.machine.target

    @property
    def machine_name(self):
        return self.manager.active.machine.name

    @property
    def directory(self):
        return self.manager.active.machine.directory
