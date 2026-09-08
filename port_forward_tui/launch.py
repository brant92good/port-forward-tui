"""Lightweight entry point; load a screen only when a new view is needed."""
import argparse
from pathlib import Path
import sys

from port_forward_tui.forwarding import DATA_DIR, InstanceLock, Store, TunnelManager
from port_forward_tui.machines import Catalog


def run_view(machine, catalog, foreground=False, app_factory=None):
    lock = manager = daemon_lock = registration = None
    try:
        store = Store(machine.directory)
        store.load()
        if store.keep_alive and not foreground:
            from port_forward_tui.background import DaemonClient
            manager = DaemonClient(store.host, store.directory)
        else:
            lock = InstanceLock(store.directory)
            daemon_lock = InstanceLock(store.directory, 'daemon.lock')
            manager = TunnelManager(store.host, store.directory)
        if app_factory is None:
            from port_forward_tui.ui import PortApp
            app_factory = PortApp
        application = app_factory(store, manager)
        if getattr(manager, 'persistent', False):
            from port_forward_tui.views import ViewRegistration
            registration = ViewRegistration(store.directory, store.host, catalog_root=catalog.root, machine=machine.id)
            application.view_registration = registration
        return application.run()
    finally:
        if registration:
            registration.close()
        if manager and not getattr(manager, 'persistent', False):
            manager.close()
        if daemon_lock:
            daemon_lock.close()
        if lock:
            lock.close()


def main(app_factory=None):
    parser = argparse.ArgumentParser(description='Keyboard port forwards for multiple machines; background connections are the default')
    parser.add_argument('--data-dir', type=Path, default=DATA_DIR, help='Machine catalog and saved connections folder')
    target = parser.add_mutually_exclusive_group()
    target.add_argument('--host', help='Add/use an SSH name without changing any existing destination')
    target.add_argument('--machine', help='Saved machine id or unambiguous name from ports.py machines list')
    parser.add_argument('--machines', action='store_true', help='Open the machine picker')
    parser.add_argument('--foreground', action='store_true', help='Stop this view\'s connections when it closes')
    parser.add_argument('--focus-existing', action='store_true', help='Return to a view for this machine, or open one')
    parser.add_argument('--stop-all', action='store_true', help='Stop the selected machine\'s connections')
    parser.add_argument('--stop-daemon', action='store_true', help='Stop the selected machine\'s background controller')
    parser.add_argument('--check', action='store_true', help='Validate setup without opening a screen or connections')
    options = parser.parse_args()
    try:
        catalog = Catalog(options.data_dir)
        selector = catalog.add(options.host).id if options.host else options.machine
        if options.stop_all or options.stop_daemon:
            from port_forward_tui.background import exchange
            directory = catalog.get(selector).directory if selector else options.data_dir
            if not selector and not (directory / 'endpoint.json').exists():
                items = catalog.list()
                if len(items) != 1:
                    raise ValueError('Select a machine with --machine. Other machines will keep running.')
                directory = items[0].directory
            exchange(directory, 'shutdown' if options.stop_daemon else 'stop_all')
            print('Selected machine\'s background tunnels stopped.')
            return 0
        if options.check:
            items = [catalog.get(selector)] if selector else catalog.list()
            if not items:
                print('OK: installation is ready. Add or import a machine when you open the app.')
            for machine in items:
                store = Store(machine.directory)
                store.load()
                print(f'OK: {machine.target}; {len(store.forwards)} saved forwards; {store.path}')
            return 0
        from port_forward_tui.window_context import choose_machine
        machine = choose_machine(catalog, selector, picker=options.machines)
        if machine and options.focus_existing and not options.foreground:
            from port_forward_tui.views import focus_existing
            if focus_existing(machine.directory):
                return 0
        while machine:
            result = run_view(machine, catalog, options.foreground, app_factory)
            if result != 'pick-machine':
                break
            machine = choose_machine(catalog, picker=True)
        return 0
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        print(f'Port manager: {error}', file=sys.stderr)
        print(f'Settings folder: {options.data_dir}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
