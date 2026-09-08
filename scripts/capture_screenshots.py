"""Capture the real Textual UI with simulated example data, without SSH."""
import asyncio
import os
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from app import PortApp
from textual import events
from forwarding import Forward, Store


class DemoConnections:
    persistent = True

    def __init__(self, rules):
        self.running = {r.id: r for r in rules[:2]}

    def status(self, key):
        return 'ON' if key in self.running else 'OFF'

    def poll(self):
        pass

    def details(self, key):
        return ''


def capture(app, path, title):
    svg = app.export_screenshot(title=title)
    path.write_text('\n'.join(line.rstrip() for line in svg.splitlines()) + '\n', encoding='utf-8')


async def main():
    # Documentation shows the normal color theme, even when CI requests monochrome logs.
    os.environ.pop('NO_COLOR', None)
    output = ROOT / 'docs/screenshots'
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='ports-screenshot-') as folder:
        store = Store(Path(folder))
        store.host = 'demo-server'
        store.forwards = [Forward.make(8000, 8000, 'My web app'),
                          Forward.make(18888, 8888, 'Jupyter notebook'),
                          Forward.make(3000, 3000, 'Project preview')]
        app = PortApp(store, DemoConnections(store.forwards))
        async with app.run_test(size=(104, 28)) as pilot:
            app.post_message(events.AppFocus())
            await pilot.press('down', 'up')
            await pilot.pause()
            capture(app, output / 'connections.svg', 'Port Forward TUI - example data')
            await pilot.resize_terminal(104, 34)
            await pilot.press('a')
            await pilot.pause()
            capture(app, output / 'add-connection.svg', 'Add a connection - example data')
            await pilot.press('escape')
            await pilot.resize_terminal(104, 28)
            await pilot.press('f2')
            await pilot.pause()
            capture(app, output / 'settings.svg', 'Choose where shortcuts return')
    print('Captured actual UI screens with simulated example data in docs/screenshots.')


if __name__ == '__main__':
    asyncio.run(main())
