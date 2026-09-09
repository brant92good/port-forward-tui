"""Exercise a compiled bundle installer in owned directories (developer check).

No connection is started. --with-path is for disposable CI runners only.
--release exercises the real HTTPS installer and release downloads, without overrides.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--bundle', type=Path)
    mode.add_argument('--release', action='store_true')
    parser.add_argument('--version', default='0.7.1')
    parser.add_argument('--ref')
    parser.add_argument('--with-path', action='store_true')
    options = parser.parse_args()
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?', options.version):
        parser.error('Invalid release version')
    ref = options.ref or 'v'+options.version
    if not re.fullmatch(r'[A-Za-z0-9._/-]+', ref):
        parser.error('Invalid source reference')
    source = Path(__file__).resolve().parents[1]
    bundle = options.bundle.resolve(strict=True) if options.bundle else None
    digest = hashlib.sha256(bundle.read_bytes()).hexdigest() if bundle else None
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
    with tempfile.TemporaryDirectory(prefix='ports-install-') as temporary:
        root = Path(temporary)/"space \u6e2c\u8a66 and ' quote"
        root.mkdir()
        installed, home, data = root/'app', root/'home', root/'data'
        home.mkdir()
        for profile in ('.bashrc','.bash_login','.profile'):
            (home/profile).write_text('# existing '+profile+'\n')
        def install(expected=digest, destination=installed, success=True):
            env = dict(os.environ, PORTS_INSTALL_DIR=str(destination), PORTS_VERSION=options.version,
                       PORTS_NO_PATH='0' if options.with_path else '1', HOME=str(home), SHELL='/bin/bash',
                       PYTHONHOME=str(root/'missing-python'), PYTHONPATH=str(root/'shadow'),
                       CONDA_PREFIX=str(root/'missing-conda'), VIRTUAL_ENV=str(root/'missing-venv'))
            env.pop('PORTS_BUNDLE',None)
            env.pop('PORTS_SHA256',None)
            if bundle:
                env['PORTS_BUNDLE'] = str(bundle)
            if expected:
                env['PORTS_SHA256'] = expected
            if options.release:
                extension = 'ps1' if os.name == 'nt' else 'sh'
                url = f'https://raw.githubusercontent.com/brant92good/port-forward-tui/{ref}/install.{extension}'
                command = (['powershell.exe','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-Command',f'irm {url} | iex']
                           if os.name == 'nt' else ['sh','-c',f'curl -fsSL {url} | sh'])
            elif os.name == 'nt':
                command = ['powershell.exe','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',str(source/'install.ps1')]
            else:
                command = ['sh',str(source/'install.sh')]
            result = subprocess.run(command,env=env,capture_output=True,encoding='utf-8',errors='replace',timeout=180 if options.release else 45,creationflags=flags)
            assert (result.returncode == 0) == success, (result.stdout,result.stderr)
        executable = installed/('bin/ports.exe' if os.name == 'nt' else 'bin/ports')
        def run(*arguments):
            env = dict(os.environ,PYTHONHOME=str(root/'missing-python'),PYTHONPATH=str(root/'shadow'),CONDA_PREFIX=str(root/'missing-conda'),VIRTUAL_ENV=str(root/'missing-venv'))
            result = subprocess.run([str(executable),*arguments],env=env,cwd=root,capture_output=True,text=True,encoding='utf-8',timeout=15,creationflags=flags)
            assert result.returncode == 0, (result.stdout,result.stderr)
            return result.stdout
        install()
        assert run('--version').strip() == 'ports '+options.version
        created = json.loads(run('--data-dir',str(data),'machines','add','demo.invalid','--name','Demo','--json'))
        identifier = created['machine']['id']
        machine = json.loads(run('--data-dir',str(data),'--machine',identifier,'machines','pick','--no-window-context','--json'))['machine']
        store = Path(machine['directory'])/'forwards.json'
        saved = json.loads(store.read_text(encoding='utf-8'))
        saved['forwards'][0]['name'] = 'Keep my saved connection'
        store.write_text(json.dumps(saved),encoding='utf-8')
        before = store.read_bytes()
        listing = json.loads(run('--data-dir',str(data),'list','--json'))
        (installed/'notes.txt').write_text('user data')
        install()
        assert store.read_bytes() == before
        assert json.loads(run('--data-dir',str(data),'list','--json')) == listing
        assert (installed/'notes.txt').read_text() == 'user data'
        binary_hash = hashlib.sha256(executable.read_bytes()).hexdigest()
        install(expected='0'*64,success=False)
        assert hashlib.sha256(executable.read_bytes()).hexdigest() == binary_hash
        assert store.read_bytes() == before
        unowned = root/'not-owned'; unowned.mkdir(); (unowned/'keep.txt').write_text('other project')
        install(destination=unowned,success=False)
        assert (unowned/'keep.txt').read_text() == 'other project'
        assert not (installed/'python').exists() and not (installed/'uv').exists()
        if options.with_path:
            if os.name == 'nt':
                import winreg
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER,'Environment') as key:
                    user_path = winreg.QueryValueEx(key,'Path')[0]
                assert sum(Path(p).resolve() == (installed/'bin').resolve() for p in user_path.split(';') if p) == 1
            else:
                env = dict(os.environ,HOME=str(home),PATH='/usr/bin:/bin')
                for mode in (['-lc'],['--noprofile','-ic']):
                    result = subprocess.run(['/bin/bash',*mode,'command -v ports'],env=env,capture_output=True,text=True,timeout=10)
                    assert result.returncode == 0 and result.stdout.strip() == str(executable), (result.stdout,result.stderr)
                assert not (home/'.bash_profile').exists()
                assert (home/'.profile').read_text() == '# existing .profile\n'
                assert (home/'.bashrc').read_text().count('# ports') == 1
                assert (home/'.bash_login').read_text().count('# ports') == 1
        assert not list(data.rglob('endpoint.json')), 'Installer verification must not start controllers'
        print(json.dumps({'ok':True,'mode':'HTTPS release' if options.release else 'local bundle','version':options.version,'sha256':binary_hash,
                          'checks':['fresh install','update','checksum rejection','directory ownership','saved favorites','Unicode/quoted paths','polluted environment','PATH' if options.with_path else 'PATH unchanged']}))


if __name__ == '__main__':
    main()
