"""Exercise notice-aware installers with inert bundles in temporary directories.

No network, controller, SSH, or user PATH changes. Binaries only print a version.
"""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
FLAGS = getattr(subprocess, 'CREATE_NO_WINDOW', 0)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    with tempfile.TemporaryDirectory(prefix='ports-notices-') as temporary:
        root = Path(temporary) / "space 測試 and ' quote"
        root.mkdir()
        home = root / 'home'
        home.mkdir()
        binaries = {}
        for version in ('0.8.1', '0.9.0'):
            if os.name == 'nt':
                source, binary = root / (version+'.cs'), root / (version+'.exe')
                source.write_text('class P { static void Main() { System.Console.WriteLine("ports '+version+'"); } }', encoding='utf-8')
                compiler = Path(os.environ['WINDIR']) / 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
                subprocess.run([str(compiler), '/nologo', '/target:exe', '/out:'+str(binary), str(source)], check=True, capture_output=True, timeout=30, creationflags=FLAGS)
                binaries[version] = binary.read_bytes()
            else:
                binaries[version] = ('#!/bin/sh\nprintf \'ports '+version+'\\n\'\n').encode()

        def bundle(label, version, mode='full'):
            files = {('ports.exe' if os.name == 'nt' else 'ports'): binaries[version]}
            if os.name == 'nt':
                files.update({'PortsFocus.exe': b'fixture helper', 'TerminalViews.exe': b'fixture helper'})
            if mode != 'legacy':
                files['LICENSE.txt'] = b'Ports fixture license\n'
                if mode != 'partial':
                    files['THIRD_PARTY_NOTICES.txt'] = b'Fixture third-party notice\n'
            checksums = ''.join(f'{sha(data)}  {name}\n' for name, data in files.items()
                                if not (mode == 'missing-index' and name == 'THIRD_PARTY_NOTICES.txt')).encode()
            if mode == 'bad-notice':
                files['THIRD_PARTY_NOTICES.txt'] = b'tampered notice\n'
            if mode == 'unexpected':
                files['other.txt'] = b'not allowed'
            files['SHA256SUMS'] = checksums
            archive = root / (label + ('.zip' if os.name == 'nt' else '.tar.gz'))
            if os.name == 'nt':
                with zipfile.ZipFile(archive, 'w') as output:
                    for name, data in files.items():
                        output.writestr(name, data)
            else:
                with tarfile.open(archive, 'w:gz') as output:
                    for name, data in files.items():
                        entry = tarfile.TarInfo(name)
                        entry.size, entry.mode = len(data), 0o755 if name == 'ports' else 0o644
                        output.addfile(entry, io.BytesIO(data))
            return archive

        def install(archive, version, destination, success):
            env = dict(os.environ, PORTS_BUNDLE=str(archive), PORTS_SHA256=sha(archive.read_bytes()),
                       PORTS_VERSION=version, PORTS_INSTALL_DIR=str(destination), PORTS_NO_PATH='1', HOME=str(home))
            command = (['powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', str(ROOT/'install.ps1')]
                       if os.name == 'nt' else ['sh', str(ROOT/'install.sh')])
            result = subprocess.run(command, env=env, capture_output=True, encoding='utf-8', errors='replace', timeout=45, creationflags=FLAGS)
            assert (result.returncode == 0) == success, (archive.name, result.stdout, result.stderr)

        installed = root / 'new-install'
        full = bundle('full', '0.9.0')
        install(full, '0.9.0', installed, True)
        notices = {'LICENSE.txt': b'Ports fixture license\n', 'THIRD_PARTY_NOTICES.txt': b'Fixture third-party notice\n'}
        for name, data in notices.items():
            assert (installed/'bin'/name).read_bytes() == data
        note = installed / 'notes.txt'
        note.write_text('owner data', encoding='utf-8')
        before = {path.name: path.read_bytes() for path in (installed/'bin').iterdir() if path.is_file()}
        install(full, '0.9.0', installed, True)
        for mode in ('legacy', 'partial', 'bad-notice', 'missing-index', 'unexpected'):
            install(bundle(mode, '0.9.0', mode), '0.9.0', installed, False)
            assert note.read_text(encoding='utf-8') == 'owner data'
            for name, data in before.items():
                assert (installed/'bin'/name).read_bytes() == data, (mode, name)
        old = root / 'old-install'
        install(bundle('old', '0.8.1', 'legacy'), '0.8.1', old, True)
        assert not (old/'bin'/'THIRD_PARTY_NOTICES.txt').exists()
        assert list(home.iterdir()) == [], 'No shell startup files may be written'
        print('PASS: full notices fresh/update, five malformed bundles preserve installation, explicit 0.8.1 legacy bundle, no PATH/profile writes.')


if __name__ == '__main__':
    main()
