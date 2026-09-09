"""Read-only CLI process timing, not a TUI, SSH or desktop-focus benchmark."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import statistics
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--native', type=Path, required=True)
parser.add_argument('--python', type=Path, required=True)
parser.add_argument('--source', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
native = args.native.resolve(strict=True)
with tempfile.TemporaryDirectory(prefix='ports-cli-benchmark-') as temporary:
    missing = Path(temporary)/'absent-data'
    commands = {
        'python': [str(args.python), '-E', '-s', str(args.source/'ports.py')],
        'rust': [str(native)],
    }
    for command in commands.values():
        command.extend(['--data-dir', str(missing), 'list', '--json'])
    samples = {name: [] for name in commands}
    def run(name):
        start = time.perf_counter_ns()
        result = subprocess.run(commands[name], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
        elapsed = (time.perf_counter_ns() - start)/1_000_000
        assert result.returncode == 0, (name, result.stderr)
        assert json.loads(result.stdout)['ok'] is True
        assert not missing.exists(), 'Read-only list created its data directory'
        return elapsed
    for _ in range(3):
        for name in commands:
            run(name)
    ordering = []
    randomizer = random.Random(20260910)
    for _ in range(40):
        block = list(commands)
        randomizer.shuffle(block)
        ordering.append(block)
        for name in block:
            samples[name].append(run(name))
    report = {
        'measured_at': time.strftime('%Y-%m-%dT%H:%M:%S%z'),
        'operation': 'process creation through exit: list --json, absent data directory',
        'conditions': 'Windows x64, warm cache, 3 warmups, 40 interleaved paired blocks; no shell profiles, TUI paint, SSH or focus measured',
        'python_version': subprocess.check_output([str(args.python), '--version'], text=True).strip(),
        'native_version': subprocess.check_output([str(native), '--version'], text=True).strip(),
        'native_sha256': hashlib.sha256(native.read_bytes()).hexdigest(),
        'python_ignores_environment': '-E -s, matching the installed historical launcher',
        'block_order': ordering,
        'results': {name: {
            'median_ms': round(statistics.median(values), 3),
            'p95_ms': round(sorted(values)[37], 3),
            'min_ms': round(min(values), 3),
            'max_ms': round(max(values), 3),
            'samples_ms': [round(value, 3) for value in values],
        } for name, values in samples.items()},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
    print(json.dumps({name: result['median_ms'] for name, result in report['results'].items()}))
