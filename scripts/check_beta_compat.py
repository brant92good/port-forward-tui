"""Windows: actual stable/legacy readers and controllers reject isolated beta.

Developer qualification only. Every controller owns a fresh test directory;
commands never start SSH. No personal data, live controller or browser is used.
"""
import argparse
from dataclasses import asdict
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from port_forward_tui.forwarding import Forward, Store
from port_forward_tui.background import exchange as legacy_exchange

FLAGS = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0


def invoke(binary, directory, *args):
    result = subprocess.run([str(binary), '--data-dir', str(directory), '--json', *args],
                            capture_output=True, text=True, encoding='utf-8', timeout=12, creationflags=FLAGS)
    return result.returncode, json.loads(result.stdout)


def request(directory, command, protocol=None, **kwargs):
    endpoint = json.loads((directory/'endpoint.json').read_text())
    payload = dict(token=endpoint['token'], protocol=endpoint['protocol'] if protocol is None else protocol,
                   command=command, **kwargs)
    with socket.create_connection(('127.0.0.1', endpoint['port']), timeout=2) as connection:
        connection.settimeout(2)
        connection.sendall(json.dumps(payload).encode()+b'\n')
        with connection.makefile('rb') as reader:
            return json.loads(reader.readline(2*1024*1024))


class Controller:
    def __init__(self, command, directory):
        self.directory = directory
        self.log = (directory/'test-controller.log').open('wb')
        self.child = subprocess.Popen([*map(str, command), '--serve', '--data-dir', str(directory)],
                                     stdin=subprocess.DEVNULL, stdout=self.log, stderr=self.log,
                                     creationflags=FLAGS)
        try:
            deadline = time.monotonic()+10
            while time.monotonic() < deadline:
                assert self.child.poll() is None, 'Owned controller exited during startup'
                try:
                    if request(directory, 'status').get('ok'):
                        return
                except (OSError, ValueError):
                    pass
                time.sleep(.05)
            raise AssertionError('Owned controller startup deadline')
        except BaseException:
            self.close()
            raise

    def close(self):
        try:
            if self.child.poll() is None:
                request(self.directory, 'shutdown')
                self.child.wait(timeout=5)
        finally:
            if self.child.poll() is None:
                self.child.kill()
                self.child.wait(timeout=5)
            self.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--stable-binary', type=Path)
    options = parser.parse_args()
    if os.name != 'nt':
        parser.error('Historical controller qualification runs on Windows')
    binary = options.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='ports-beta-compat-') as temporary:
        root = Path(temporary)
        stable_binary = options.stable_binary
        if stable_binary is None:
            installation = root/'stable-install'
            subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
                            str(ROOT/'install.ps1'), '-Version', '0.9.1', '-InstallDir', str(installation), '-NoPath'],
                           check=True, timeout=120, creationflags=FLAGS)
            stable_binary = installation/'bin'/'ports.exe'
        stable_binary = stable_binary.resolve(strict=True)
        old_rule = Forward.make(18000, 8000, 'Original')
        old = dict(version=1, host='isolated-no-ssh', keep_alive=True, forwards=[asdict(old_rule)])

        # Verify real readers, including the released native executable, rather
        # than merely reimplementing their expected schema validation.
        schema = root/'schema'; schema.mkdir()
        raw = dict(old, version=2, forwards=[dict(id='a'*32,name='Proxy',kind='socks',local_port=1080)])
        path = schema/'forwards.json'; path.write_text(json.dumps(raw), encoding='utf-8')
        before = path.read_bytes()
        assert invoke(stable_binary, schema, 'list')[0] != 0
        try:
            Store(schema).load()
        except (ValueError, TypeError):
            pass
        else:
            raise AssertionError('Legacy store accepted SOCKS schema2')
        assert path.read_bytes() == before
        print('PASS: released stable and legacy readers reject real SOCKS schema2 without rewriting it', flush=True)

        for kind, command in [('stable', [stable_binary]), ('legacy', [sys.executable,'-E','-s', ROOT/'port_forward_tui/background.py'])]:
            directory=root/kind; directory.mkdir()
            (directory/'forwards.json').write_text(json.dumps(old), encoding='utf-8')
            controller=Controller(command,directory)
            try:
                initial=request(directory,'status'); before=(directory/'forwards.json').read_bytes()
                # Normally beta rejects unmarked stable data even before IPC.
                assert invoke(binary,directory,'save','--socks')[0] != 0
                (directory/'.ports-channel').write_bytes(b'beta\n')
                for action,args in [('save',['--socks']),('delete',[old_rule.id,'--yes']),('stop',[old_rule.id]),('stop-all',[])]:
                    assert invoke(binary,directory,action,*args)[0] != 0
                # Explicit raw protocol2 must be rejected at old server dispatch.
                for action in ['upsert','delete','status','stop','stop_all','shutdown']:
                    response=request(directory,action,protocol=2,rule=asdict(old_rule),rule_id=old_rule.id,expected=asdict(old_rule))
                    assert response['ok'] is False,(kind,action,response)
                assert controller.child.poll() is None
                assert request(directory,'status')['pid']==initial['pid']
                assert (directory/'forwards.json').read_bytes()==before
                assert not (directory/'port_forward_tui.background.log').exists(), 'Beta launched fallback helper'
                print(f'PASS: beta/old protocol2 mutations refused by actual {kind} controller, same PID/bytes and no fallback',flush=True)
            finally:
                controller.close()

        directory=root/'beta'; directory.mkdir()
        (directory/'.ports-channel').write_bytes(b'beta\n')
        (directory/'forwards.json').write_text(json.dumps(old), encoding='utf-8')
        controller=Controller([binary],directory)
        try:
            code,result=invoke(binary,directory,'save','--socks','--name','Proxy')
            assert code==0,result
            before=(directory/'forwards.json').read_bytes()
            for action in ['upsert','delete','status','stop','stop_all','shutdown']:
                response=request(directory,action,protocol=1,rule=asdict(old_rule),rule_id=old_rule.id,expected=asdict(old_rule))
                assert response['ok'] is False,(action,response)
            try:
                legacy_exchange(directory,'status',timeout=.3)
            except (OSError,ValueError):
                pass
            else:
                raise AssertionError('Legacy client accepted beta endpoint')
            assert invoke(stable_binary,directory,'list')[0]!=0
            assert (directory/'forwards.json').read_bytes()==before
            assert not request(directory,'status')['running']
            print('PASS: beta rejects all protocol1 mutations; old native/legacy clients refuse and OFF saves create no SSH',flush=True)
        finally:
            controller.close()
        print('PASS: all owned controllers exited normally',flush=True)


if __name__ == '__main__':
    main()
