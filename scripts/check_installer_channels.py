"""Inert installer fixtures: stable preservation, explicit beta and rollback.

No network, SSH, GUI, live install or persistent PATH changes.
"""
import argparse
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile
import tarfile

ROOT = Path(__file__).resolve().parents[1]
FLAGS = getattr(subprocess, 'CREATE_NO_WINDOW', 0)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--powershell', choices=('powershell.exe','pwsh.exe'), default='powershell.exe')
    options = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='ports-channels-') as temp:
        root = Path(temp).resolve()/"space \u958b\u767c and ' quote"
        root.mkdir()
        home, local = root/'home', root/'local'
        home.mkdir(); local.mkdir()
        initial_path = os.environ.get('PATH', '')
        registry_path = None
        if os.name == 'nt':
            import winreg
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, 'Environment') as key:
                registry_path = winreg.QueryValueEx(key, 'Path')[0]
        archives = {}
        for version in ('0.9.1', '0.10.0-beta.1', '0.10.0-beta.2'):
            if os.name == 'nt':
                cs, exe = root/(version+'.cs'), root/(version+'.exe')
                cs.write_text('class P { static void Main() { System.Console.WriteLine("ports '+version+'"); } }')
                compiler = Path(os.environ['WINDIR'])/'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
                subprocess.run([str(compiler), '/nologo', '/out:'+str(exe), str(cs)], check=True,
                               capture_output=True, timeout=30, creationflags=FLAGS)
                files = {'ports.exe': exe.read_bytes(), 'PortsFocus.exe': b'fixture', 'TerminalViews.exe': b'fixture'}
            else:
                files = {'ports': ('#!/bin/sh\nprintf "ports '+version+'\\n"\n').encode()}
            files.update({'LICENSE.txt': b'fixture license\n', 'THIRD_PARTY_NOTICES.txt': b'fixture notices\n'})
            files['SHA256SUMS'] = ''.join(hashlib.sha256(data).hexdigest()+'  '+name+'\n' for name, data in files.items()).encode()
            archive = root/(version+('.zip' if os.name == 'nt' else '.tar.gz'))
            if os.name == 'nt':
                with zipfile.ZipFile(archive, 'w') as output:
                    for name, data in files.items():
                        output.writestr(name, data)
            else:
                with tarfile.open(archive, 'w:gz') as output:
                    for name, data in files.items():
                        entry = tarfile.TarInfo(name); entry.size = len(data); entry.mode = 0o755 if name == 'ports' else 0o644
                        output.addfile(entry, io.BytesIO(data))
            archives[version] = archive
        count = 0

        def install(version, channel, destination, success=True, *, pin=None, bad_hash=False, path=False):
            nonlocal count
            archive = archives[version]
            env = dict(os.environ, HOME=str(home), LOCALAPPDATA=str(local), XDG_DATA_HOME=str(home/'data'),
                       PORTS_BUNDLE=str(archive), PORTS_SHA256='0'*64 if bad_hash else digest(archive),
                       PORTS_NO_PATH='0' if path else '1')
            env.pop('PORTS_CHANNEL', None); env.pop('PORTS_VERSION', None); env.pop('PORTS_INSTALL_DIR', None)
            if pin is not False:
                env['PORTS_VERSION'] = version if pin is None else pin
            if destination is not None:
                env['PORTS_INSTALL_DIR'] = str(destination)
            if os.name == 'nt':
                command = [options.powershell, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', str(ROOT/'install.ps1')]
                if channel is not None:
                    command += ['-Channel', channel]
            else:
                command = ['sh', str(ROOT/'install.sh')]
                if channel is not None:
                    env['PORTS_CHANNEL'] = channel
            result = subprocess.run(command, env=env, capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=45, creationflags=FLAGS)
            assert (result.returncode == 0) == success, (version, channel, destination, result.stdout, result.stderr)
            count += 1

        stable, beta = root/'stable', root/'beta'
        install('0.9.1', None, stable, pin=False)
        stable_files = {str(p.relative_to(stable)): p.read_bytes() for p in stable.rglob('*') if p.is_file()}
        install('0.10.0-beta.1', 'beta', beta, path=True)
        beta_exe = beta/'bin'/('ports-beta.exe' if os.name == 'nt' else 'ports-beta')
        assert not (beta/'bin'/('ports.exe' if os.name == 'nt' else 'ports')).exists()
        assert (beta/'.ports-installer').read_text().strip() == 'port-forward-tui-beta'
        assert subprocess.check_output([str(beta_exe), '--version'], text=True, creationflags=FLAGS).strip() == 'ports 0.10.0-beta.1'
        (beta/'keep.txt').write_text('beta settings unchanged')
        install('0.10.0-beta.2', 'beta', beta)
        install('0.10.0-beta.1', 'beta', beta)  # explicit version rollback in this compatible inert fixture
        assert subprocess.check_output([str(beta_exe), '--version'], text=True, creationflags=FLAGS).strip() == 'ports 0.10.0-beta.1'
        assert (beta/'keep.txt').read_text() == 'beta settings unchanged'
        before = beta_exe.read_bytes()
        install('0.10.0-beta.2', 'beta', beta, False, bad_hash=True)
        install('0.10.0-beta.2', 'beta', beta, False, pin='0.10.0-beta.1')
        assert beta_exe.read_bytes() == before
        install('0.10.0-beta.1', None, root/'implicit-beta', False)
        install('0.10.0-beta.1', 'stable', root/'wrong-channel', False)
        install('0.9.1', 'beta', root/'wrong-beta', False)
        for bad in ('latest', 'v0.10.0-beta.1', '0.10.0-beta.0', '0.10.0-beta.01', '0.10.0-beta.1\n'):
            install('0.10.0-beta.1', 'beta', root/'invalid', False, pin=bad)
        install('0.10.0-beta.1', 'beta', stable, False)
        install('0.9.1', 'stable', beta, False)
        install('0.10.0-beta.1', 'beta', None, path=True)
        default_beta = local/'Programs/PortsBeta' if os.name == 'nt' else home/'data/ports-beta-install'
        assert (default_beta/'bin'/beta_exe.name).is_file()
        stable_default = local/'Programs/Ports' if os.name == 'nt' else home/'data/ports-install'
        install('0.10.0-beta.1', 'beta', stable_default, False)
        link = root/'linked-beta'
        if os.name == 'nt':
            subprocess.run(['powershell.exe','-NoProfile','-NonInteractive','-Command',
                            'New-Item -ItemType Junction -Path $env:PORTS_TEST_LINK -Target $env:PORTS_TEST_TARGET | Out-Null'],
                           env=dict(os.environ, PORTS_TEST_LINK=str(link), PORTS_TEST_TARGET=str(beta)), check=True, capture_output=True, timeout=15, creationflags=FLAGS)
        else:
            link.symlink_to(beta, target_is_directory=True)
        try:
            install('0.10.0-beta.1', 'beta', link, False)
        finally:
            os.rmdir(link) if os.name == 'nt' else link.unlink()
        linked_files = root/'linked-files'; linked_files.mkdir()
        (linked_files/'.ports-installer').write_text('port-forward-tui-beta')
        bin_link = linked_files/'bin'
        if os.name == 'nt':
            subprocess.run(['powershell.exe','-NoProfile','-NonInteractive','-Command',
                            'New-Item -ItemType Junction -Path $env:PORTS_TEST_LINK -Target $env:PORTS_TEST_TARGET | Out-Null'],
                           env=dict(os.environ, PORTS_TEST_LINK=str(bin_link), PORTS_TEST_TARGET=str(stable/'bin')), check=True, capture_output=True, timeout=15, creationflags=FLAGS)
        else:
            bin_link.symlink_to(stable/'bin', target_is_directory=True)
        try:
            install('0.10.0-beta.1', 'beta', linked_files, False)
        finally:
            os.rmdir(bin_link) if os.name == 'nt' else bin_link.unlink()
        assert stable_files == {str(p.relative_to(stable)): p.read_bytes() for p in stable.rglob('*') if p.is_file()}
        assert os.environ.get('PATH', '') == initial_path
        assert not list(home.glob('.*')) and not (beta/'env').exists()
        if os.name == 'nt':
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, 'Environment') as key:
                assert winreg.QueryValueEx(key, 'Path')[0] == registry_path
        print(f'PASS: {count} isolated channel installs/refusals; stable bytes/PATH preserved, distinct beta command, explicit update/rollback and alias rejection.')


if __name__ == '__main__':
    main()
