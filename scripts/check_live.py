"""Opt-in live check: a real tunnel survives the client exiting and reconnecting."""
import argparse
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from port_forward_tui.background import DaemonClient, exchange
from port_forward_tui.forwarding import Forward, Store, listeners, validate_host


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True, help="An existing, trusted SSH config alias")
    options = parser.parse_args()
    validate_host(options.host)
    with tempfile.TemporaryDirectory() as folder:
        directory = Path(folder)
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            local_port = reservation.getsockname()[1]
        store = Store(directory)
        store.host = options.host
        rule = Forward.make(local_port, 22, "Temporary transport check")
        store.save([rule])
        client_code = """from pathlib import Path
import sys
from port_forward_tui.background import DaemonClient
from port_forward_tui.forwarding import Store
store = Store(Path(sys.argv[1])); store.load()
client = DaemonClient(store.host, store.directory)
client.start(store.forwards[0])
"""
        try:
            subprocess.run([sys._base_executable, "-c", client_code, folder], check=True,
                           cwd=ROOT, timeout=15,
                           creationflags=subprocess.CREATE_NO_WINDOW)
            client = DaemonClient(store.host, directory)
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                client.poll()
                if client.status(rule.id) in ("ON", "ERROR"):
                    break
                time.sleep(.1)
            if client.status(rule.id) != "ON":
                raise RuntimeError(client.details(rule.id) or "Tunnel did not become ready")
            with socket.create_connection(("127.0.0.1", local_port), timeout=5) as connection:
                banner = connection.recv(256)
            if not banner.startswith(b"SSH-2.0-"):
                raise RuntimeError("Expected an SSH banner from remote loopback port 22")
            print("PASS: original client exited; a reopened client found the same active tunnel.")
            print("PASS: real traffic crossed the tunnel after its original client process was gone.")
            client.stop(rule.id)
            if any(number == local_port for _, number in listeners()):
                raise RuntimeError("Local tunnel listener was not removed by explicit stop")
            print("PASS: explicit stop closed the tunnel and released its local port.")
        finally:
            try:
                exchange(directory, "shutdown")
            except (OSError, ValueError):
                pass
            deadline = time.monotonic() + 5
            while (directory / "endpoint.json").exists() and time.monotonic() < deadline:
                time.sleep(.1)


if __name__ == "__main__":
    main()
