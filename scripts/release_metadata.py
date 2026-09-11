"""Validate immutable release identity and collect pre-publication qualification.

No Git, network, publication or installation is performed by this tool.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tomllib

TARGETS = ('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-musl',
           'aarch64-unknown-linux-musl', 'aarch64-apple-darwin', 'x86_64-apple-darwin')
BASE = r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)'


def channel(version):
    if re.fullmatch(BASE, version):
        return 'stable'
    if re.fullmatch(BASE+r'-beta\.[1-9][0-9]*', version):
        return 'beta'
    raise ValueError('Unsupported release version; use x.y.z or x.y.z-beta.N')


def identity(root, tag=None):
    version = tomllib.loads((root/'Cargo.toml').read_text())['package']['version']
    entries = tomllib.loads((root/'Cargo.lock').read_text())['package']
    locked = [item['version'] for item in entries if item['name'] == 'port-forward-tui']
    if locked != [version]:
        raise ValueError('Cargo.toml and Cargo.lock package versions disagree')
    result = {'version': version, 'channel': channel(version), 'tag': 'v'+version}
    if tag is not None and tag != result['tag']:
        raise ValueError('Tag must exactly match Cargo version')
    if not (root/'docs/releases'/f'{version}.md').is_file():
        raise ValueError('Exact release notes are required before qualification')
    return result


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def checks(target, release_channel):
    names = ['fmt', 'all-target-tests', 'clippy', 'notices', 'installer-channel-fixtures',
             'release-metadata-tests', 'release-build', 'local-bundle-install-update']
    if target.endswith('windows-msvc'):
        names += ['ps51-and-ps7-installers', 'standalone-capture', 'beta-protocol-rejection' if release_channel == 'beta' else 'legacy-controller-compatibility']
    if 'linux' in target:
        names += ['real-openssh-local-and-socks' if release_channel == 'beta' else 'real-openssh-local']
    return names


def write_new(path, value):
    # Never refresh an old receipt in place: a failed candidate gets new artifacts.
    with path.open('x', encoding='utf-8', newline='\n') as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write('\n')


def qualify(root, directory, source, target, tag=None):
    item = identity(root, tag)
    if target not in TARGETS or not re.fullmatch(r'[a-f0-9]{40}', source):
        raise ValueError('Exact source commit and supported target required')
    archive = f'ports-{target}'+('.zip' if target.endswith('windows-msvc') else '.tar.gz')
    digest = sha(directory/archive)
    if (directory/(archive+'.sha256')).read_text().strip() != f'{digest}  {archive}':
        raise ValueError('Archive sidecar does not match exact bytes and filename')
    item.update(schema_version=1, source_commit=source, target=target, result='pass',
                checks=checks(target, item['channel']),
                assets={name: sha(directory/name) for name in (archive, archive+'.sha256')})
    write_new(directory/f'qualification-{target}.json', item)


def record(root, directory, source, tag):
    item = identity(root, tag)
    if not re.fullmatch(r'[a-f0-9]{40}', source):
        raise ValueError('Exact source commit required')
    matrix = []
    expected = set()
    for target in TARGETS:
        name = f'qualification-{target}.json'
        proof = json.loads((directory/name).read_text())
        archive = f'ports-{target}'+('.zip' if target.endswith('windows-msvc') else '.tar.gz')
        if (any(proof.get(key) != value for key, value in item.items()) or
                proof.get('source_commit') != source or proof.get('target') != target or
                proof.get('result') != 'pass' or proof.get('schema_version') != 1 or
                proof.get('checks') != checks(target, item['channel']) or
                set(proof.get('assets', {})) != {archive, archive+'.sha256'}):
            raise ValueError('Qualification identity or gate matrix mismatch: '+target)
        for asset, digest in proof['assets'].items():
            if sha(directory/asset) != digest:
                raise ValueError('Changed qualified asset: '+asset)
        expected.update((name, archive, archive+'.sha256'))
        matrix.append(proof)
    if {path.name for path in directory.iterdir()} != expected:
        raise ValueError('Release directory has missing or unexpected payloads')
    item.update(schema_version=1, source_commit=source, publication_policy='prerelease; latest=false',
                artifacts={name: sha(directory/name) for name in sorted(expected)}, matrix=matrix,
                post_publication={'gate': 'actual HTTPS fresh/update on all five targets',
                                  'status_at_record_creation': 'required; not yet observed',
                                  'evidence': 'released-install jobs and their separate immutable receipts'},
                compatibility={'controller_protocol': 2 if item['channel'] == 'beta' else 1,
                               'shared_stable_controller': item['channel'] != 'beta'},
                stable_default='0.9.1', automatic_stable_promotion=False)
    write_new(directory/'release-record.json', item)
    index = ''.join(sha(path)+'  '+path.name+'\n' for path in sorted(directory.iterdir()))
    with (directory/'SHA256SUMS').open('x', encoding='ascii', newline='\n') as stream:
        stream.write(index)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--tag')
    parser.add_argument('--github-output', type=Path)
    parser.add_argument('--artifacts', type=Path, default=Path('dist'))
    parser.add_argument('--source', default=os.environ.get('GITHUB_SHA', ''))
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--qualify', choices=TARGETS)
    mode.add_argument('--record', action='store_true')
    args = parser.parse_args()
    value = identity(args.root, args.tag)
    if args.github_output:
        with args.github_output.open('a', encoding='utf-8') as stream:
            stream.write(f'version={value["version"]}\nchannel={value["channel"]}\n')
    if args.qualify:
        qualify(args.root, args.artifacts, args.source, args.qualify, args.tag)
    elif args.record:
        if args.tag is None:
            parser.error('--record requires the exact immutable --tag')
        record(args.root, args.artifacts, args.source, args.tag)
    print(json.dumps(value))


if __name__ == '__main__':
    main()
