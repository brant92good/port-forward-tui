"""Capture the real Textual UI with simulated example data, without SSH."""
import asyncio
import base64
from html import escape
import os
from pathlib import Path
import re
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from port_forward_tui.ui import PortApp
from textual import events
from port_forward_tui.forwarding import Forward, Store


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
    # SVGs embedded through <img> cannot fetch external fonts. Embed the exact
    # font used by Rich so GitHub and offline previews use the same cell metrics.
    fonts = ROOT / 'docs/fonts'
    for weight, name in [(400, 'Regular'), (700, 'Bold')]:
        encoded = base64.b64encode((fonts / f'FiraCode-{name}.woff2').read_bytes()).decode('ascii')
        face = ('@font-face { font-family: "Fira Code"; '
                f'src: url("data:font/woff2;base64,{encoded}") format("woff2"); '
                f'font-style: normal; font-weight: {weight}; }}')
        svg = re.sub(r'@font-face\s*\{[^}]*font-weight:\s*' + str(weight) + r';[^}]*\}',
                     lambda match: face, svg, count=1)
    license_text = escape((fonts / 'LICENSE').read_text(encoding='utf-8'))
    svg = svg.replace('<!-- Generated with Rich https://www.textualize.io -->',
                      '<!-- Generated with Rich https://www.textualize.io -->\n'
                      f'    <metadata>Embedded Fira Code 6.2 font license:\n{license_text}</metadata>')
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
            await pilot.press('escape', 'a', '8', '0', '0', '0', 'tab', 'tab', *'My web app', 'tab')
            await pilot.pause()
            capture(app, output / 'add-connection.svg', 'Add form: Esc then A - example data')
            await pilot.press('escape')
            await pilot.resize_terminal(104, 28)
            await pilot.press('f2')
            await pilot.pause()
            capture(app, output / 'settings.svg', 'Choose where shortcuts return')
    print('Captured actual UI screens with simulated example data in docs/screenshots.')


if __name__ == '__main__':
    asyncio.run(main())
