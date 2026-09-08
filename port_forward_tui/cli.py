"""Commands for people, scripts, and coding agents; no screen control needed."""
import argparse
from dataclasses import asdict, replace
import json
from pathlib import Path
import sys
import time

from port_forward_tui.forwarding import DATA_DIR, Forward, Store, port


class UsageError(ValueError):
    pass


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise UsageError(message)


def arguments(argv):
    shared = argparse.ArgumentParser(add_help=False)
    shared.add_argument('--json', action='store_true', default=argparse.SUPPRESS, help='Print one JSON result for scripts')
    shared.add_argument('--data-dir', type=Path, default=argparse.SUPPRESS, help='Use a separate saved-connections folder')
    shared.add_argument('--machine', default=argparse.SUPPRESS, help='Saved machine id or unambiguous name')
    parser = Parser(description='Save and control connections to apps on your remote computer.', parents=[shared])
    commands = parser.add_subparsers(dest='command', required=True, parser_class=Parser)
    machines = commands.add_parser('machines', help='List, add or import machines without connecting', parents=[shared])
    machine_commands = machines.add_subparsers(dest='machine_command', required=True, parser_class=Parser)
    machine_commands.add_parser('list', parents=[shared], help='List saved machines')
    add = machine_commands.add_parser('add', parents=[shared], help='Save a machine; no SSH connection is opened')
    add.add_argument('target', help='SSH config alias or user@address')
    add.add_argument('--name', default='')
    add.add_argument('--ssh-port', type=port, help='Optional SSH login port, not the forwarded app port')
    add.add_argument('--config', type=Path, help='Optional SSH configuration file to use at connection time')
    for verb in ('discover', 'import'):
        command = machine_commands.add_parser(verb, parents=[shared], help='Read SSH host names' if verb == 'discover' else 'Save host names from SSH config')
        command.add_argument('--config', type=Path, help='Defaults to ~/.ssh/config')
        if verb == 'import':
            command.add_argument('--select', action='append', help='Import this alias only; repeat for multiple names')
    for name, help_text in [('doctor', 'Check this computer without changing it or contacting SSH'),
                            ('list', 'Read saved connections and any available live status'),
                            ('restart-manager', 'Load an app update; briefly interrupt and restore this machine\'s requested connections'),
                            ('stop-all', 'Stop every connection in this data folder')]:
        commands.add_parser(name, help=help_text, parents=[shared])
    save = commands.add_parser('save', help='Save a favorite without opening a connection', parents=[shared])
    save.add_argument('--remote', type=port, required=True, help='Port used by the app on your remote computer')
    save.add_argument('--local', type=port, help='Port on this computer; defaults to the remote port')
    save.add_argument('--name', default='', help='Optional name, such as My web app')
    for name in ('start', 'stop', 'delete'):
        command = commands.add_parser(name, parents=[shared], help=f'{name.title()} one favorite using its ID from list')
        command.add_argument('id', help='Exact favorite ID from list; names and row numbers are not IDs')
        if name == 'start':
            command.add_argument('--wait', type=float, default=5, help='Seconds to wait for a listener, from 0 to 30 (default 5)')
        if name == 'delete':
            command.add_argument('--yes', action='store_true', help='Confirm removing the favorite and stopping its connection')
    options = parser.parse_args(argv)
    options.json = getattr(options, 'json', False)
    options.data_dir = getattr(options, 'data_dir', DATA_DIR)
    options.machine = getattr(options, 'machine', None)
    if options.command == 'start' and not 0 <= options.wait <= 30:
        raise UsageError('--wait must be between 0 and 30 seconds.')
    if options.command == 'delete' and not options.yes:
        raise UsageError('Deleting stops this connection and removes its favorite. Add --yes to confirm.')
    if options.command == 'save' and len(options.name.strip()) > 80:
        raise UsageError('Use a name of 80 characters or fewer.')
    return options


def read_store(directory):
    store = Store(directory)
    if store.path.exists():
        store.load()  # Avoid creating starter favorites in a read-only command.
    return store


def listing(store):
    from port_forward_tui.background import exchange
    snapshot = None
    warning = ''
    if store.path.exists() and (store.directory / 'endpoint.json').exists():
        try:
            snapshot = exchange(store.directory, 'status')
        except (OSError, ValueError, KeyError):
            warning = 'Live status is unavailable. Existing connections might still be running; reopen the app to check.'
    rules = snapshot.get('forwards', []) if snapshot else [asdict(r) for r in store.forwards]
    states = snapshot.get('states', {}) if snapshot else {}
    return dict(host=store.host, background='connected' if snapshot else 'not_connected', warning=warning,
                forwards=[dict(r, state=states.get(r['id'], 'OFF') if snapshot else 'UNKNOWN',
                               url=f"http://127.0.0.1:{r['local_port']}") for r in rules])


