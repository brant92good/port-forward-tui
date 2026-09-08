"""Lightweight CLI; load the TUI only when a new view is needed."""
import argparse
from pathlib import Path
import sys

from forwarding import DATA_DIR, InstanceLock, Store, TunnelManager, validate_host


def main(app_factory=None):
    parser = argparse.ArgumentParser(description="Keyboard port-forward manager with persistent background tunnels")
    parser.add_argument("--data-dir", type=Path, default=DATA_DIR)
    parser.add_argument("--host", help="SSH config alias or user@hostname; saved for future launches")
    parser.add_argument("--foreground", action="store_true", help="Stop tunnels when the UI closes (background is the default)")
    parser.add_argument("--focus-existing", action="store_true", help="Focus an existing live Terminal view if possible; otherwise open a new view")
    parser.add_argument("--stop-all", action="store_true", help="Stop background tunnels without opening the UI")
    parser.add_argument("--stop-daemon", action="store_true", help="Stop background tunnels and their supervisor")
    parser.add_argument("--check", action="store_true", help="Validate saved settings without opening tunnels")
    options = parser.parse_args()
    lock = manager = daemon_lock = registration = None
    try:
        if options.stop_all or options.stop_daemon:
            from background import exchange
            exchange(options.data_dir, "shutdown" if options.stop_daemon else "stop_all")
            print("Background tunnels stopped.")
            return 0
        store = Store(options.data_dir)
        store.load()
        if options.host:
            validate_host(options.host)
            if store.host != options.host:
                from background import exchange
                try:
                    existing = exchange(options.data_dir, "status")
                except (OSError, ValueError, KeyError):
                    existing = None
                if existing:
                    raise ValueError("Stop the background manager with --stop-daemon before changing hosts.")
                store.host = options.host
                store.save(store.forwards)
        if options.check:
            print(f"OK: {store.host}; {len(store.forwards)} saved forwards; {store.path}")
            return 0
        if not store.host:
            raise ValueError("Choose an SSH target on first launch: app.py --host YOUR_SSH_ALIAS")
        if options.focus_existing and not options.foreground:
            from views import focus_existing
            if focus_existing(options.data_dir):
                return 0
        if store.keep_alive and not options.foreground:
            from background import DaemonClient
            manager = DaemonClient(store.host, options.data_dir)
        else:
            lock = InstanceLock(options.data_dir)
            daemon_lock = InstanceLock(options.data_dir, "daemon.lock")
            manager = TunnelManager(store.host, options.data_dir)
        if app_factory is None:
            from app import PortApp
            app_factory = PortApp
        application = app_factory(store, manager)
        if getattr(manager, "persistent", False):
            from views import ViewRegistration
            registration = ViewRegistration(options.data_dir, store.host)
            application.view_registration = registration
        application.run()
        return 0
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        print(f"Port manager: {error}", file=sys.stderr)
        print(f"Settings: {options.data_dir / 'forwards.json'}", file=sys.stderr)
        return 1
    finally:
        if registration:
            registration.close()
        if manager and not getattr(manager, "persistent", False):
            manager.close()
        if daemon_lock:
            daemon_lock.close()
        if lock:
            lock.close()


if __name__ == "__main__":
    raise SystemExit(main())
