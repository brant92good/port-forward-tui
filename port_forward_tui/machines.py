"""Local machine catalog. SSH configuration is referenced, never copied or executed."""
from dataclasses import dataclass
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import shlex

from port_forward_tui.forwarding import DATA_DIR, Store, port, validate_host


def machine_id(target, ssh_port=None, ssh_config=None):
    identity = json.dumps([validate_host(target), ssh_port, ssh_config], ensure_ascii=True)
    return hashlib.sha256(identity.encode()).hexdigest()[:32]


@dataclass(frozen=True)
class Machine:
    id: str
    name: str
    target: str
    directory: Path
    ssh_port: int | None = None
    ssh_config: str | None = None

    def public(self):
        return dict(id=self.id, name=self.name, target=self.target,
                    ssh_port=self.ssh_port, ssh_config=self.ssh_config)


class Catalog:
    def __init__(self, root=DATA_DIR):
        self.root = Path(root)

    def list(self):
        result = []
        # Register the old data folder in place: no favorite IDs, files or
        # running controller endpoints are moved during migration.
        for directory in [self.root, *sorted((self.root / 'machines').glob('*'))]:
            store = Store(directory)
            if not store.path.is_file():
                continue
            store.load()
            if not store.host:
                continue
            key = machine_id(store.host, store.ssh_port, store.ssh_config)
            if directory != self.root and directory.name != key:
                raise ValueError('A machine destination was changed in its data file. Restore it and add a separate machine instead.')
            result.append(Machine(key, store.machine_name or store.host, store.host, directory,
                                  store.ssh_port, store.ssh_config))
        return result

    def get(self, selector):
        items = self.list()
        exact = next((m for m in items if m.id == selector), None)
        if exact:
            return exact
        matches = [m for m in items if m.target == selector or m.name == selector]
        if len(matches) != 1:
            raise ValueError('Machine not found or ambiguous. Run machines list and use its exact id.')
        return matches[0]

    def add(self, target, name='', ssh_port=None, ssh_config=None):
        target = validate_host(target.strip())
        name = name.strip()
        if len(name) > 80:
            raise ValueError('Use a machine name of 80 characters or fewer.')
        ssh_port = port(ssh_port) if ssh_port is not None else None
        if ssh_config:
            config = Path(ssh_config).expanduser().resolve()
            if not config.is_file():
                raise ValueError('SSH configuration file does not exist.')
            ssh_config = str(config)
        else:
            ssh_config = None
        key = machine_id(target, ssh_port, ssh_config)
        existing = next((m for m in self.list() if m.id == key), None)
        if existing:
            return existing
        directory = self.root / 'machines' / key
        directory.mkdir(parents=True, exist_ok=True)
        # Serialize first creation across simultaneous first-use windows.
        from port_forward_tui.forwarding import InstanceLock
        import time
        deadline = time.monotonic() + 3
        while True:
            try:
                lock = InstanceLock(directory, 'catalog.lock')
                break
            except RuntimeError:
                if time.monotonic() >= deadline:
                    raise ValueError('Another window is adding this machine. Try again.') from None
                time.sleep(.025)
        try:
            store = Store(directory)
            if store.path.exists():
                store.load()
            else:
                store.host, store.ssh_port, store.ssh_config = target, ssh_port, ssh_config
                store.machine_name = name or target
                store.load()  # Create starter favorites once, initially OFF.
        finally:
            lock.close()
        return self.get(key)


def ssh_aliases(config=None):
    """Discover literal Host names, including Includes, without ssh -G/Match exec.

    This is name discovery, not a replacement OpenSSH evaluator. Conditional
    includes may contribute names; OpenSSH evaluates their settings at login.
    """
    config = Path(config) if config else Path.home() / '.ssh/config'
    aliases, visited = set(), set()
    base = Path.home() / '.ssh'

    def visit(path, depth=0):
        path = path.expanduser().resolve()
        if path in visited or depth > 16:
            return
        if len(visited) >= 256:
            raise ValueError('Too many SSH Include files; import a smaller configuration.')
        visited.add(path)
        if path.stat().st_size > 2_000_000:
            raise ValueError('SSH configuration is too large to import.')
        for line in path.read_text(encoding='utf-8-sig').splitlines():
            line = re.sub(r'^\s*([A-Za-z]+)\s*=\s*', r'\1 ', line)
            lexer = shlex.shlex(line, posix=True)
            lexer.whitespace_split = True
            lexer.escape = ''  # Preserve Windows path backslashes.
            try:
                tokens = list(lexer)
            except ValueError:
                raise ValueError(f'Unclosed quote in SSH configuration: {path.name}') from None
            if not tokens:
                continue
            keyword, values = tokens[0].lower(), tokens[1:]
            if keyword == 'host':
                for value in values:
                    try:
                        aliases.add(validate_host(value))
                    except ValueError:
                        pass  # Wildcards and negations are patterns, not machines.
            elif keyword == 'include':
                for value in values:
                    if '%' in value:
                        continue  # Dynamic tokens cannot be resolved by name discovery.
                    candidate = Path(os.path.expandvars(value)).expanduser()
                    if not candidate.is_absolute():
                        candidate = base / candidate
                    for included in sorted(glob.glob(str(candidate))):
                        if Path(included).is_file():
                            visit(Path(included), depth + 1)
    visit(config)
    return sorted(aliases, key=str.casefold)


def import_ssh(catalog, config=None, selected=None):
    aliases = ssh_aliases(config)
    if selected is not None:
        if set(selected) - set(aliases):
            raise ValueError('A selected SSH name is no longer in this file. Read host names again.')
        aliases = [a for a in aliases if a in selected]
    default = (Path.home() / '.ssh/config').resolve()
    source = str(Path(config).expanduser().resolve()) if config and Path(config).expanduser().resolve() != default else None
    return [catalog.add(alias, ssh_config=source) for alias in aliases]
