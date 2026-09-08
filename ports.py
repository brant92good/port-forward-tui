"""Commands for people, scripts, and coding agents; no screen control needed."""
import argparse
from dataclasses import asdict, replace
import json
from pathlib import Path
import sys
import time

from forwarding import DATA_DIR, Forward, Store, port


class UsageError(ValueError):
    pass


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise UsageError(message)


def arguments(argv):
    shared = argparse.ArgumentParser(add_help=False)
    shared.add_argument('--json', action='store_true', default=argparse.SUPPRESS, help='Print one JSON result for scripts')
    shared.add_argument('--data-dir', type=Path, default=argparse.SUPPRESS, help='Use a separate saved-connections folder')
    parser = Parser(description='Save and control connections to apps on your remote computer.', parents=[shared])
    commands = parser.add_subparsers(dest='command', required=True, parser_class=Parser)
    for name, help_text in [('doctor', 'Check this computer without changing it or contacting SSH'),
                            ('list', 'Read saved connections and any available live status'),
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
    from background import exchange
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
    if options.command == 'doctor':
        from diagnostics import app_checks, report
        return report(app_checks(options.data_dir))
    if options.command == 'stop-all':
        from background import exchange
        snapshot = exchange(options.data_dir, 'stop_all')
        return dict(ok=True, host=snapshot['host'], background='connected',
                    forwards=[dict(r, state=snapshot['states'].get(r['id'], 'OFF'),
                                   url=f"http://127.0.0.1:{r['local_port']}") for r in snapshot.get('forwards', [])])
    store = read_store(options.data_dir)
    if options.command == 'list':
        return dict(ok=True, **listing(store))
    if not store.host:
        raise ValueError('Choose your remote computer first: .\\install.ps1 -HostName YOUR_SSH_NAME')
    if not store.keep_alive:
        raise ValueError('These commands use background mode. This data folder is set to foreground mode; use its TUI.')
    if options.command in ('start', 'stop', 'delete'):
        rule = next((r for r in store.forwards if r.id == options.id), None)
        if rule is None:
            raise ValueError('Favorite ID not found. Run list and use the exact id of the connection you want.')
    from background import DaemonClient
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
            from diagnostics import print_report
            print_report(result, as_json)
        elif as_json:
            print(json.dumps(result, ensure_ascii=True))
        else:
            if result.get('warning'):
                print(result['warning'])
            if not result.get('forwards'):
                print('No saved connections. Run install.ps1 first, then save --remote 8000 --name "My app".')
            for rule in result.get('forwards', []):
                print(f"{rule['state']:10} {rule['name']} | this PC {rule['local_port']} -> remote {rule['remote_port']} | id {rule['id']}")
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
