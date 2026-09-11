# Native CLI startup measurement

For the 0.9 buffered screen writer, see the separate [frame output measurement](rendering.md).
The older CLI comparison below does not measure that change.

On this Windows desktop, a read-only saved-state query fell from **92.760 ms to
17.929 ms median** after the Rust rewrite. That is 74.831 ms less, or about 81%.
It is not a measurement of a new TUI's first frame, a return shortcut, or SSH
connection time.

| Implementation | Median | 95th percentile | Range |
| --- | ---: | ---: | ---: |
| Historical Python CLI | 92.760 ms | 120.088 ms | 88.284–132.707 ms |
| Compiled Rust CLI | 17.929 ms | 21.757 ms | 16.383–21.982 ms |

Measured September 10, 2026. Each command lists an absent data directory as
JSON, without creating files or starting a controller. The stopwatch includes
process creation through exit. There are three warmups followed by 40 paired
blocks with randomized order; all measured samples are retained. This was an
ordinary working desktop, without CPU isolation.

The Python command uses the existing Windows Python 3.12.11 virtual environment,
`-E -s`, and `ports.py`. The Rust executable comes from the passing CI artifact
for source `3656b4ac0fe149c7000d4d1eefb8408114b59542`, before release publication.
Its full SHA256 and raw samples are in
[the measurement file](benchmarks/native-cli-windows.json).
The historical Python CLI files are unchanged from that branch's previous main.

This compares two complete implementations, including their startup and imports.
It does not establish that rewriting any Python program yields the same gain.
For this small command, avoiding interpreter startup and imports is useful.
Once an SSH forward is running, OpenSSH carries the traffic in either version;
this result is not a network-throughput improvement.

## What may still be worth optimizing

Windows return shortcuts also pay for Terminal creating/closing a launcher tab,
helper startup, accessibility enumeration, machine selection, and content focus.
Those costs need their own measurements. The parent retains a
[dated before/after report](https://github.com/brant92good/terminal-workspace/blob/main/docs/before-after.md)
and a separate
[launcher experiment](https://github.com/brant92good/terminal-workspace/blob/main/docs/language-experiment.md).
Do not subtract these CLI timings from those keyboard measurements.

Combining repeated window discovery and helper launches might reduce return
latency further, but a stale cached tab could send input to the wrong machine.
Any such change needs identity, multiple-window and focus-cancellation checks.

CPU speed, storage, antivirus scanning, cold caches, tab count and accessibility
providers can change local startup/focus costs. Shell profiles and Conda hooks
matter when a launcher runs through an interactive shell. This benchmark does
not run those profiles; the historical `-E -s` invocation also ignores Python
environment overrides. SSH agents, proxies, DNS, VPNs and server load affect
connection establishment separately from this read-only local query.

## Reproduce the developer measurement

`scripts/benchmark_native_cli.py` accepts explicit native executable, historical
Python executable, source directory and output file paths. It uses Python only
as a developer timing harness; normal installed use needs no Python.

```powershell
python scripts/benchmark_native_cli.py --native C:\Tools\Ports\bin\ports.exe --python .venv\Scripts\python.exe --source . --output timing.json
```
