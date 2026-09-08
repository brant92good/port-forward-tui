"""Local setup checks. Never install packages, create settings, or contact SSH."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys

from forwarding import DATA_DIR, SSH, Store

ROOT = Path(__file__).resolve().parent


def check(key, passed, message, fix='', *, warning=False):
    return dict(id=key, status='ok' if passed else ('warning' if warning else 'error'),
                message=message, next_step='' if passed else fix)


def report(checks):
    return dict(schema_version=1, ok=not any(c['status'] == 'error' for c in checks), checks=checks)


def app_checks(directory=DATA_DIR, root=ROOT):
    checks = [check('windows', sys.platform == 'win32', 'Windows is required for this app.', 'Use Windows 10 or 11.'),
              check('python', sys.version_info >= (3, 12), 'Python 3.12 or newer is required.',
                    'Install Windows Python 3.12+; setup also accepts -Python C:\\path\\python.exe.'),
              check('ssh', Path(SSH).is_file(), 'Windows OpenSSH client is available.',
                    'Install OpenSSH Client in Windows Settings > Optional features.'),
              check('terminal', bool(shutil.which('wt.exe')), 'Windows Terminal is available.',
                    'Install Windows Terminal from Microsoft Store.'),
              check('environment', (root / '.venv/Scripts/python.exe').is_file(), 'The private app environment exists.',
                    'Run .\\install.ps1 to prepare this checkout.'),
              check('textual', importlib.util.find_spec('textual') is not None, 'The screen interface dependency is available.',
                    'Run .\\install.ps1 to install the app dependencies.')]
    try:
        import ssl
        available = bool(ssl.OPENSSL_VERSION)
    except ImportError:
        available = False
    checks.append(check('ssl', available, 'Python can load SSL for package downloads.',
                        'Select a working Python with install.ps1 -Python C:\\path\\python.exe.'))
    compiler = Path(os.environ.get('SystemRoot', r'C:\Windows')) / 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    checks.append(check('focus_helper', compiler.is_file(), 'The Windows shortcut-helper compiler is available.',
                        'The installer needs the Windows .NET Framework compiler; see README troubleshooting.'))
    store = Store(directory)
    try:
        if store.path.exists():
            store.load()
        checks.append(check('saved_target', bool(store.host), 'A remote computer is selected.',
                            'Run .\\install.ps1 -HostName workbox, replacing workbox with your SSH name.'))
    except (OSError, ValueError, KeyError, TypeError):
        checks.append(check('saved_settings', False, 'The saved connections file could not be read.',
                            'Keep a backup of forwards.json and repair its JSON; do not delete your favorites.'))
    checks.append(check('ssh_login', False, 'Remote login and the remote app have not been tested.',
                        'Run ssh YOUR_SSH_NAME yourself. Background connections need an SSH key or ssh-agent.', warning=True))
    return checks


def print_report(value, as_json=False):
    if as_json:
        print(json.dumps(value, ensure_ascii=True))
        return
    print('Setup check: ' + ('ready for the checks below' if value['ok'] else 'some steps need attention'))
    for item in value['checks']:
        print(f"[{item['status'].upper()}] {item['message']}")
        if item['next_step']:
            print('  Next: ' + item['next_step'])
    print('These are local checks. No settings were changed and no SSH connection was opened.')