def execute(options):
    from port_forward_tui.machines import Catalog, import_ssh, ssh_aliases
    catalog = Catalog(options.data_dir)
    if options.command == 'machines':
        if options.machine_command == 'discover':
            return dict(ok=True, aliases=ssh_aliases(options.config))
        if options.machine_command == 'add':
            added = catalog.add(options.target, options.name, options.ssh_port, options.config)
            return dict(ok=True, machine=added.public(), machines=[m.public() for m in catalog.list()])
        if options.machine_command == 'import':
            imported = import_ssh(catalog, options.config, options.select)
            return dict(ok=True, imported=[m.id for m in imported], machines=[m.public() for m in catalog.list()])
        return dict(ok=True, machines=[m.public() for m in catalog.list()])
    if options.command == 'doctor':
        from port_forward_tui.diagnostics import app_checks, report
        return report(app_checks(options.data_dir))
    if options.machine:
        options.data_dir = catalog.get(options.machine).directory
    else:
        # Preserve emergency stop with an unreadable legacy favorites file.
        try:
            machines = catalog.list()
        except (OSError, ValueError, KeyError, TypeError):
            if options.command != 'stop-all' or not (options.data_dir / 'endpoint.json').exists():
                raise
            machines = []
        if len(machines) == 1:
            options.data_dir = machines[0].directory
        elif len(machines) > 1:
            if options.command == 'list':
                summaries = [dict(machine=m.public(), **listing(read_store(m.directory))) for m in machines]
                return dict(ok=True, machines=summaries,
                            forwards=[dict(rule, machine_id=item['machine']['id'], machine_name=item['machine']['name'])
                                      for item in summaries for rule in item['forwards']])
            raise UsageError('Several machines are saved. Add --machine ID; run machines list to choose one.')
    if options.command == 'stop-all':
        from port_forward_tui.background import exchange
        snapshot = exchange(options.data_dir, 'stop_all')
        return dict(ok=True, host=snapshot['host'], background='connected',
                    forwards=[dict(r, state=snapshot['states'].get(r['id'], 'OFF'),
                                   url=f"http://127.0.0.1:{r['local_port']}") for r in snapshot.get('forwards', [])])
    store = read_store(options.data_dir)
    if options.command == 'list':
        return dict(ok=True, **listing(store))
    if not store.host:
        raise ValueError('Add a machine first: .\\ports.ps1 machines add YOUR_SSH_NAME. Installation does not require a host.')
    if not store.keep_alive:
        raise ValueError('These commands use background mode. This data folder is set to foreground mode; use its TUI.')
    if options.command == 'restart-manager':
        from port_forward_tui.background import restart_daemon
        restored = restart_daemon(store.directory)
        return dict(ok=True, **listing(store), restored_ids=restored)
    if options.command in ('start', 'stop', 'delete'):
        rule = next((r for r in store.forwards if r.id == options.id), None)
        if rule is None:
            raise ValueError('Favorite ID not found. Run list and use the exact id of the connection you want.')
    from port_forward_tui.background import DaemonClient
    manager = DaemonClient(store.host, store.directory)
    # All writes go through the same server as open screens, preserving concurrent edits.
    if options.command == 'save':
        local = options.local if options.local is not None else options.remote
        previous = next((r for r in manager.forwards if (r.local_port, r.remote_port) == (local, options.remote)), None)
        rule = (replace(previous, name=options.name.strip() or previous.name) if previous
                else Forward.make(local, options.remote, options.name))
        rule = manager.upsert(rule, expected=previous)
    elif options.command == 'start':
        manager.start(rule)
        deadline = time.monotonic() + options.wait
        while manager.status(rule.id) == 'CONNECTING' and time.monotonic() < deadline:
            time.sleep(.1)
            manager.poll()
        if manager.status(rule.id) == 'ERROR':
            raise OSError(manager.details(rule.id) or 'SSH could not open this connection.')
    elif options.command == 'stop':
        manager.stop(rule.id)
    elif options.command == 'delete':
        manager.delete(rule)
    result = dict(ok=True, **listing(store))
    if options.command != 'stop-all':
        result['id'] = rule.id
    return result


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    as_json = '--json' in argv
    try:
        options = arguments(argv)
        result = execute(options)
        result.update(schema_version=1, command=options.command)
        if options.command == 'doctor':
            from port_forward_tui.diagnostics import print_report
            print_report(result, as_json)
        elif as_json:
            print(json.dumps(result, ensure_ascii=True))
        elif options.command == 'machines':
            for alias in result.get('aliases', []):
                print(alias)
            for machine in result.get('machines', []):
                print(f"{machine['name']} | {machine['target']} | id {machine['id']}")
            if not result.get('machines') and not result.get('aliases'):
                print('No machines yet. Use machines add YOUR_SSH_NAME or machines import.')
        else:
            if result.get('warning'):
                print(result['warning'])
            if not result.get('forwards'):
                print('No saved connections. Add a machine, then save --remote 8000 --name "My app".')
            for rule in result.get('forwards', []):
                print(f"{rule.get('machine_name', '')} {rule['state']:10} {rule['name']} | this PC {rule['local_port']} -> remote {rule['remote_port']} | id {rule['id']}")
            if result.get('id'):
                print('Favorite ID: ' + result['id'])
            print('ON means a local listener exists; the remote app must also be running. UNKNOWN means live status was not observed.')
        return 0 if result['ok'] else 1
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        code = 'invalid_arguments' if isinstance(error, UsageError) else 'operation_failed'
        result = dict(schema_version=1, ok=False, error=dict(code=code, message=str(error)))
        if as_json:
            print(json.dumps(result, ensure_ascii=True))
        else:
            print('Could not complete that command: ' + str(error), file=sys.stderr)
        return 2 if isinstance(error, UsageError) else 1


if __name__ == '__main__':
    raise SystemExit(main())
