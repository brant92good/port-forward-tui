"""Collect locked runtime/build dependency notices for the five binary targets.

Development tool only. It does not run build scripts or enter the installed app.
Use --check in CI to reject stale or incomplete checked-in notices.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-musl',
           'aarch64-unknown-linux-musl', 'aarch64-apple-darwin', 'x86_64-apple-darwin')
PREFIXES = ('license', 'licence', 'copying', 'notice', 'copyright', 'unlicense')


def fallback_files(package, directory=None):
    directory = directory or ROOT / 'docs/licenses/upstream'
    index = json.loads((directory/'sources.json').read_text(encoding='utf-8'))
    matches = [item for item in index['fallbacks']
               if (item['name'], item['version']) == (package['name'], package['version'])]
    if len(matches) != 1:
        raise ValueError(f"Review missing notices: {package['name']} {package['version']}")
    item = matches[0]
    crate = Path(package['manifest_path']).resolve(strict=True).parent
    revision = json.loads((crate/'.cargo_vcs_info.json').read_text(encoding='utf-8'))['git']['sha1']
    if revision != item['revision']:
        raise ValueError('Fallback source revision changed; review required')
    result = [('upstream-provenance.txt', item['reason']+'\nCrate source revision: '+revision+'\n')]
    for name in item['files']:
        path = (directory/name).resolve(strict=True)
        if not path.is_relative_to(directory.resolve(strict=True)) or not path.is_file():
            raise ValueError('Fallback path leaves notice directory')
        text = path.read_text(encoding='utf-8')
        document = index['documents'][name]
        if hashlib.sha256(text.encode('utf-8')).hexdigest() != document['sha256']:
            raise ValueError('Fallback document digest changed; review required')
        result.append(('upstream/'+name, 'Document: '+document['url']+'\n\n'+text))
    return result


def dependencies(metadata):
    """Keep normal/build edges; never walk a dependency reached only through dev."""
    packages = {package['id']: package for package in metadata['packages']}
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    root = metadata['resolve']['root']
    pending, visited, result = [root], set(), {}
    while pending:
        identifier = pending.pop()
        if identifier in visited:
            continue
        visited.add(identifier)
        if identifier != root:
            result[identifier] = packages[identifier]
        pending.extend(edge['pkg'] for edge in nodes[identifier]['deps']
                       if any(kind['kind'] != 'dev' for kind in edge['dep_kinds']))
    return result


def license_files(package):
    root = Path(package['manifest_path']).resolve(strict=True).parent
    files = {path for path in root.iterdir()
             if path.is_file() and path.name.lower().startswith(PREFIXES)}
    if package.get('license_file'):
        files.add(root / package['license_file'])
    if not files:
        return fallback_files(package)
    result = []
    for path in sorted(files, key=lambda path: path.name.casefold()):
        resolved = path.resolve(strict=True)
        if not resolved.is_relative_to(root) or not resolved.is_file():
            raise ValueError(f"License path leaves crate distribution: {package['name']}")
        if resolved.stat().st_size > 2 * 1024 * 1024:
            raise ValueError(f"Review oversized license: {package['name']}/{path.name}")
        text = resolved.read_text(encoding='utf-8')
        result.append((resolved.relative_to(root).as_posix(), text))
    return result


def render(packages):
    parts = ['Ports third-party notices\n\nGenerated from the locked runtime and build dependencies for the five release targets.\nDevelopment-only dependencies are excluded.\n']
    inventory = []
    for package in sorted(packages.values(), key=lambda item: (item['name'], item['version'], item['id'])):
        files = license_files(package)
        source = f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download"
        parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nDeclared license: {package.get('license') or 'See included license file'}\nSource: {source}\n")
        records = []
        for name, text in files:
            parts.append(f'\n--- {name} ---\n{text}')
            records.append({'file': name, 'sha256': hashlib.sha256(text.encode('utf-8')).hexdigest()})
        inventory.append({'name': package['name'], 'version': package['version'],
                          'license': package.get('license'), 'source': source, 'files': records})
    return '\n'.join(parts), json.dumps({'targets': TARGETS, 'dependencies': inventory}, indent=2, ensure_ascii=False) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    used = {}
    for target in TARGETS:
        command = ['cargo', '+1.94.0', 'metadata', '--locked', '--format-version', '1', '--filter-platform', target]
        if args.offline:
            command.append('--offline')
        metadata = json.loads(subprocess.check_output(command, cwd=ROOT, encoding='utf-8', timeout=120))
        used.update(dependencies(metadata))
    notice, inventory = render(used)
    directory = ROOT / 'docs' / 'licenses'
    for name, content in [('THIRD_PARTY_NOTICES.txt', notice), ('dependencies.json', inventory)]:
        path = directory / name
        if args.check:
            if not path.is_file() or path.read_text(encoding='utf-8') != content:
                raise ValueError(f'{name} is stale. Run scripts/collect_licenses.py and review the changes.')
        else:
            directory.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding='utf-8', newline='\n')
    print(f'{len(used)} locked dependency distributions: notices {"verified" if args.check else "collected"}.')


if __name__ == '__main__':
    main()
